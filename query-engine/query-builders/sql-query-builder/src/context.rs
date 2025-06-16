use std::sync::{self, atomic::AtomicUsize};

use quaint::prelude::{ConnectionInfo, SqlFamily};
use telemetry::TraceParent;

use crate::filter::alias::Alias;
use crate::dynamic_schema::DynamicSchema;

pub struct Context<'a> {
    connection_info: &'a ConnectionInfo,
    pub(crate) traceparent: Option<TraceParent>,
    /// Maximum rows allowed at once for an insert query.
    /// None is unlimited.
    pub(crate) max_insert_rows: Option<usize>,
    /// Maximum number of bind parameters allowed for a single query.
    /// None is unlimited.
    pub(crate) max_bind_values: Option<usize>,

    dynamic_schema: DynamicSchema,

    alias_counter: AtomicUsize,
}

impl<'a> Context<'a> {
    pub fn new(connection_info: &'a ConnectionInfo, traceparent: Option<TraceParent>) -> Self {
        let max_insert_rows = connection_info.max_insert_rows();
        let max_bind_values = connection_info.max_bind_values();

        Context {
            connection_info,
            traceparent,
            max_insert_rows,
            max_bind_values: Some(max_bind_values),
            dynamic_schema: DynamicSchema::default(),
            alias_counter: Default::default(),
        }
    }

    pub fn new_with_dynamic_schema(connection_info: &'a ConnectionInfo, dynamic_schema: DynamicSchema, traceparent: Option<TraceParent>) -> Self {
        let mut ctx = Context::new(connection_info, traceparent);
        ctx.dynamic_schema = dynamic_schema;
        ctx
    }

    pub fn traceparent(&self) -> Option<TraceParent> {
        self.traceparent
    }

    pub(crate) fn schema_name(&self) -> &str {
        self.connection_info.schema_name()
    }

    pub fn sql_family(&self) -> SqlFamily {
        self.connection_info.sql_family()
    }

    pub fn max_insert_rows(&self) -> Option<usize> {
        self.max_insert_rows
    }

    pub fn max_bind_values(&self) -> Option<usize> {
        self.max_bind_values
    }

    pub(crate) fn next_table_alias(&self) -> Alias {
        Alias::Table(self.alias_counter.fetch_add(1, sync::atomic::Ordering::SeqCst))
    }

    pub(crate) fn next_join_alias(&self) -> Alias {
        Alias::Join(self.alias_counter.fetch_add(1, sync::atomic::Ordering::SeqCst))
    }

    pub fn target_schema(&self, origin_schema: &str) -> Option<String> {
        if self.dynamic_schema.is_empty() {
            return Some(origin_schema.to_owned());
        }

        self.dynamic_schema.get(origin_schema).map(|s| s.to_owned())
    }
}
