use crate::proxy::{CommonProxy, DriverProxy};
use crate::types::{AdapterProvider, Query};
use crate::{JsObject, JsResult};

use super::conversion;
use crate::send_future::UnsafeFuture;
use async_trait::async_trait;
use futures::Future;
use quaint::connector::{AdapterName, DescribedQuery, ExternalConnectionInfo, ExternalConnector};
use quaint::{
    connector::{metrics, IsolationLevel, Transaction},
    prelude::{Query as QuaintQuery, Queryable as QuaintQueryable, ResultSet, TransactionCapable},
    visitor::{self, Visitor},
};
use telemetry::formatting::QueryForTracing;
use tracing::{info_span, Instrument};

/// A JsQueryable adapts a Proxy to implement quaint's Queryable interface. It has the
/// responsibility of transforming inputs and outputs of `query` and `execute` methods from quaint
/// types to types that can be translated into javascript and viceversa. This is to let the rest of
/// the query engine work as if it was using quaint itself. The aforementioned transformations are:
///
/// Transforming a `quaint::ast::Query` into SQL by visiting it for the specific flavour of SQL
/// expected by the client connector. (eg. using the mysql visitor for the Planetscale client
/// connector)
///
/// Transforming a `JSResultSet` (what client connectors implemented in javascript provide)
/// into a `quaint::connector::result_set::ResultSet`. A quaint `ResultSet` is basically a vector
/// of `quaint::Value` but said type is a tagged enum, with non-unit variants that cannot be converted to javascript as is.
pub(crate) struct JsBaseQueryable {
    pub(crate) proxy: CommonProxy,
    pub provider: AdapterProvider,
    pub adapter_name: AdapterName,
    pub(crate) db_system_name: &'static str,
}

impl JsBaseQueryable {
    pub(crate) fn new(proxy: CommonProxy) -> Self {
        let provider = proxy.provider;
        let adapter_name = proxy.adapter_name;
        let db_system_name = provider.db_system_name();
        Self {
            proxy,
            adapter_name,
            provider,
            db_system_name,
        }
    }

    /// visit a quaint query AST according to the provider of the JS connector
    fn visit_quaint_query<'a>(&self, q: QuaintQuery<'a>) -> quaint::Result<(String, Vec<quaint::Value<'a>>)> {
        match self.provider {
            #[cfg(feature = "mysql")]
            AdapterProvider::Mysql => visitor::Mysql::build(q),
            #[cfg(feature = "postgresql")]
            AdapterProvider::Postgres => visitor::Postgres::build(q),
            #[cfg(feature = "sqlite")]
            AdapterProvider::Sqlite => visitor::Sqlite::build(q),
            #[cfg(feature = "mssql")]
            AdapterProvider::SqlServer => visitor::Mssql::build(q),
        }
    }

    async fn build_query(&self, sql: &str, values: &[quaint::Value<'_>]) -> quaint::Result<Query> {
        let sql: String = sql.to_string();

        let args_converter = match self.provider {
            #[cfg(feature = "postgresql")]
            AdapterProvider::Postgres => conversion::postgres::value_to_js_arg,
            #[cfg(feature = "sqlite")]
            AdapterProvider::Sqlite => conversion::sqlite::value_to_js_arg,
            #[cfg(feature = "mysql")]
            AdapterProvider::Mysql => conversion::mysql::value_to_js_arg,
            #[cfg(feature = "mssql")]
            AdapterProvider::SqlServer => conversion::mssql::value_to_js_arg,
        };

        let args = values
            .iter()
            .map(args_converter)
            .collect::<serde_json::Result<Vec<conversion::JSArg>>>()?;

        let arg_types = values
            .iter()
            .map(conversion::value_to_js_arg_type)
            .collect::<Vec<conversion::JSArgType>>();

        Ok(Query { sql, args, arg_types })
    }
}

#[async_trait]
impl QuaintQueryable for JsBaseQueryable {
    async fn query(&self, q: QuaintQuery<'_>) -> quaint::Result<ResultSet> {
        let (sql, params) = self.visit_quaint_query(q)?;
        self.query_raw(&sql, &params).await
    }

