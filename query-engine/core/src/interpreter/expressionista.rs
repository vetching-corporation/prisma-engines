use query_structure::IntoFilter;

use super::{expression::*, Env, ExpressionResult, InterpretationResult, InterpreterError};
use crate::{query_graph::*, Query};
use std::{
    collections::{HashSet, VecDeque},
    convert::TryInto,
};

pub(crate) struct Expressionista;

/// Helper accumulator struct.
#[derive(Default)]
struct IfNodeAcc {
    then: Option<(EdgeRef, NodeRef)>,
    _else: Option<(EdgeRef, NodeRef)>,
    other: Vec<(EdgeRef, NodeRef)>,
}

impl Expressionista {
    pub fn translate(mut graph: QueryGraph) -> InterpretationResult<Expression> {
        // Must collect the root nodes first, because the following iteration is mutating the graph
        let root_nodes: Vec<NodeRef> = graph.root_nodes().collect();

        root_nodes
            .into_iter()
            .map(|root_node| Self::build_expression(&mut graph, &root_node, vec![]))
            .collect::<InterpretationResult<Vec<Expression>>>()
            .map(|res| Expression::Sequence { seq: res })
    }

    fn build_expression(
        graph: &mut QueryGraph,
        node: &NodeRef,
        parent_edges: Vec<EdgeRef>,
    ) -> InterpretationResult<Expression> {
        match graph
            .node_content(node)
            .unwrap_or_else(|| panic!("Node content {} was empty", node.id()))
        {
            Node::Query(_) => Self::build_query_expression(graph, node, parent_edges),
            Node::Flow(_) => Self::build_flow_expression(graph, node, parent_edges),
            Node::Computation(_) => Self::build_computation_expression(graph, node, parent_edges),
            Node::Empty => Self::build_empty_expression(graph, node, parent_edges),
        }
    }

    fn build_query_expression(
        graph: &mut QueryGraph,
        node: &NodeRef,
        parent_edges: Vec<EdgeRef>,
    ) -> InterpretationResult<Expression> {
        graph.mark_visited(node);

        // Child edges are ordered, evaluation order is low to high in the graph, unless other rules override.
        let direct_children = graph.direct_child_pairs(node);

        let mut child_expressions = Self::process_children(graph, direct_children)?;

        let is_result = graph.is_result_node(node);
        let node_id = node.id();
        let node = graph.pluck_node(node);
        let into_expr = Box::new(|node: Node| {
            let query: Box<Query> = Box::new(node.try_into()?);
            Ok(Expression::Query { query })
        });

        let expr = Self::transform_node(graph, parent_edges, node, into_expr)?;

        if child_expressions.is_empty() {
            Ok(expr)
        } else {
            let node_binding_name = node_id;

            // Add a final statement to the evaluation if the current node has child nodes and is supposed to be the
            // final result, to make sure it propagates upwards.
            if is_result {
                child_expressions.push(Expression::Get {
                    binding_name: node_binding_name.clone(),
                });
            }

            Ok(Expression::Let {
                bindings: vec![Binding {
                    name: node_binding_name,
                    expr,
                }],
                expressions: child_expressions,
            })
        }
    }

    fn process_children(
        graph: &mut QueryGraph,
        mut child_pairs: Vec<(EdgeRef, NodeRef)>,
    ) -> InterpretationResult<Vec<Expression>> {
        // Find the positions of all result returning graph nodes.
        let mut result_positions: Vec<usize> = child_pairs
            .iter()
            .enumerate()
            .filter_map(|(ix, (_, child_node))| {
                if graph.subgraph_contains_result(child_node) {
                    Some(ix)
                } else {
                    None
                }
            })
            .collect();

        // Start removing the highest indices first to not invalidate subsequent removals.
        result_positions.sort_unstable();
        result_positions.reverse();

        let result_subgraphs: Vec<(EdgeRef, NodeRef)> = result_positions
            .into_iter()
            .map(|pos| child_pairs.remove(pos))
            .collect();

        // Because we split from right to left, everything remaining in `child_pairs`
        // doesn't belong into results, and is executed before all result scopes.
        let mut expressions: Vec<Expression> = child_pairs
            .into_iter()
            .map(|(_, node)| {
                let edges = graph.incoming_edges(&node);
                Self::build_expression(graph, &node, edges)
            })
            .collect::<InterpretationResult<Vec<Expression>>>()?;

        // Fold result scopes into one expression.
        if !result_subgraphs.is_empty() {
            let result_exp = Self::fold_result_scopes(graph, result_subgraphs)?;
            expressions.push(result_exp);
        }

        Ok(expressions)
    }

