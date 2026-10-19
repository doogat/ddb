use async_graphql::dynamic::{Field, FieldFuture, FieldValue, Object, TypeRef};
use async_graphql::{Name, Value as GqlValue};
use ddb_core::types::TableSchema;
use indexmap::IndexMap;

use crate::schema::sanitize_type_name;

use super::where_build::WhereClause;

// -- Connection type --

/// Generate a `{TypeName}Connection` object type with `items` and `totalCount`.
pub fn build_connection_type(type_name: &str) -> Object {
    let item_type = type_name.to_string();
    Object::new(format!("{type_name}Connection"))
        .field(Field::new(
            "items",
            TypeRef::named_nn_list_nn(&item_type),
            |ctx| {
                FieldFuture::new(async move {
                    let parent = ctx.parent_value.try_downcast_ref::<GqlValue>()?;
                    if let GqlValue::Object(map) = parent {
                        if let Some(GqlValue::List(items)) = map.get("items") {
                            return Ok(Some(FieldValue::list(
                                items.iter().map(|v| FieldValue::owned_any(v.clone())),
                            )));
                        }
                    }
                    Ok(Some(FieldValue::list(std::iter::empty::<FieldValue>())))
                })
            },
        ))
        .field(Field::new(
            "totalCount",
            TypeRef::named_nn(TypeRef::INT),
            |ctx| {
                FieldFuture::new(async move {
                    let parent = ctx.parent_value.try_downcast_ref::<GqlValue>()?;
                    if let GqlValue::Object(map) = parent {
                        if let Some(v) = map.get("totalCount") {
                            return Ok(Some(FieldValue::value(v.clone())));
                        }
                    }
                    Ok(Some(FieldValue::value(GqlValue::from(0))))
                })
            },
        ))
}

// -- Aggregation --

/// Check if a column type is numeric.
fn is_numeric(data_type: &str) -> bool {
    matches!(data_type.to_uppercase().as_str(), "INTEGER" | "REAL")
}

/// Generate a `{TypeName}Aggregate` object type.
///
/// Always has `count: Int!`. For each numeric column, adds
/// `min{Col}`, `max{Col}`, `sum{Col}`, `avg{Col}` as nullable Float fields.
fn add_aggregate_fields(obj: Object, schema: &TableSchema) -> Object {
    // count: Int!
    let mut obj = obj.field(Field::new(
        "count",
        TypeRef::named_nn(TypeRef::INT),
        |ctx| {
            FieldFuture::new(async move {
                let parent = ctx.parent_value.try_downcast_ref::<GqlValue>()?;
                if let GqlValue::Object(map) = parent {
                    if let Some(v) = map.get("count") {
                        return Ok(Some(FieldValue::value(v.clone())));
                    }
                }
                Ok(Some(FieldValue::value(GqlValue::from(0))))
            })
        },
    ));

    // Numeric aggregate fields
    for col in &schema.columns {
        if !is_numeric(&col.data_type) {
            continue;
        }
        let cap = sanitize_type_name(&col.name);
        for prefix in ["min", "max", "sum", "avg"] {
            let field_name = format!("{prefix}{cap}");
            let key = field_name.clone();
            obj = obj.field(Field::new(
                &field_name,
                TypeRef::named(TypeRef::FLOAT),
                move |ctx| {
                    let key = key.clone();
                    FieldFuture::new(async move {
                        let parent = ctx.parent_value.try_downcast_ref::<GqlValue>()?;
                        if let GqlValue::Object(map) = parent {
                            if let Some(v) = map.get(key.as_str()) {
                                return Ok(Some(FieldValue::value(v.clone())));
                            }
                        }
                        Ok(None)
                    })
                },
            ));
        }
    }

    obj
}

pub fn build_aggregate_group_type(type_name: &str, schema: &TableSchema) -> Object {
    let group_name = format!("{type_name}AggregateGroup");
    let mut obj = Object::new(&group_name);

    // key: String!
    obj = obj.field(Field::new(
        "key",
        TypeRef::named_nn(TypeRef::STRING),
        |ctx| {
            FieldFuture::new(async move {
                let parent = ctx.parent_value.try_downcast_ref::<GqlValue>()?;
                if let GqlValue::Object(map) = parent {
                    if let Some(v) = map.get("key") {
                        return Ok(Some(FieldValue::value(v.clone())));
                    }
                }
                Ok(Some(FieldValue::value(GqlValue::from(""))))
            })
        },
    ));

    add_aggregate_fields(obj, schema)
}