    async fn query_raw(&self, sql: &str, params: &[quaint::Value<'_>]) -> quaint::Result<ResultSet> {
        metrics::query("js.query_raw", self.db_system_name, sql, params, move || async move {
            self.do_query_raw(sql, params).await
        })
        .await
    }

    async fn query_raw_typed(&self, sql: &str, params: &[quaint::Value<'_>]) -> quaint::Result<ResultSet> {
        self.query_raw(sql, params).await
    }

    async fn describe_query(&self, sql: &str) -> quaint::Result<DescribedQuery> {
        self.describe_query(sql).await
    }

    async fn execute(&self, q: QuaintQuery<'_>) -> quaint::Result<u64> {
        let (sql, params) = self.visit_quaint_query(q)?;
        self.execute_raw(&sql, &params).await
    }

    async fn execute_raw(&self, sql: &str, params: &[quaint::Value<'_>]) -> quaint::Result<u64> {
        metrics::query("js.execute_raw", self.db_system_name, sql, params, move || async move {
            self.do_execute_raw(sql, params).await
        })
        .await
    }

    async fn execute_raw_typed(&self, sql: &str, params: &[quaint::Value<'_>]) -> quaint::Result<u64> {
        self.execute_raw(sql, params).await
    }

    async fn raw_cmd(&self, cmd: &str) -> quaint::Result<()> {
        let params = &[];
        metrics::query("js.raw_cmd", self.db_system_name, cmd, params, move || async move {
            self.do_execute_raw(cmd, params).await?;
            Ok(())
        })
        .await
    }

    // Note: Needed by the Wasm Schema Engine only.
    async fn version(&self) -> quaint::Result<Option<String>> {
        let version_expr: &'static str = match self.provider {
            #[cfg(feature = "mysql")]
            AdapterProvider::Mysql => visitor::Mysql::version_expr(),
            #[cfg(feature = "postgresql")]
            AdapterProvider::Postgres => visitor::Postgres::version_expr(),
            #[cfg(feature = "sqlite")]
            AdapterProvider::Sqlite => visitor::Sqlite::version_expr(),
            #[cfg(feature = "mssql")]
            AdapterProvider::SqlServer => visitor::Mssql::version_expr(),
        };

        let query = format!(r#"SELECT {version_expr} AS version"#);
        let rows = self.query_raw(query.as_str(), &[]).await?;

        let version_string = rows
            .first()
            .and_then(|row| row.get("version").and_then(|version| version.to_string()));

        Ok(version_string)
    }

    fn is_healthy(&self) -> bool {
        // Note: JS Connectors don't use this method.
        true
    }

    /// Sets the transaction isolation level to given value.
    /// Implementers have to make sure that the passed isolation level is valid for the underlying database.
    async fn set_tx_isolation_level(&self, isolation_level: IsolationLevel) -> quaint::Result<()> {
        self.raw_cmd(&format!("SET TRANSACTION ISOLATION LEVEL {isolation_level}"))
            .await
    }

    fn requires_isolation_first(&self) -> bool {
        match self.provider {
            #[cfg(feature = "mysql")]
            AdapterProvider::Mysql => true,
            #[cfg(feature = "postgresql")]
            AdapterProvider::Postgres => false,
            #[cfg(feature = "sqlite")]
            AdapterProvider::Sqlite => false,
            #[cfg(feature = "mssql")]
            AdapterProvider::SqlServer => true,
        }
    }
}

impl JsBaseQueryable {
    pub fn phantom_query_message(stmt: &str) -> String {
        format!(r#"-- Implicit "{stmt}" query via underlying driver"#)
    }

    async fn do_query_raw_inner(&self, sql: &str, params: &[quaint::Value<'_>]) -> quaint::Result<ResultSet> {
        let serialization_span = info_span!(
            "prisma:engine:js:query:args",
            "otel.kind" = "client",
            "prisma.db_query.params.count" = params.len(),
            user_facing = true,
        );
        let query = self.build_query(sql, params).instrument(serialization_span).await?;

        let sql_span = info_span!(
            "prisma:engine:js:query:sql",
            "otel.kind" = "client",
            "db.system" = %self.db_system_name,
            "db.query.text" = %QueryForTracing(sql),
            user_facing = true,
        );
        let result_set = self.proxy.query_raw(query).instrument(sql_span).await?;

        let _deserialization_span = info_span!(
            "prisma:engine:js:query:result",
            "otel.kind" = "client",
            "db.response.returned_rows" = result_set.len(),
            user_facing = true,
        )
        .entered();

        result_set.try_into()
    }

    fn do_query_raw<'a>(
        &'a self,
        sql: &'a str,
        params: &'a [quaint::Value<'a>],
    ) -> UnsafeFuture<impl Future<Output = quaint::Result<ResultSet>> + 'a> {
        UnsafeFuture(self.do_query_raw_inner(sql, params))
    }

    async fn do_execute_raw_inner(&self, sql: &str, params: &[quaint::Value<'_>]) -> quaint::Result<u64> {
        let serialization_span = info_span!(
            "prisma:engine:js:query:args",
            "otel.kind" = "client",
            "prisma.db_query.params.count" = params.len(),
            user_facing = true,
        );
        let query = self.build_query(sql, params).instrument(serialization_span).await?;

        let sql_span = info_span!(
            "prisma:engine:js:query:sql",
            "otel.kind" = "client",
            "db.system" = %self.db_system_name,
            "db.query.text" = %QueryForTracing(sql),
            user_facing = true,
        );
        let affected_rows = self.proxy.execute_raw(query).instrument(sql_span).await?;

        Ok(affected_rows as u64)
    }

    fn do_execute_raw<'a>(
        &'a self,
        sql: &'a str,
        params: &'a [quaint::Value<'a>],
    ) -> UnsafeFuture<impl Future<Output = quaint::Result<u64>> + 'a> {
        UnsafeFuture(self.do_execute_raw_inner(sql, params))
    }
}

/// A JsQueryable adapts a Proxy to implement quaint's Queryable interface. It has the
/// responsibility of transforming inputs and outputs of `query` and `execute` methods from quaint
/// types to types that can be translated into javascript and viceversa. This is to let the rest of
/// the query engine work as if it was using quaint itself. The aforementioned transformations are:
///
/// Transforming a `quaint::ast::Query` into SQL by visiting it for the specific flavour of SQL
/// expected by the client connector. (eg. using the mysql visitor for the Planetscale client
/// connector)
///
/// Transforming a `JSResultSet` (what client connectors implemented in javascript provide)
/// into a `quaint::connector::result_set::ResultSet`. A quaint `ResultSet` is basically a vector
/// of `quaint::Value` but said type is a tagged enum, with non-unit variants that cannot be converted to javascript as is.
///
pub struct JsQueryable {
    inner: JsBaseQueryable,
    driver_proxy: DriverProxy,
}

impl std::fmt::Display for JsQueryable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "JSQueryable(driver)")
    }
}