    fn build_empty_expression(
        graph: &mut QueryGraph,
        node: &NodeRef,
        parent_edges: Vec<EdgeRef>,
    ) -> InterpretationResult<Expression> {
        graph.mark_visited(node);

        let child_pairs = graph.direct_child_pairs(node);
        let exprs: Vec<Expression> = child_pairs
            .into_iter()
            .map(|(_, node)| Self::build_expression(graph, &node, graph.incoming_edges(&node)))
            .collect::<InterpretationResult<_>>()?;

        let into_expr = Box::new(move |_node: Node| Ok(Expression::Sequence { seq: exprs }));
        Self::transform_node(graph, parent_edges, Node::Empty, into_expr)
    }

    fn build_computation_expression(
        graph: &mut QueryGraph,
        node: &NodeRef,
        parent_edges: Vec<EdgeRef>,
    ) -> InterpretationResult<Expression> {
        graph.mark_visited(node);

        let node_id = node.id();
        let child_pairs = graph.direct_child_pairs(node);

        let exprs: Vec<Expression> = child_pairs
            .into_iter()
            .map(|(_, node)| Self::build_expression(graph, &node, graph.incoming_edges(&node)))
            .collect::<InterpretationResult<_>>()?;

        let node = graph.pluck_node(node);
        let into_expr = Box::new(move |node: Node| {
            Ok(Expression::Func {
                func: Box::new(move |_| match node {
                    Node::Computation(Computation::DiffLeftToRight(DiffNode { left, right })) => {
                        let left: HashSet<_> = left.into_iter().collect();
                        let right: HashSet<_> = right.into_iter().collect();

                        let diff = left.difference(&right);

                        Ok(Expression::Return {
                            result: Box::new(ExpressionResult::FixedResult(diff.cloned().collect())),
                        })
                    }

                    Node::Computation(Computation::DiffRightToLeft(DiffNode { left, right })) => {
                        let left: HashSet<_> = left.into_iter().collect();
                        let right: HashSet<_> = right.into_iter().collect();

                        let diff = right.difference(&left);

                        Ok(Expression::Return {
                            result: Box::new(ExpressionResult::FixedResult(diff.cloned().collect())),
                        })
                    }

                    _ => unreachable!(),
                }),
            })
        });

        let expr = Self::transform_node(graph, parent_edges, node, into_expr)?;

        if exprs.is_empty() {
            Ok(expr)
        } else {
            let node_binding_name = node_id;

            Ok(Expression::Let {
                bindings: vec![Binding {
                    name: node_binding_name,
                    expr,
                }],
                expressions: exprs,
            })
        }
    }

    fn build_flow_expression(
        graph: &mut QueryGraph,
        node: &NodeRef,
        parent_edges: Vec<EdgeRef>,
    ) -> InterpretationResult<Expression> {
        graph.mark_visited(node);

        match graph.node_content(node).unwrap() {
            Node::Flow(Flow::If { .. }) => Self::translate_if_node(graph, node, parent_edges),
            Node::Flow(Flow::Return(_)) => Self::translate_return_node(graph, node, parent_edges),
            _ => unreachable!(),
        }
    }

    fn translate_if_node(
        graph: &mut QueryGraph,
        node: &NodeRef,
        parent_edges: Vec<EdgeRef>,
    ) -> InterpretationResult<Expression> {
        let child_pairs = graph.direct_child_pairs(node);

        let if_node_info = child_pairs
            .into_iter()
            .fold(IfNodeAcc::default(), |mut acc, (edge, node)| {
                match graph.edge_content(&edge) {
                    Some(QueryGraphDependency::Then) => acc.then = Some((edge, node)),
                    Some(QueryGraphDependency::Else) => acc._else = Some((edge, node)),
                    _ => acc.other.push((edge, node)),
                };

                acc
            });

        let then_pair = if_node_info
            .then
            .expect("Expected if-node to always have a then edge to another node.");

        // Build expressions for both arms.
        let then_expr = Self::build_expression(graph, &then_pair.1, graph.incoming_edges(&then_pair.1))?;
        let else_expr = if_node_info
            ._else
            .into_iter()
            .map(|(_, node)| Self::build_expression(graph, &node, graph.incoming_edges(&node)))
            .collect::<InterpretationResult<Vec<_>>>()?;

        let child_expressions = Self::process_children(graph, if_node_info.other)?;

        let node_id = node.id();
        let node = graph.pluck_node(node);
        let into_expr = Box::new(move |node: Node| {
            let flow: Flow = node.try_into()?;

            if let Flow::If { rule, data } = flow {
                let if_expr = Expression::If {
                    func: Box::new(move || rule.matches_result(&ExpressionResult::FixedResult(data))),
                    then: vec![then_expr],
                    else_: else_expr,
                };

                let expr = if !child_expressions.is_empty() {
                    Expression::Let {
                        bindings: vec![Binding {
                            name: node_id,
                            expr: if_expr,
                        }],
                        expressions: child_expressions,
                    }
                } else {
                    if_expr
                };

                Ok(expr)
            } else {
                unreachable!()
            }
        });

        Self::transform_node(graph, parent_edges, node, into_expr)
    }

