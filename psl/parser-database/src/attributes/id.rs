use super::{FieldResolutionError, FieldResolvingSetup};
use crate::{
    ast::{self, WithName, WithSpan},
    attributes::{format_fields_in_error_with_leading_word, resolve_field_array_with_args},
    coerce,
    context::Context,
    types::{FieldWithArgs, IdAttribute, IndexFieldPath, ModelAttributes, ScalarField, SortOrder},
    DatamodelError, ScalarFieldId, StringId,
};
use std::borrow::Cow;

/// @@id on models
pub(super) fn model(model_data: &mut ModelAttributes, model_id: crate::ModelId, ctx: &mut Context<'_>) {
    let attr = ctx.current_attribute();
    let fields = match ctx.visit_default_arg("fields") {
        Ok(value) => value,
        Err(err) => return ctx.push_error(err),
    };

    let resolving = FieldResolvingSetup::OnlyTopLevel;

    let resolved_fields = match resolve_field_array_with_args(fields, attr.span, model_id, resolving, ctx) {
        Ok(fields) => fields,
        Err(FieldResolutionError::AlreadyDealtWith) => return,
        Err(FieldResolutionError::ProblematicFields {
            unknown_fields: unresolvable_fields,
            relation_fields,
        }) => {
            if !unresolvable_fields.is_empty() {
                let field_names = unresolvable_fields
                    .into_iter()
                    .map(|((file_id, top_id), field_name)| match top_id {
                        ast::TopId::CompositeType(ctid) => {
                            let ct_name = ctx.asts[(file_id, ctid)].name();

                            Cow::from(format!("{field_name} in type {ct_name}"))
                        }
                        ast::TopId::Model(_) => Cow::from(field_name),
                        _ => unreachable!(),
                    });

                let msg = format!(
                    "The multi field id declaration refers to the unknown {}.",
                    format_fields_in_error_with_leading_word(field_names)
                );

                ctx.push_error(DatamodelError::new_model_validation_error(
                    &msg,
                    "model",
                    ctx.asts[model_id].name(),
                    fields.span(),
                ));
            }

            if !relation_fields.is_empty() {
                let field_names = relation_fields.iter().map(|(f, _)| f.name());

                let msg = format!(
                    "The id definition refers to the relation {}. ID definitions must reference only scalar fields.",
                    format_fields_in_error_with_leading_word(field_names)
                );

                ctx.push_error(DatamodelError::new_model_validation_error(
                    &msg,
                    "model",
                    ctx.asts[model_id].name(),
                    attr.span,
                ));
            }

            return;
        }
    };

    let ast_model = &ctx.asts[model_id];

    // ID attribute fields must reference only required fields.
    let fields_that_are_not_required: Vec<&str> = resolved_fields
        .iter()
        .filter_map(|field| match field.path.field_in_index() {
            either::Either::Left(id) => {
                let ScalarField { model_id, field_id, .. } = ctx.types[id];
                let field = &ctx.asts[model_id][field_id];

                if field.arity.is_required() {
                    None
                } else {
                    Some(field.name())
                }
            }
            either::Either::Right((ctid, field_id)) => {
                let field = &ctx.asts[ctid][field_id];

                if field.arity.is_required() {
                    None
                } else {
                    Some(field.name())
                }
            }
        })
        .collect();

    if !fields_that_are_not_required.is_empty() && !model_data.is_ignored {
        ctx.push_error(DatamodelError::new_model_validation_error(
            &format!(
                "The id definition refers to the optional {}. ID definitions must reference only required fields.",
                format_fields_in_error_with_leading_word(fields_that_are_not_required)
            ),
            "model",
            ast_model.name(),
            attr.span,
        ))
    }

    if model_data.primary_key.is_some() {
        ctx.push_error(DatamodelError::new_model_validation_error(
            "Each model must have at most one id criteria. You can't have `@id` and `@@id` at the same time.",
            "model",
            ast_model.name(),
            ast_model.span(),
        ))
    }

    let (name, mapped_name) = {
        let mapped_name = primary_key_mapped_name(ctx);
        let name = super::get_name_argument(ctx);

        if let Some(name) = name {
            super::validate_client_name(attr.span(), ast_model.name(), name, "@@id", ctx);
        }

        (name, mapped_name)
    };

    let clustered = super::validate_clustering_setting(ctx);

    model_data.primary_key = Some(IdAttribute {
        name,
        source_attribute: ctx.current_attribute_id(),
        mapped_name,
        fields: resolved_fields,
        source_field: None,
        clustered,
    });
}

pub(super) fn field<'db>(
    ast_model: &'db ast::Model,
    scalar_field_id: ScalarFieldId,
    field_id: ast::FieldId,
    model_attributes: &mut ModelAttributes,
    ctx: &mut Context<'db>,
) {
    if model_attributes.primary_key.is_some() {
        ctx.push_error(DatamodelError::new_model_validation_error(
            "At most one field must be marked as the id field with the `@id` attribute.",
            "model",
            ast_model.name(),
            ast_model.span(),
        ))
    } else {
        let mapped_name = primary_key_mapped_name(ctx);

        let length = ctx
            .visit_optional_arg("length")
            .and_then(|length| coerce::integer(length, ctx.diagnostics))
            .map(|len| len as u32);

        let sort_order = match ctx
            .visit_optional_arg("sort")
            .and_then(|sort| coerce::constant(sort, ctx.diagnostics))
        {
            Some("Desc") => Some(SortOrder::Desc),
            Some("Asc") => Some(SortOrder::Asc),
            Some(other) => {
                ctx.push_attribute_validation_error(&format!(
                    "The `sort` argument can only be `Asc` or `Desc` you provided: {other}."
                ));
                None
            }
            None => None,
        };

        let clustered = super::validate_clustering_setting(ctx);

        let source_attribute = ctx.current_attribute_id();
        model_attributes.primary_key = Some(IdAttribute {
            name: None,
            mapped_name,
            source_attribute,
            fields: vec![FieldWithArgs {
                path: IndexFieldPath::new(scalar_field_id),
                sort_order,
                length,
                operator_class: None,
            }],
            source_field: Some(field_id),
            clustered,
        })
    }
}

// This has to be a separate step because we don't have the model attributes
// (which may include `@@ignored`) collected yet when we process field attributes.
pub(super) fn validate_id_field_arities(
    model_id: crate::ModelId,
    model_attributes: &ModelAttributes,
    ctx: &mut Context<'_>,
) {
    if model_attributes.is_ignored {
        return;
    }

    let Some(pk) = &model_attributes.primary_key else {
        return;
    };

    let ast_field = if let Some(field_id) = pk.source_field {
        &ctx.asts[model_id][field_id]
    } else {
        return;
    };

    if !ast_field.arity.is_required() {
        ctx.push_error(DatamodelError::new_attribute_validation_error(
            "Fields that are marked as id must be required.",
            "@id",
            ctx.asts[pk.source_attribute].span,
        ))
    }
}

fn primary_key_mapped_name(ctx: &mut Context<'_>) -> Option<StringId> {
    let mapped_name = match ctx
        .visit_optional_arg("map")
        .and_then(|name| coerce::string(name, ctx.diagnostics))
    {
        Some("") => {
            ctx.push_attribute_validation_error("The `map` argument cannot be an empty string.");
            None
        }
        Some(name) => Some(ctx.interner.intern(name)),
        None => None,
    };

    mapped_name
}