impl std::fmt::Debug for JsQueryable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "JSQueryable(driver)")
    }
}

#[async_trait]
impl ExternalConnector for JsQueryable {
    fn adapter_name(&self) -> AdapterName {
        self.inner.adapter_name
    }

    fn provider(&self) -> AdapterProvider {
        self.inner.provider
    }

    async fn execute_script(&self, script: &str) -> quaint::Result<()> {
        self.driver_proxy.execute_script(script.to_owned()).await
    }

    async fn get_connection_info(&self) -> quaint::Result<ExternalConnectionInfo> {
        let conn_info = self.driver_proxy.get_connection_info().await?;

        Ok(conn_info.into_external_connection_info(&self.inner.provider))
    }

    async fn dispose(&self) -> quaint::Result<()> {
        self.driver_proxy.dispose().await
    }
}

#[async_trait]
impl QuaintQueryable for JsQueryable {
    async fn query(&self, q: QuaintQuery<'_>) -> quaint::Result<ResultSet> {
        self.inner.query(q).await
    }

    async fn query_raw(&self, sql: &str, params: &[quaint::Value<'_>]) -> quaint::Result<ResultSet> {
        self.inner.query_raw(sql, params).await
    }

    async fn query_raw_typed(&self, sql: &str, params: &[quaint::Value<'_>]) -> quaint::Result<ResultSet> {
        self.inner.query_raw_typed(sql, params).await
    }

    async fn describe_query(&self, sql: &str) -> quaint::Result<DescribedQuery> {
        self.inner.describe_query(sql).await
    }

    async fn execute(&self, q: QuaintQuery<'_>) -> quaint::Result<u64> {
        self.inner.execute(q).await
    }

    async fn execute_raw(&self, sql: &str, params: &[quaint::Value<'_>]) -> quaint::Result<u64> {
        self.inner.execute_raw(sql, params).await
    }

    async fn execute_raw_typed(&self, sql: &str, params: &[quaint::Value<'_>]) -> quaint::Result<u64> {
        self.inner.execute_raw_typed(sql, params).await
    }

    async fn raw_cmd(&self, cmd: &str) -> quaint::Result<()> {
        self.inner.raw_cmd(cmd).await
    }

    async fn version(&self) -> quaint::Result<Option<String>> {
        self.inner.version().await
    }

    fn is_healthy(&self) -> bool {
        self.inner.is_healthy()
    }

    async fn set_tx_isolation_level(&self, isolation_level: IsolationLevel) -> quaint::Result<()> {
        self.inner.set_tx_isolation_level(isolation_level).await
    }

    fn requires_isolation_first(&self) -> bool {
        self.inner.requires_isolation_first()
    }
}

impl JsQueryable {
    async fn start_transaction_inner<'a>(
        &'a self,
        isolation: Option<IsolationLevel>,
    ) -> quaint::Result<Box<dyn Transaction + 'a>> {
        let tx = self.driver_proxy.start_transaction(isolation).await?;
        self.server_reset_query(tx.as_ref()).await?;
        Ok(tx)
    }

    pub fn dispose_non_blocking(&self) {
        self.driver_proxy.dispose_non_blocking();
    }
}

#[async_trait]
impl TransactionCapable for JsQueryable {
    async fn start_transaction<'a>(
        &'a self,
        isolation: Option<IsolationLevel>,
    ) -> quaint::Result<Box<dyn Transaction + 'a>> {
        UnsafeFuture(self.start_transaction_inner(isolation)).await
    }
}