    fn translate_return_node(
        graph: &mut QueryGraph,
        node: &NodeRef,
        parent_edges: Vec<EdgeRef>,
    ) -> InterpretationResult<Expression> {
        let direct_children = graph.direct_child_pairs(node);
        let child_expressions = Self::process_children(graph, direct_children)?;

        let into_expr = Box::new(move |node: Node| {
            let flow: Flow = node.try_into()?;

            if let Flow::Return(result) = flow {
                Ok(Expression::Return {
                    result: ExpressionResult::FixedResult(result).into(),
                })
            } else {
                unreachable!()
            }
        });

        let node_binding_name = node.id();
        let node = graph.pluck_node(node);
        let expr = Self::transform_node(graph, parent_edges, node, into_expr)?;

        if child_expressions.is_empty() {
            Ok(expr)
        } else {
            Ok(Expression::Let {
                bindings: vec![Binding {
                    name: node_binding_name,
                    expr,
                }],
                expressions: child_expressions,
            })
        }
    }

    /// Runs transformer functions (e.g. `ParentIdsFn`) via `Expression::Func` if necessary, or if none present,
    /// builds an expression directly. `into_expr` does the final expression building based on the node coming in.
    fn transform_node(
        graph: &mut QueryGraph,
        parent_edges: Vec<EdgeRef>,
        node: Node,
        into_expr: Box<dyn FnOnce(Node) -> InterpretationResult<Expression> + Send + Sync + 'static>,
    ) -> InterpretationResult<Expression> {
        if parent_edges.is_empty() {
            into_expr(node)
        } else {
            // Collect all parent ID dependency tuples (transformers).
            let parent_id_deps = Self::collect_parent_transformers(graph, parent_edges);

            // If there is at least one parent ID dependency we build a func to run the transformer(s),
            // else just render a flat query expression.
            if parent_id_deps.is_empty() {
                into_expr(node)
            } else {
                Ok(Expression::Func {
                    func: Box::new(move |env: Env| {
                        // Run transformers in order on the query to retrieve the final, transformed, query.
                        let node: InterpretationResult<Node> =
                            parent_id_deps
                                .into_iter()
                                .try_fold(node, |mut node, (parent_binding_name, dependency)| {
                                    let binding = match env.get(&parent_binding_name) {
                                        Some(binding) => Ok(binding),
                                        None => Err(InterpreterError::EnvVarNotFound(format!(
                                            "Expected parent binding '{parent_binding_name}' to be present."
                                        ))),
                                    }?;

                                    let res = match dependency {
                                        QueryGraphDependency::ProjectedDataDependency(selection, f, expectation) => {
                                            expectation
                                                .map(|expect| expect.check(binding))
                                                .transpose()
                                                .map_err(Into::into)
                                                .and(binding.as_selection_results(&selection))
                                                .and_then(|parent_selections| Ok(f(node, parent_selections)?))
                                        }

                                        QueryGraphDependency::ProjectedDataSinkDependency(
                                            selection,
                                            consumer,
                                            expectation,
                                        ) => expectation
                                            .map(|expect| expect.check(binding))
                                            .transpose()
                                            .map_err(Into::into)
                                            .and(binding.as_selection_results(&selection))
                                            .and_then(|mut parent_selections| {
                                                match consumer {
                                                    RowSink::Single(field) => {
                                                        let row = parent_selections.pop().expect(
                                                            "parent selection should be present after validation",
                                                        );
                                                        *field.node_input_field(&mut node) = row;
                                                    }
                                                    RowSink::All(field) => {
                                                        *field.node_input_field(&mut node) = parent_selections
                                                    }
                                                    RowSink::AtMostOne(field) => {
                                                        parent_selections.truncate(1);
                                                        *field.node_input_field(&mut node) = parent_selections;
                                                    }
                                                    RowSink::ExactlyOne(field) => {
                                                        let row = parent_selections.pop().expect(
                                                            "parent selection should be present after validation",
                                                        );
                                                        *field.node_input_field(&mut node) = vec![row];
                                                    }
                                                    RowSink::ExactlyOneFilter(filter) => {
                                                        let row = parent_selections.pop().expect(
                                                            "parent selection should be present after validation",
                                                        );
                                                        *filter.node_input_field(&mut node) = row.filter();
                                                    }
                                                    RowSink::ExactlyOneWriteArgs(selection, field) => {
                                                        let row = parent_selections.pop().expect(
                                                            "parent selection should be present after validation",
                                                        );
                                                        let model = node.as_query().map(Query::model);
                                                        let args = field.node_input_field(&mut node);
                                                        for arg in args {
                                                            arg.inject(selection.assimilate(row.clone())?);
                                                            if let Some(model) = &model {
                                                                arg.update_datetimes(model);
                                                            }
                                                        }
                                                    }
                                                    RowSink::Discard => {}
                                                }
                                                Ok(node)
                                            }),

                                        QueryGraphDependency::DataDependency(consumer, expectation) => {
                                            if let Some(expectation) = expectation {
                                                expectation.check(binding)?;
                                            }
                                            match consumer {
                                                RowCountSink::Discard => {}
                                            }
                                            Ok(node)
                                        }

                                        _ => unreachable!(),
                                    };

                                    res.map_err(|err| {
                                        InterpreterError::InterpretationError(
                                            format!("Error for binding '{parent_binding_name}'"),
                                            Some(Box::new(err)),
                                        )
                                    })
                                });

                        into_expr(node?)
                    }),
                })
            }
        }
    }

