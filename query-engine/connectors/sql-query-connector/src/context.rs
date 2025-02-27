use std::{cell::RefCell, collections::HashMap};

use quaint::prelude::ConnectionInfo;
use telemetry::TraceParent;
use tokio::task_local;


task_local! {
    pub static MULTITENANCY_CONTEXT: RefCell<HashMap<String, String>>;
}


pub(super) struct Context<'a> {
    connection_info: &'a ConnectionInfo,
    pub(crate) traceparent: Option<TraceParent>,
    /// Maximum rows allowed at once for an insert query.
    /// None is unlimited.
    pub(crate) max_insert_rows: Option<usize>,
    /// Maximum number of bind parameters allowed for a single query.
    /// None is unlimited.
    pub(crate) max_bind_values: Option<usize>,
}

impl<'a> Context<'a> {
    pub(crate) fn new(connection_info: &'a ConnectionInfo, traceparent: Option<TraceParent>) -> Self {
        let max_insert_rows = connection_info.max_insert_rows();
        let max_bind_values = connection_info.max_bind_values();

        Context {
            connection_info,
            traceparent,
            max_insert_rows,
            max_bind_values: Some(max_bind_values),
        }
    }

    pub(crate) fn schema_name(&self) -> &str {
        self.connection_info.schema_name()
    }
}