pub fn build_aggregate_type(type_name: &str, schema: &TableSchema) -> Object {
    let group_type_name = format!("{type_name}AggregateGroup");
    let obj = Object::new(format!("{type_name}Aggregate"));
    let obj = add_aggregate_fields(obj, schema);

    // groups: [{Type}AggregateGroup!]
    obj.field(Field::new(
        "groups",
        TypeRef::named_list_nn(&group_type_name),
        |ctx| {
            FieldFuture::new(async move {
                let parent = ctx.parent_value.try_downcast_ref::<GqlValue>()?;
                if let GqlValue::Object(map) = parent {
                    if let Some(GqlValue::List(items)) = map.get("groups") {
                        return Ok(Some(FieldValue::list(
                            items.iter().map(|item| FieldValue::owned_any(item.clone())),
                        )));
                    }
                }
                Ok(None)
            })
        },
    ))
}

/// Build the SQL for an aggregate query on a materialized table.
///
/// Returns (sql, column_names) where column_names maps positionally
/// to the GraphQL aggregate field names.
pub fn build_aggregate_sql(
    table_name: &str,
    schema: &TableSchema,
    where_clause: &WhereClause,
) -> (String, Vec<String>) {
    build_aggregate_sql_grouped(table_name, schema, where_clause, None)
}

pub fn build_aggregate_sql_grouped(
    table_name: &str,
    schema: &TableSchema,
    where_clause: &WhereClause,
    group_by: Option<&str>,
) -> (String, Vec<String>) {
    let (selects, names) = build_aggregate_selects(schema, group_by);

    let where_part = if where_clause.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", where_clause.sql)
    };

    let group_part = match group_by {
        Some(col) => {
            let escaped = col.replace('"', "\"\"");
            format!(" GROUP BY \"{escaped}\"")
        }
        None => String::new(),
    };

    let sql = format!(
        "SELECT {} FROM \"{table_name}\"{where_part}{group_part}",
        selects.join(", ")
    );

    (sql, names)
}

/// Build SELECT columns for an aggregate query (COUNT, MIN, MAX, SUM, AVG).
fn build_aggregate_selects(
    schema: &TableSchema,
    group_by: Option<&str>,
) -> (Vec<String>, Vec<String>) {
    let mut selects = Vec::new();
    let mut names = Vec::new();

    if let Some(col) = group_by {
        let escaped = col.replace('"', "\"\"");
        selects.push(format!("\"{escaped}\" AS \"key\""));
        names.push("key".to_string());
    }

    selects.push("COUNT(*) AS count".to_string());
    names.push("count".to_string());

    for col in &schema.columns {
        if !is_numeric(&col.data_type) {
            continue;
        }
        let cap = sanitize_type_name(&col.name);
        let c = &col.name;
        for (func, prefix) in [
            ("MIN", "min"),
            ("MAX", "max"),
            ("SUM", "sum"),
            ("AVG", "avg"),
        ] {
            let alias = format!("{prefix}{cap}");
            selects.push(format!("{func}(\"{c}\") AS \"{alias}\""));
            names.push(alias);
        }
    }

    (selects, names)
}

/// Convert an aggregate query row into a GqlValue object.
pub fn aggregate_row_to_value(row: &[String], names: &[String]) -> GqlValue {
    let mut map = IndexMap::new();
    for (i, name) in names.iter().enumerate() {
        let val = row.get(i).map(|s| s.as_str()).unwrap_or("NULL");
        let gql_val = if val == "NULL" {
            GqlValue::Null
        } else if name == "count" {
            GqlValue::from(val.parse::<i64>().unwrap_or(0))
        } else if name == "key" {
            GqlValue::from(val)
        } else {
            GqlValue::from(val.parse::<f64>().unwrap_or(0.0))
        };
        map.insert(Name::new(name), gql_val);
    }
    GqlValue::Object(map)
}