pub fn queryable_from_js(driver: JsObject) -> JsQueryable {
    let common = CommonProxy::new(&driver).unwrap();
    let driver_proxy = DriverProxy::new(&driver).unwrap();

    JsQueryable {
        inner: JsBaseQueryable::new(common),
        driver_proxy,
    }
}

#[cfg(target_arch = "wasm32")]
impl super::wasm::FromJsValue for JsBaseQueryable {
    fn from_js_value(value: wasm_bindgen::prelude::JsValue) -> JsResult<Self> {
        use wasm_bindgen::JsCast;

        let object = value.dyn_into::<JsObject>()?;
        let common_proxy = CommonProxy::new(&object)?;
        Ok(Self::new(common_proxy))
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl ::napi::bindgen_prelude::FromNapiValue for JsBaseQueryable {
    unsafe fn from_napi_value(env: napi::sys::napi_env, napi_val: napi::sys::napi_value) -> JsResult<Self> {
        let object = JsObject::from_napi_value(env, napi_val)?;
        let common_proxy = CommonProxy::new(&object)?;
        Ok(Self::new(common_proxy))
    }
}

#[cfg(target_arch = "wasm32")]
impl super::wasm::FromJsValue for JsQueryable {
    fn from_js_value(value: wasm_bindgen::prelude::JsValue) -> JsResult<Self> {
        use wasm_bindgen::JsCast;

        let object = value.dyn_into::<JsObject>()?;
        let common_proxy = CommonProxy::new(&object)?;
        let driver_proxy = DriverProxy::new(&object)?;
        Ok(Self {
            inner: JsBaseQueryable::new(common_proxy),
            driver_proxy,
        })
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl ::napi::bindgen_prelude::FromNapiValue for JsQueryable {
    unsafe fn from_napi_value(env: napi::sys::napi_env, napi_val: napi::sys::napi_value) -> JsResult<Self> {
        let object = JsObject::from_napi_value(env, napi_val)?;
        let common_proxy = CommonProxy::new(&object)?;
        let driver_proxy = DriverProxy::new(&object)?;

        Ok(Self {
            inner: JsBaseQueryable::new(common_proxy),
            driver_proxy,
        })
    }
}