    /// Collects all edge dependencies that perform a node transformation based on the parent.
    fn collect_parent_transformers(
        graph: &mut QueryGraph,
        parent_edges: Vec<EdgeRef>,
    ) -> Vec<(String, QueryGraphDependency)> {
        parent_edges
            .into_iter()
            .filter_map(|edge| match graph.pluck_edge(&edge) {
                x @ (QueryGraphDependency::DataDependency(_, _)
                | QueryGraphDependency::ProjectedDataDependency(_, _, _)
                | QueryGraphDependency::ProjectedDataSinkDependency(_, _, _)) => {
                    let parent_binding_name = graph.edge_source(&edge).id();
                    Some((parent_binding_name, x))
                }
                _ => None,
            })
            .collect()
    }

    fn fold_result_scopes(
        graph: &mut QueryGraph,
        result_subgraphs: Vec<(EdgeRef, NodeRef)>,
    ) -> InterpretationResult<Expression> {
        // if the subgraphs all point to the same result node, we fold them in sequence
        // if not, we can separate them with a getfirstnonempty
        let bindings: Vec<Binding> = result_subgraphs
            .into_iter()
            .map(|(_, node)| {
                let name = node.id();
                let edges = graph.incoming_edges(&node);
                let expr = Self::build_expression(graph, &node, edges)?;

                Ok(Binding { name, expr })
            })
            .collect::<InterpretationResult<Vec<Binding>>>()?;

        let result_binding_names = bindings.iter().map(|b| b.name.clone()).collect();
        let result_nodes: Vec<NodeRef> = graph.result_nodes().collect();

        if result_nodes.len() == 1 {
            let mut exprs: VecDeque<Expression> = bindings
                .into_iter()
                .map(|binding| Expression::Let {
                    bindings: vec![binding],
                    expressions: vec![],
                })
                .collect();

            if let Some(Expression::Let { bindings, expressions }) = exprs.back_mut() {
                expressions.push(Expression::Get {
                    binding_name: bindings
                        .last()
                        .map(|b| b.name.clone())
                        .expect("Expected let binding to have at least one expr."),
                })
            }

            let first = exprs.pop_front().unwrap();
            let folded = exprs.into_iter().fold(first, |mut acc, next| {
                if let Expression::Let {
                    bindings: _,
                    ref mut expressions,
                } = acc
                {
                    expressions.push(next);
                }

                acc
            });

            Ok(folded)
        } else {
            Ok(Expression::Let {
                bindings,
                expressions: vec![Expression::GetFirstNonEmpty {
                    binding_names: result_binding_names,
                }],
            })
        }
    }
}
