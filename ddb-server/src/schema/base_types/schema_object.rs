use std::collections::HashMap;

use async_graphql::dynamic::*;
use async_graphql::Value as GqlValue;
use ddb_core::types::{ColumnDef, TableSchema};

use super::naming::*;
use super::to_gql::*;
use super::TypeSchemaMap;

// -- Type builders & helpers --

/// Check if a string is a valid GraphQL name (`/[_A-Za-z][_0-9A-Za-z]*/`).
pub fn is_valid_graphql_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c == '_' || c.is_ascii_alphabetic() => {}
        _ => return false,
    }
    chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
}

pub(crate) fn simple_field(name: &str, type_ref: TypeRef) -> Field {
    let name_owned = name.to_string();
    Field::new(name, type_ref, move |ctx| {
        let name = name_owned.clone();
        FieldFuture::new(async move {
            let obj = ctx.parent_value.try_downcast_ref::<GqlValue>()?;
            Ok(obj_field(obj, &name))
        })
    })
}

/// Convert a GqlValue into the correct FieldValue variant:
/// objects → owned_any, lists → recursive, scalars → value.
pub(crate) fn gql_to_field_value(val: GqlValue) -> FieldValue<'static> {
    match val {
        GqlValue::Object(_) => FieldValue::owned_any(val),
        GqlValue::List(items) => FieldValue::list(items.into_iter().map(gql_to_field_value)),
        other => FieldValue::value(other),
    }
}

pub(crate) fn obj_field(obj: &GqlValue, key: &str) -> Option<FieldValue<'static>> {
    match obj {
        GqlValue::Object(map) => Some(gql_to_field_value(map.get(key)?.clone())),
        _ => None,
    }
}

/// Field names already defined by `doogat_object` (base type).
/// Used to avoid duplicate field registration on typed objects.
pub(super) const BASE_DOOGAT_FIELDS: &[&str] = &[
    "id",
    "title",
    "date",
    "type",
    "tags",
    "body",
    "path",
    "fields",
    "links",
    "attachments",
    "updated_at",
    "created_at",
];

/// PRD 00129 §4 (Option B): add per-typedef nested accessors to the base
/// `Doogat` object so `Doogat.link { url }` resolves the typed link row
/// in a single round trip when the doogat's type matches, and returns
/// null when it doesn't. Same accessor lights up on every mutation
/// response (`createDoogat`, `updateDoogat`, etc.) and every read path
/// (`doogat(id:)`, `doogats`, nested references), because all of them
/// share this Object.
pub(crate) fn add_typed_doogat_accessors(
    mut doogat: Object,
    type_schemas: &[TableSchema],
    known_types: &HashMap<String, String>,
) -> Object {
    for ts in type_schemas {
        let target_table = ts.table_name.clone();
        let target_typename = match known_types.get(&target_table) {
            Some(name) => name.clone(),
            None => continue, // not a registered GraphQL type (kebab collision, etc.)
        };
        let field_name = sanitize_field_name(&target_table);
        // Don't shadow the reserved Doogat fields (id/title/type/tags/etc).
        // `BASE_DOOGAT_FIELDS` enumerates them; collision = skip.
        if BASE_DOOGAT_FIELDS.contains(&field_name.as_str()) {
            continue;
        }
        doogat = doogat.field(Field::new(
            &field_name,
            TypeRef::named(&target_typename),
            move |ctx| {
                let target_table = target_table.clone();
                FieldFuture::new(async move {
                    let parent = ctx.parent_value.try_downcast_ref::<GqlValue>()?;
                    let GqlValue::Object(map) = parent else {
                        return Ok(None);
                    };
                    let id = match map.get("id") {
                        Some(GqlValue::String(s)) if !s.is_empty() => s.clone(),
                        _ => return Ok(None),
                    };
                    let ztype = match map.get("type") {
                        Some(GqlValue::String(s)) => s.clone(),
                        _ => return Ok(None),
                    };
                    if ztype != target_table {
                        return Ok(None);
                    }
                    let pool = ctx.data::<crate::read_pool::ReadPool>()?;
                    let schemas = ctx.data::<TypeSchemaMap>()?;
                    let z = match pool.get_doogat(id).await {
                        Ok(z) => z,
                        Err(_) => return Ok(None),
                    };
                    let val = match schemas.0.get(&target_table) {
                        Some(ts) => typed_doogat_to_value(&z, ts),
                        None => doogat_to_value(&z),
                    };
                    Ok(Some(FieldValue::owned_any(val)))
                })
            },
        ));
    }
    doogat
}

