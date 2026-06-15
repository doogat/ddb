use async_graphql::dynamic::*;
use async_graphql::{Name, Value as GqlValue};
use ddb_core::error::DoogatError;
use ddb_core::types::TableSchema;
use indexmap::IndexMap;

use crate::actor::{ActorHandle, UpdateDoogatParams};
use crate::error::to_graphql_error;
use crate::read_pool::ReadPool;
use crate::schema::base_types::*;

/// Build the per-singleton `update_*` and `upsert_*` mutation fields for one
/// singleton typedef. Returned in registration order (update first, upsert
/// second) so the caller preserves the original schema field ordering.
pub(super) fn build_singleton_fields(schema: &TableSchema) -> (Field, Field) {
    let type_name = sanitize_type_name(&schema.table_name);
    let field_base = singleton_field_base(&schema.table_name);
    let update_field_name = format!("update_{field_base}");
    let upsert_field_name = format!("upsert_{field_base}");
    let table_name = schema.table_name.clone();
    let schema_clone = schema.clone();
    let update_desc = format!(
        "Update the {} singleton row. Rejects with SINGLETON_NOT_FOUND when the typedef is empty.",
        type_name
    );
    let upsert_desc = format!(
        "Upsert the {} singleton row. Returns id plus a created flag indicating whether the row was newly created.",
        type_name
    );

    let update_field = Field::new(
        &update_field_name,
        TypeRef::named_nn(&type_name),
        move |ctx| {
            let table_name = table_name.clone();
            let schema = schema_clone.clone();
            FieldFuture::new(async move {
                let a = ctx.data::<ActorHandle>()?;
                let pool = ctx.data::<ReadPool>()?;
                // Reuse the existing JSON-string fields transport so singleton
                // typedef mutations stay aligned with updateDoogat without
                // generating per-typedef input objects.
                let fields = parse_fields_json(ctx.args.try_get("input")?.string()?)
                    .map_err(|msg| async_graphql::ServerError::new(msg, None))?;
                let rows = pool
                    .aggregate_query_rows(
                        format!(
                            "SELECT id FROM \"{}\" LIMIT 1",
                            table_name.replace('"', "\"\"")
                        ),
                        Vec::new(),
                    )
                    .await
                    .map_err(to_graphql_error)?;
                let id = rows
                    .first()
                    .and_then(|row| row.first())
                    .cloned()
                    .ok_or_else(|| {
                        to_graphql_error(DoogatError::singleton_not_found(&table_name))
                    })?;
                let z = a
                    .update_doogat(UpdateDoogatParams {
                        id,
                        title: None,
                        body: None,
                        tags: None,
                        doogat_type: None,
                        fields,
                        unset_fields: vec![],
                    })
                    .await
                    .map_err(to_graphql_error)?;
                Ok(Some(FieldValue::owned_any(typed_doogat_to_value(
                    &z, &schema,
                ))))
            })
        },
    )
    .argument(
        InputValue::new("input", TypeRef::named_nn(TypeRef::STRING))
            .description("JSON object of typed field values to update."),
    )
    .description(&update_desc);

    let table_name = schema.table_name.clone();
    let upsert_field = Field::new(
        &upsert_field_name,
        TypeRef::named_nn("UpsertResult"),
        move |ctx| {
            let table_name = table_name.clone();
            FieldFuture::new(async move {
                let a = ctx.data::<ActorHandle>()?;
                let fields = parse_fields_json(ctx.args.try_get("input")?.string()?)
                    .map_err(|msg| async_graphql::ServerError::new(msg, None))?;
                let outcome = a
                    .upsert_singleton(table_name.clone(), fields)
                    .await
                    .map_err(to_graphql_error)?;
                let mut obj = IndexMap::new();
                obj.insert(Name::new("id"), GqlValue::String(outcome.id));
                obj.insert(Name::new("created"), GqlValue::Boolean(outcome.created));
                Ok(Some(FieldValue::owned_any(GqlValue::Object(obj))))
            })
        },
    )
    .argument(
        InputValue::new("input", TypeRef::named_nn(TypeRef::STRING))
            .description("JSON object of typed field values to upsert."),
    )
    .description(&upsert_desc);

    (update_field, upsert_field)
}
