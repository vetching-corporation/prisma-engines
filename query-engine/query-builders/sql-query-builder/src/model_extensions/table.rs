use crate::{model_extensions::AsColumns, Context};
use quaint::ast::{Column, Table};
use query_structure::Model;

/**
 * Changed by @vetching-corporation
 * Author: nfl1ryxditimo12@gmail.com
 * Date: 2025-06-16
 * Note: Add `target_schema` function to support dynamic schema
 */
pub(crate) fn db_name_with_schema(model: &Model, ctx: &Context<'_>) -> Table<'static> {
    let schema_prefix = model
        .walker()
        .schema_name()
        .and_then(|origin_schema| ctx.target_schema(origin_schema).or(Some(origin_schema.to_owned())))
        .or_else(|| ctx.schema_name().map(ToOwned::to_owned));

    let model_db_name = model.db_name().to_owned();

    if let Some(schema_prefix) = schema_prefix {
        (schema_prefix, model_db_name).into()
    } else {
        model_db_name.into()
    }
}

pub trait AsTable {
    fn as_table(&self, ctx: &Context<'_>) -> Table<'static>;
}

impl AsTable for Model {
    fn as_table(&self, ctx: &Context<'_>) -> Table<'static> {
        let table: Table<'static> = db_name_with_schema(self, ctx);

        let id_cols: Vec<Column<'static>> = self
            .primary_identifier()
            .as_scalar_fields()
            .expect("Primary identifier has non-scalar fields.")
            .as_columns(ctx)
            .collect();

        let table = table.add_unique_index(id_cols);

        self.unique_indexes().fold(table, |table, index| {
            let fields: Vec<_> = index
                .fields()
                .map(|f| query_structure::ScalarFieldRef::from((self.dm.clone(), f)))
                .collect();
            let index: Vec<Column<'static>> = fields.as_columns(ctx).collect();
            table.add_unique_index(index)
        })
    }
}