pub(crate) fn doogat_object(name: &str) -> Object {
    Object::new(name)
        .field(simple_field("id", TypeRef::named_nn(TypeRef::ID)))
        .field(simple_field("title", TypeRef::named(TypeRef::STRING)))
        .field(simple_field("date", TypeRef::named(TypeRef::STRING)))
        .field(simple_field("type", TypeRef::named(TypeRef::STRING)))
        .field(Field::new(
            "tags",
            TypeRef::named_nn_list_nn(TypeRef::STRING),
            |ctx| {
                FieldFuture::new(async move {
                    let obj = ctx.parent_value.try_downcast_ref::<GqlValue>()?;
                    Ok(obj_field(obj, "tags"))
                })
            },
        ))
        .field(simple_field("body", TypeRef::named_nn(TypeRef::STRING)))
        .field(simple_field("path", TypeRef::named_nn(TypeRef::STRING)))
        .field(Field::new(
            "fields",
            TypeRef::named_nn_list_nn("InlineField"),
            |ctx| {
                FieldFuture::new(async move {
                    let obj = ctx.parent_value.try_downcast_ref::<GqlValue>()?;
                    Ok(obj_field(obj, "fields"))
                })
            },
        ))
        .field(Field::new(
            "links",
            TypeRef::named_nn_list_nn("Link"),
            |ctx| {
                FieldFuture::new(async move {
                    let obj = ctx.parent_value.try_downcast_ref::<GqlValue>()?;
                    Ok(obj_field(obj, "links"))
                })
            },
        ))
        .field(Field::new(
            "attachments",
            TypeRef::named_nn_list_nn("Attachment"),
            |ctx| {
                FieldFuture::new(async move {
                    let obj = ctx.parent_value.try_downcast_ref::<GqlValue>()?;
                    Ok(obj_field(obj, "attachments"))
                })
            },
        ))
        .field(simple_field("updated_at", TypeRef::named(TypeRef::STRING)))
        .field(simple_field("created_at", TypeRef::named(TypeRef::STRING)))
}

/// Determine the GQL type name for a REFERENCES column's target.
/// Falls back to "Doogat" if the target type isn't in the known set.
pub(super) fn ref_target_gql_type(col: &ColumnDef, known_types: &HashMap<String, String>) -> String {
    let target = col.references.as_deref().unwrap_or("");
    known_types
        .get(target)
        .cloned()
        .unwrap_or_else(|| "Doogat".to_string())
}

/// Extract a string sort key from a `GqlValue::Object` by field name.
fn extract_sort_key(val: &GqlValue, field: &str) -> String {
    match val {
        GqlValue::Object(map) => match map.get(field) {
            Some(GqlValue::String(s)) => s.clone(),
            Some(GqlValue::Number(n)) => format!("{:020}", n.as_f64().unwrap_or(0.0)),
            Some(v) => format!("{v}"),
            None => String::new(),
        },
        _ => String::new(),
    }
}

fn extract_singular_id(parent: &GqlValue, col_name: &str) -> Option<String> {
    let GqlValue::Object(map) = parent else {
        return None;
    };
    match map.get(col_name) {
        Some(GqlValue::String(s)) if !s.is_empty() => Some(s.to_string()),
        _ => None,
    }
}

fn build_singular_ref_field(
    col: &ColumnDef,
    field_name: &str,
    known_types: &HashMap<String, String>,
) -> Field {
    let target_type = ref_target_gql_type(col, known_types);
    let target_ref_name = col.references.clone().unwrap_or_default();
    let col_name = col.name.clone();
    Field::new(field_name, TypeRef::named(&target_type), move |ctx| {
        let col_name = col_name.clone();
        let target_ref_name = target_ref_name.clone();
        FieldFuture::new(async move {
            let parent = ctx.parent_value.try_downcast_ref::<GqlValue>()?;
            let Some(id) = extract_singular_id(parent, &col_name) else {
                return Ok(None);
            };
            let pool = ctx.data::<crate::read_pool::ReadPool>()?;
            let schemas = ctx.data::<TypeSchemaMap>()?;
            let doogat = match pool.get_doogat(id).await {
                Ok(z) => z,
                Err(_) => return Ok(None),
            };
            let val = match schemas.0.get(&target_ref_name) {
                Some(ts) => typed_doogat_to_value(&doogat, ts),
                None => doogat_to_value(&doogat),
            };
            Ok(Some(FieldValue::owned_any(val)))
        })
    })
}

fn extract_ref_ids(parent: &GqlValue, list_name: &str) -> Option<Vec<String>> {
    let GqlValue::Object(map) = parent else {
        return None;
    };
    let GqlValue::List(items) = map.get(list_name)? else {
        return None;
    };
    Some(
        items
            .iter()
            .filter_map(|v| match v {
                GqlValue::String(s) if !s.is_empty() => Some(s.to_string()),
                _ => None,
            })
            .collect(),
    )
}

fn sort_and_limit_values(
    values: &mut Vec<GqlValue>,
    ctx: &async_graphql::dynamic::ResolverContext,
) {
    if let Some(order_field) = ctx.args.get("orderBy").and_then(|v| v.string().ok()) {
        let desc = ctx
            .args
            .get("orderDir")
            .and_then(|v| v.string().ok())
            .map(|d| d.eq_ignore_ascii_case("DESC"))
            .unwrap_or(false);
        values.sort_by(|a, b| {
            let cmp = extract_sort_key(a, order_field).cmp(&extract_sort_key(b, order_field));
            if desc {
                cmp.reverse()
            } else {
                cmp
            }
        });
    }
    if let Some(limit) = ctx.args.get("limit").and_then(|v| v.i64().ok()) {
        values.truncate(limit.max(0) as usize);
    }
}

fn build_plural_ref_field(
    col: &ColumnDef,
    field_name: &str,
    known_types: &HashMap<String, String>,
) -> Field {
    let target_type = ref_target_gql_type(col, known_types);
    let target_ref_name = col.references.clone().unwrap_or_default();
    let plural_desc = format!("All referenced {} doogats.", target_ref_name);
    let data_list_name = pluralize(&col.name);
    Field::new(
        field_name,
        TypeRef::named_nn_list_nn(&target_type),
        move |ctx| {
            let data_list_name = data_list_name.clone();
            let target_ref_name = target_ref_name.clone();
            FieldFuture::new(async move {
                let parent = ctx.parent_value.try_downcast_ref::<GqlValue>()?;
                let empty = || Ok(Some(FieldValue::list(std::iter::empty::<FieldValue>())));
                let ids = match extract_ref_ids(parent, &data_list_name) {
                    Some(ids) if !ids.is_empty() => ids,
                    _ => return empty(),
                };
                let pool = ctx.data::<crate::read_pool::ReadPool>()?;
                let schemas = ctx.data::<TypeSchemaMap>()?;
                let target_schema = schemas.0.get(&target_ref_name);
                let doogats = pool
                    .get_doogats_batch(ids)
                    .await
                    .map_err(crate::error::to_graphql_error)?;
                let mut values: Vec<GqlValue> = doogats
                    .iter()
                    .map(|z| match target_schema {
                        Some(ts) => typed_doogat_to_value(z, ts),
                        None => doogat_to_value(z),
                    })
                    .collect();
                sort_and_limit_values(&mut values, &ctx);
                let resolved: Vec<FieldValue> =
                    values.into_iter().map(FieldValue::owned_any).collect();
                Ok(Some(FieldValue::list(resolved)))
            })
        },
    )
    .argument(InputValue::new("limit", TypeRef::named(TypeRef::INT)))
    .argument(InputValue::new("orderBy", TypeRef::named(TypeRef::STRING)))
    .argument(InputValue::new("orderDir", TypeRef::named(TypeRef::STRING)))
    .description(&plural_desc)
}

fn build_raw_ref_field(col_name: &str, field_name: &str) -> Field {
    let col_name = col_name.to_string();
    Field::new(field_name, TypeRef::named(TypeRef::STRING), move |ctx| {
        let col_name = col_name.clone();
        FieldFuture::new(async move {
            let obj = ctx.parent_value.try_downcast_ref::<GqlValue>()?;
            Ok(obj_field(obj, &col_name))
        })
    })
}

fn add_reference_fields(
    obj: Object,
    col: &ColumnDef,
    known_types: &HashMap<String, String>,
) -> Object {
    let gql_col_name = sanitize_field_name(&col.name);
    let stripped = strip_id_suffix(&col.name);
    let has_id_suffix = stripped.len() < col.name.len();

    let obj_field_name = if has_id_suffix {
        sanitize_field_name(stripped)
    } else {
        gql_col_name
    };
    let raw_field_name = if has_id_suffix {
        sanitize_field_name(&col.name)
    } else {
        sanitize_field_name(&format!("{}_id", col.name))
    };

    let mut obj = obj;
    if !BASE_DOOGAT_FIELDS.contains(&obj_field_name.as_str()) {
        obj = obj.field(build_singular_ref_field(col, &obj_field_name, known_types));
    }

    let plural_base = if has_id_suffix { stripped } else { &col.name };
    let list_name = pluralize(plural_base);
    let gql_list_name = if is_valid_graphql_name(&list_name) {
        list_name
    } else {
        pluralize_preserving_case(&sanitize_field_name(plural_base))
    };
    if !BASE_DOOGAT_FIELDS.contains(&gql_list_name.as_str()) {
        obj = obj.field(build_plural_ref_field(col, &gql_list_name, known_types));
    }

    obj.field(build_raw_ref_field(&col.name, &raw_field_name))
}

/// Build a dynamic GraphQL object type for a _typedef schema.
pub(crate) fn build_typed_object(
    type_name: &str,
    schema: &TableSchema,
    known_types: &HashMap<String, String>,
) -> Object {
    let mut obj = doogat_object(type_name);

    for col in &schema.columns {
        if col.references.is_some() {
            obj = add_reference_fields(obj, col, known_types);
            continue;
        }
        let gql_col_name = sanitize_field_name(&col.name);
        if !BASE_DOOGAT_FIELDS.contains(&gql_col_name.as_str()) {
            let gql_type = column_to_gql_type(col);
            let col_name = col.name.clone();
            obj = obj.field(Field::new(&gql_col_name, gql_type, move |ctx| {
                let col_name = col_name.clone();
                FieldFuture::new(async move {
                    let obj = ctx.parent_value.try_downcast_ref::<GqlValue>()?;
                    Ok(obj_field(obj, &col_name))
                })
            }));
        } else if !ddb_core::indexer::is_core_column(&col.name) {
            // Core columns (id, title, type, date, updated_at) can now be
            // declared with constraints (NOT NULL, VARCHAR(N)) so the SQL
            // validator enforces them — see PRD 00122. They live in
            // `schema.columns` but are exposed via the base doogat fields
            // above, not as new typed-object fields. Silently skip; warn
            // only for non-core base-field collisions like `body`, `tags`,
            // `links`, etc., where the user almost certainly named the
            // column by mistake.
            tracing::warn!(
                typedef = %type_name,
                column = %col.name,
                gql_name = %gql_col_name,
                "typedef column name collides with a base doogat field and was dropped from the typed GraphQL object; rename the column to expose it"
            );
        }
    }

    obj
}

pub(crate) fn column_to_gql_type(col: &ColumnDef) -> TypeRef {
    match col.data_type.to_uppercase().as_str() {
        "BOOLEAN" => TypeRef::named(TypeRef::BOOLEAN),
        "INTEGER" => TypeRef::named(TypeRef::INT),
        "REAL" => TypeRef::named(TypeRef::FLOAT),
        _ => TypeRef::named(TypeRef::STRING),
    }
}
