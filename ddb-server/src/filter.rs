use async_graphql::dynamic::{
    Enum, EnumItem, Field, FieldFuture, FieldValue, InputObject, InputValue, Object, TypeRef,
};
use async_graphql::{Name, Value as GqlValue};
use indexmap::IndexMap;
use rusqlite::types::Value as SqlValue;
use ddb_core::types::TableSchema;

use crate::schema::{resolve_column, sanitize_field_name, sanitize_type_name};

// -- Shared scalar filter input types --

/// `StringFilter` — equality, pattern matching, set membership on TEXT columns.
pub fn string_filter() -> InputObject {
    InputObject::new("StringFilter")
        .field(InputValue::new("eq", TypeRef::named(TypeRef::STRING)))
        .field(InputValue::new("neq", TypeRef::named(TypeRef::STRING)))
        .field(InputValue::new("contains", TypeRef::named(TypeRef::STRING)))
        .field(InputValue::new(
            "startsWith",
            TypeRef::named(TypeRef::STRING),
        ))
        .field(InputValue::new("in", TypeRef::named_list(TypeRef::STRING)))
}

/// `IntFilter` — equality, comparison, set membership on INTEGER columns.
pub fn int_filter() -> InputObject {
    InputObject::new("IntFilter")
        .field(InputValue::new("eq", TypeRef::named(TypeRef::INT)))
        .field(InputValue::new("neq", TypeRef::named(TypeRef::INT)))
        .field(InputValue::new("gt", TypeRef::named(TypeRef::INT)))
        .field(InputValue::new("gte", TypeRef::named(TypeRef::INT)))
        .field(InputValue::new("lt", TypeRef::named(TypeRef::INT)))
        .field(InputValue::new("lte", TypeRef::named(TypeRef::INT)))
        .field(InputValue::new("in", TypeRef::named_list(TypeRef::INT)))
}

/// `FloatFilter` — equality, comparison, set membership on REAL columns.
pub fn float_filter() -> InputObject {
    InputObject::new("FloatFilter")
        .field(InputValue::new("eq", TypeRef::named(TypeRef::FLOAT)))
        .field(InputValue::new("neq", TypeRef::named(TypeRef::FLOAT)))
        .field(InputValue::new("gt", TypeRef::named(TypeRef::FLOAT)))
        .field(InputValue::new("gte", TypeRef::named(TypeRef::FLOAT)))
        .field(InputValue::new("lt", TypeRef::named(TypeRef::FLOAT)))
        .field(InputValue::new("lte", TypeRef::named(TypeRef::FLOAT)))
        .field(InputValue::new("in", TypeRef::named_list(TypeRef::FLOAT)))
}

/// `BoolFilter` — equality on BOOLEAN columns.
pub fn bool_filter() -> InputObject {
    InputObject::new("BoolFilter").field(InputValue::new("eq", TypeRef::named(TypeRef::BOOLEAN)))
}

/// `IDFilter` — equality and set membership on ID/reference columns.
pub fn id_filter() -> InputObject {
    InputObject::new("IDFilter")
        .field(InputValue::new("eq", TypeRef::named(TypeRef::ID)))
        .field(InputValue::new("in", TypeRef::named_list(TypeRef::ID)))
}

/// `TagsFilter` — match the tag set on the doogat-level `_ddb_tags`
/// index. Available on every per-type query's `where` input. PRD 00129
/// §5. The filter composes with column filters via the same conjunction
/// the rest of the where clause uses; combinator semantics:
/// - `contains`: row has the named tag.
/// - `containsAll`: row has every listed tag (one EXISTS per tag,
///   AND-ed).
/// - `containsAny`: row has at least one of the listed tags.
pub fn tags_filter() -> InputObject {
    InputObject::new("TagsFilter")
        .description(
            "Filter doogats by tag set. `contains` matches a single tag, `containsAll` matches \
             rows that carry every listed tag, `containsAny` matches rows with at least one \
             listed tag. Empty `containsAll` / `containsAny` match nothing (mirrors empty `in`).",
        )
        .field(InputValue::new("contains", TypeRef::named(TypeRef::STRING)))
        .field(InputValue::new(
            "containsAll",
            TypeRef::named_nn_list_nn(TypeRef::STRING),
        ))
        .field(InputValue::new(
            "containsAny",
            TypeRef::named_nn_list_nn(TypeRef::STRING),
        ))
}

/// Returns the GraphQL type name for the filter matching a column's data type.
pub fn filter_type_for_column(col: &ddb_core::types::ColumnDef) -> &'static str {
    if col.references.is_some() {
        return "IDFilter";
    }
    match col.data_type.to_uppercase().as_str() {
        "INTEGER" => "IntFilter",
        "REAL" => "FloatFilter",
        "BOOLEAN" => "BoolFilter",
        _ => "StringFilter",
    }
}

// -- Per-type Where input generation --

/// Generate a `{TypeName}Where` input type from a `TableSchema`.
///
/// Each column gets a field typed to the matching scalar filter.
/// `_and` and `_or` are self-referencing list fields for compound logic.
pub fn build_where_input(type_name: &str, schema: &TableSchema) -> InputObject {
    let name = format!("{type_name}Where");
    let mut input = InputObject::new(&name);

    for col in &schema.columns {
        let gql_name = sanitize_field_name(&col.name);
        let filter_type = filter_type_for_column(col);
        input = input.field(InputValue::new(&gql_name, TypeRef::named(filter_type)));
    }

    // Base doogat fields (always present in materialized tables)
    let sanitized: Vec<String> = schema.columns.iter().map(|c| sanitize_field_name(&c.name)).collect();
    if !sanitized.iter().any(|n| n == "id") {
        input = input.field(InputValue::new("id", TypeRef::named("IDFilter")));
    }
    if !sanitized.iter().any(|n| n == "title") {
        input = input.field(InputValue::new("title", TypeRef::named("StringFilter")));
    }

    // PRD 00129 §5: every per-type Where input exposes tag filtering
    // backed by the doogat-level `_ddb_tags` table. Hidden when a typedef
    // declares its own `tags` column to avoid a name collision.
    if !sanitized.iter().any(|n| n == "tags") {
        input = input.field(InputValue::new("tags", TypeRef::named("TagsFilter")));
    }

    // Compound combinators (self-referencing)
    input = input.field(InputValue::new("_and", TypeRef::named_list(&name)));
    input = input.field(InputValue::new("_or", TypeRef::named_list(&name)));

    input
}

// -- Sorting types --

/// `SortOrder` GraphQL enum — ASC or DESC.
pub fn sort_order_enum() -> Enum {
    Enum::new("SortOrder")
        .item(EnumItem::new("ASC"))
        .item(EnumItem::new("DESC"))
}

/// Generate a `{TypeName}OrderBy` input type from a `TableSchema`.
pub fn build_order_by_input(type_name: &str, schema: &TableSchema) -> InputObject {
    let mut input = InputObject::new(format!("{type_name}OrderBy"));

    for col in &schema.columns {
        let gql_name = sanitize_field_name(&col.name);
        input = input.field(InputValue::new(&gql_name, TypeRef::named("SortOrder")));
    }

    input
}

/// Build an ORDER BY clause from a GraphQL orderBy input value.
///
/// Returns the clause contents without the `ORDER BY` prefix
/// (e.g. `"title" ASC, "priority" DESC`). Returns `None` when the
/// input is empty/null, so the caller can fall back to a default sort.
pub fn build_order_sql(input: &GqlValue, schema: &TableSchema) -> Option<String> {
    let obj = match input {
        GqlValue::Object(obj) if !obj.is_empty() => obj,
        _ => return None,
    };

    let mut parts = Vec::new();
    for (name, value) in obj {
        let Some(col) = resolve_column(&schema.columns, name.as_str()) else {
            continue;
        };
        let dir = match value {
            GqlValue::Enum(e) => e.as_str(),
            GqlValue::String(s) => s.as_str(),
            _ => continue,
        };
        match dir {
            "ASC" | "DESC" => parts.push(format!("\"{col}\" {dir}")),
            _ => continue,
        }
    }

    if parts.is_empty() {
        None
    } else {
        Some(parts.join(", "))
    }
}

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
        for (func, prefix) in [("MIN", "min"), ("MAX", "max"), ("SUM", "sum"), ("AVG", "avg")] {
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

// -- WHERE clause builder --

/// Parameterized SQL WHERE clause.
pub struct WhereClause {
    pub sql: String,
    pub params: Vec<SqlValue>,
}

impl WhereClause {
    pub fn empty() -> Self {
        Self {
            sql: String::new(),
            params: Vec::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.sql.is_empty()
    }
}

/// Base doogat fields present in every materialized table but not in `schema.columns`.
const BASE_FILTER_FIELDS: &[&str] = &["id", "title"];

/// Build a parameterized WHERE clause from a GraphQL filter input value.
///
/// The `input` should be the resolved `{Type}Where` object value.
/// Column names are validated against the schema to prevent injection.
pub fn build_where_sql(input: &GqlValue, schema: &TableSchema) -> WhereClause {
    let mut conditions = Vec::new();
    let mut params = Vec::new();

    let obj = match input {
        GqlValue::Object(obj) => obj,
        _ => return WhereClause::empty(),
    };

    for (name, value) in obj {
        let field = name.as_str();
        match field {
            "_and" | "_or" => {
                let combinator = if field == "_and" { "AND" } else { "OR" };
                let GqlValue::List(items) = value else { continue };
                if let Some(cond) =
                    build_logical_combinator(items, schema, combinator, &mut params)
                {
                    conditions.push(cond);
                }
            }
            // PRD 00129 §5: `tags` is a synthetic field backed by the
            // doogat-level `_ddb_tags` index, not a column on the
            // materialized type table. It only fires when the typedef
            // doesn't have its own `tags` column (the where-input
            // generator hides `TagsFilter` in that case so this branch
            // only runs for the synthetic path).
            "tags" if !schema.columns.iter().any(|c| c.name == "tags") => {
                if let Some(cond) =
                    build_tags_filter_condition(value, &schema.table_name, &mut params)
                {
                    conditions.push(cond);
                }
            }
            _ => build_column_conditions(field, value, schema, &mut conditions, &mut params),
        }
    }

    if conditions.is_empty() {
        WhereClause::empty()
    } else {
        WhereClause {
            sql: conditions.join(" AND "),
            params,
        }
    }
}

/// PRD 00129 §5: build the SQL fragment for a `TagsFilter` value.
///
/// Returns the EXISTS / aggregate-COUNT clause that joins the
/// materialized type table against the doogat-level `_ddb_tags` index.
/// All values are bound parameters (never interpolated). Empty lists
/// produce an always-false condition (matches no rows) to mirror the
/// `in: []` semantics elsewhere in the where input.
fn build_tags_filter_condition(
    value: &GqlValue,
    table_name: &str,
    params: &mut Vec<SqlValue>,
) -> Option<String> {
    let GqlValue::Object(filter) = value else {
        return None;
    };
    let mut clauses = Vec::new();
    for (op, val) in filter {
        let op_str = op.as_str();
        match op_str {
            "contains" => {
                let GqlValue::String(tag) = val else { continue };
                params.push(SqlValue::Text(tag.clone()));
                clauses.push(format!(
                    "EXISTS (SELECT 1 FROM _ddb_tags _t WHERE _t.doogat_id = \"{table}\".id AND _t.tag = ?)",
                    table = table_name,
                ));
            }
            "containsAll" => {
                let GqlValue::List(items) = val else { continue };
                if items.is_empty() {
                    clauses.push("0 = 1".to_string());
                    continue;
                }
                for item in items {
                    let GqlValue::String(tag) = item else { continue };
                    params.push(SqlValue::Text(tag.clone()));
                    clauses.push(format!(
                        "EXISTS (SELECT 1 FROM _ddb_tags _t WHERE _t.doogat_id = \"{table}\".id AND _t.tag = ?)",
                        table = table_name,
                    ));
                }
            }
            "containsAny" => {
                let GqlValue::List(items) = val else { continue };
                if items.is_empty() {
                    clauses.push("0 = 1".to_string());
                    continue;
                }
                let mut placeholders = Vec::with_capacity(items.len());
                for item in items {
                    let GqlValue::String(tag) = item else { continue };
                    params.push(SqlValue::Text(tag.clone()));
                    placeholders.push("?".to_string());
                }
                if placeholders.is_empty() {
                    continue;
                }
                clauses.push(format!(
                    "EXISTS (SELECT 1 FROM _ddb_tags _t WHERE _t.doogat_id = \"{table}\".id AND _t.tag IN ({tags}))",
                    table = table_name,
                    tags = placeholders.join(", "),
                ));
            }
            _ => {}
        }
    }
    if clauses.is_empty() {
        None
    } else {
        Some(format!("({})", clauses.join(" AND ")))
    }
}

/// Resolve a column name and apply its operator conditions.
fn build_column_conditions(
    field: &str,
    value: &GqlValue,
    schema: &TableSchema,
    conditions: &mut Vec<String>,
    params: &mut Vec<SqlValue>,
) {
    let col_name = resolve_column(&schema.columns, field)
        .or_else(|| BASE_FILTER_FIELDS.iter().find(|&&f| f == field).copied());
    let Some(col_name) = col_name else { return };
    let GqlValue::Object(filter_obj) = value else { return };
    for (op, val) in filter_obj {
        if let Some(cond) = build_operator_condition(col_name, op.as_str(), val, params) {
            conditions.push(cond);
        }
    }
}

/// Recursively build a compound AND/OR clause from a list of filter items.
fn build_logical_combinator(
    items: &[GqlValue],
    schema: &TableSchema,
    combinator: &str,
    params: &mut Vec<SqlValue>,
) -> Option<String> {
    let sub: Vec<String> = items
        .iter()
        .filter_map(|item| {
            let wc = build_where_sql(item, schema);
            if wc.is_empty() {
                None
            } else {
                params.extend(wc.params);
                Some(format!("({})", wc.sql))
            }
        })
        .collect();
    if sub.is_empty() {
        None
    } else {
        Some(format!("({})", sub.join(&format!(" {combinator} "))))
    }
}

/// Translate a single filter operator (eq, neq, contains, etc.) into SQL.
fn build_operator_condition(
    column: &str,
    op: &str,
    value: &GqlValue,
    params: &mut Vec<SqlValue>,
) -> Option<String> {
    match op {
        "eq" => {
            params.push(gql_to_sql(value));
            Some(format!("\"{column}\" = ?"))
        }
        "neq" => {
            params.push(gql_to_sql(value));
            Some(format!("\"{column}\" != ?"))
        }
        "gt" => {
            params.push(gql_to_sql(value));
            Some(format!("\"{column}\" > ?"))
        }
        "gte" => {
            params.push(gql_to_sql(value));
            Some(format!("\"{column}\" >= ?"))
        }
        "lt" => {
            params.push(gql_to_sql(value));
            Some(format!("\"{column}\" < ?"))
        }
        "lte" => {
            params.push(gql_to_sql(value));
            Some(format!("\"{column}\" <= ?"))
        }
        "contains" => {
            params.push(gql_to_sql(value));
            Some(format!("\"{column}\" LIKE '%' || ? || '%' COLLATE NOCASE"))
        }
        "startsWith" => {
            params.push(gql_to_sql(value));
            Some(format!("\"{column}\" LIKE ? || '%' COLLATE NOCASE"))
        }
        "in" => build_in_condition(column, value, params),
        _ => None,
    }
}

fn build_in_condition(
    column: &str,
    value: &GqlValue,
    params: &mut Vec<SqlValue>,
) -> Option<String> {
    let items = match value {
        GqlValue::List(items) => items,
        _ => return None,
    };
    if items.is_empty() {
        return Some("0".to_string()); // IN () is invalid; always-false
    }
    let placeholders: Vec<&str> = items
        .iter()
        .map(|v| {
            params.push(gql_to_sql(v));
            "?"
        })
        .collect();
    Some(format!("\"{}\" IN ({})", column, placeholders.join(", ")))
}

/// Convert a GraphQL value to a rusqlite parameter value.
fn gql_to_sql(value: &GqlValue) -> SqlValue {
    match value {
        GqlValue::Number(n) => {
            if let Some(i) = n.as_i64() {
                SqlValue::Integer(i)
            } else if let Some(f) = n.as_f64() {
                SqlValue::Real(f)
            } else {
                SqlValue::Text(n.to_string())
            }
        }
        GqlValue::String(s) => SqlValue::Text(s.clone()),
        GqlValue::Boolean(b) => SqlValue::Integer(if *b { 1 } else { 0 }),
        GqlValue::Null => SqlValue::Null,
        // Enum values (SortOrder etc.) come as Name strings
        GqlValue::Enum(name) => SqlValue::Text(name.to_string()),
        _ => SqlValue::Text(value.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_graphql::{Name, Value as GqlValue};
    use indexmap::IndexMap;
    use ddb_core::types::{ColumnDef, TableSchema};

    fn test_schema() -> TableSchema {
        TableSchema {
            table_name: "bookmark".to_string(),
            columns: vec![
                ColumnDef {
                    name: "title".to_string(),
                    data_type: "TEXT".to_string(),
                    references: None,
                    zone: None,
                    required: false,
                    search_boost: None,
                    allowed_values: None,
                    default_value: None,
                    on_delete: ddb_core::types::OnDeleteAction::Restrict,
                },
                ColumnDef {
                    name: "priority".to_string(),
                    data_type: "INTEGER".to_string(),
                    references: None,
                    zone: None,
                    required: false,
                    search_boost: None,
                    allowed_values: None,
                    default_value: None,
                    on_delete: ddb_core::types::OnDeleteAction::Restrict,
                },
                ColumnDef {
                    name: "status".to_string(),
                    data_type: "TEXT".to_string(),
                    references: None,
                    zone: None,
                    required: false,
                    search_boost: None,
                    allowed_values: None,
                    default_value: None,
                    on_delete: ddb_core::types::OnDeleteAction::Restrict,
                },
                ColumnDef {
                    name: "category".to_string(),
                    data_type: "TEXT".to_string(),
                    references: Some("category".to_string()),
                    zone: None,
                    required: false,
                    search_boost: None,
                    allowed_values: None,
                    default_value: None,
                    on_delete: ddb_core::types::OnDeleteAction::Restrict,
                },
            ],
            crdt_strategy: None,
            template_sections: Vec::new(),
            folder: false,
            stale_after_days: None,
            title_template: None,
            origin: None,
            unique_together: None,
        }
    }

    /// Helper: build a filter object { field: { op: value } }
    fn filter(field: &str, op: &str, val: GqlValue) -> GqlValue {
        let mut filter_obj = IndexMap::new();
        filter_obj.insert(Name::new(op), val);
        let mut obj = IndexMap::new();
        obj.insert(Name::new(field), GqlValue::Object(filter_obj));
        GqlValue::Object(obj)
    }

    #[test]
    fn test_string_eq() {
        let input = filter("title", "eq", GqlValue::String("rust".into()));
        let wc = build_where_sql(&input, &test_schema());
        assert_eq!(wc.sql, r#""title" = ?"#);
        assert_eq!(wc.params, vec![SqlValue::Text("rust".into())]);
    }

    #[test]
    fn test_string_neq() {
        let input = filter("title", "neq", GqlValue::String("java".into()));
        let wc = build_where_sql(&input, &test_schema());
        assert_eq!(wc.sql, r#""title" != ?"#);
        assert_eq!(wc.params, vec![SqlValue::Text("java".into())]);
    }

    #[test]
    fn test_string_contains() {
        let input = filter("title", "contains", GqlValue::String("rust".into()));
        let wc = build_where_sql(&input, &test_schema());
        assert_eq!(wc.sql, r#""title" LIKE '%' || ? || '%' COLLATE NOCASE"#);
        assert_eq!(wc.params, vec![SqlValue::Text("rust".into())]);
    }

    #[test]
    fn test_string_starts_with() {
        let input = filter("title", "startsWith", GqlValue::String("Hello".into()));
        let wc = build_where_sql(&input, &test_schema());
        assert_eq!(wc.sql, r#""title" LIKE ? || '%' COLLATE NOCASE"#);
        assert_eq!(wc.params, vec![SqlValue::Text("Hello".into())]);
    }

    #[test]
    fn test_int_comparison() {
        for (op, sql_op) in [("gt", ">"), ("gte", ">="), ("lt", "<"), ("lte", "<=")] {
            let input = filter("priority", op, GqlValue::Number(3.into()));
            let wc = build_where_sql(&input, &test_schema());
            assert_eq!(wc.sql, format!("\"priority\" {sql_op} ?"));
            assert_eq!(wc.params, vec![SqlValue::Integer(3)]);
        }
    }

    #[test]
    fn test_in_filter() {
        let list = GqlValue::List(vec![
            GqlValue::String("a".into()),
            GqlValue::String("b".into()),
            GqlValue::String("c".into()),
        ]);
        let input = filter("status", "in", list);
        let wc = build_where_sql(&input, &test_schema());
        assert_eq!(wc.sql, r#""status" IN (?, ?, ?)"#);
        assert_eq!(wc.params.len(), 3);
    }

    #[test]
    fn test_empty_in_filter() {
        let input = filter("status", "in", GqlValue::List(vec![]));
        let wc = build_where_sql(&input, &test_schema());
        assert_eq!(wc.sql, "0"); // always-false
    }

    #[test]
    fn test_compound_and() {
        let mut and_item1 = IndexMap::new();
        let mut f1 = IndexMap::new();
        f1.insert(Name::new("eq"), GqlValue::String("done".into()));
        and_item1.insert(Name::new("status"), GqlValue::Object(f1));

        let mut and_item2 = IndexMap::new();
        let mut f2 = IndexMap::new();
        f2.insert(Name::new("gte"), GqlValue::Number(3.into()));
        and_item2.insert(Name::new("priority"), GqlValue::Object(f2));

        let mut obj = IndexMap::new();
        obj.insert(
            Name::new("_and"),
            GqlValue::List(vec![
                GqlValue::Object(and_item1),
                GqlValue::Object(and_item2),
            ]),
        );
        let input = GqlValue::Object(obj);
        let wc = build_where_sql(&input, &test_schema());
        assert_eq!(wc.sql, r#"(("status" = ?) AND ("priority" >= ?))"#);
        assert_eq!(
            wc.params,
            vec![SqlValue::Text("done".into()), SqlValue::Integer(3)]
        );
    }

    #[test]
    fn test_compound_or() {
        let mut or_item1 = IndexMap::new();
        let mut f1 = IndexMap::new();
        f1.insert(Name::new("eq"), GqlValue::String("todo".into()));
        or_item1.insert(Name::new("status"), GqlValue::Object(f1));

        let mut or_item2 = IndexMap::new();
        let mut f2 = IndexMap::new();
        f2.insert(Name::new("eq"), GqlValue::String("doing".into()));
        or_item2.insert(Name::new("status"), GqlValue::Object(f2));

        let mut obj = IndexMap::new();
        obj.insert(
            Name::new("_or"),
            GqlValue::List(vec![GqlValue::Object(or_item1), GqlValue::Object(or_item2)]),
        );
        let input = GqlValue::Object(obj);
        let wc = build_where_sql(&input, &test_schema());
        assert_eq!(wc.sql, r#"(("status" = ?) OR ("status" = ?))"#);
        assert_eq!(
            wc.params,
            vec![
                SqlValue::Text("todo".into()),
                SqlValue::Text("doing".into())
            ]
        );
    }

    #[test]
    fn test_nested_compound() {
        // _and containing _or: _and: [{ _or: [status=todo, status=doing] }, { priority >= 3 }]
        let mut or1 = IndexMap::new();
        let mut f1 = IndexMap::new();
        f1.insert(Name::new("eq"), GqlValue::String("todo".into()));
        or1.insert(Name::new("status"), GqlValue::Object(f1));

        let mut or2 = IndexMap::new();
        let mut f2 = IndexMap::new();
        f2.insert(Name::new("eq"), GqlValue::String("doing".into()));
        or2.insert(Name::new("status"), GqlValue::Object(f2));

        let mut or_wrapper = IndexMap::new();
        or_wrapper.insert(
            Name::new("_or"),
            GqlValue::List(vec![GqlValue::Object(or1), GqlValue::Object(or2)]),
        );

        let mut prio = IndexMap::new();
        let mut f3 = IndexMap::new();
        f3.insert(Name::new("gte"), GqlValue::Number(3.into()));
        prio.insert(Name::new("priority"), GqlValue::Object(f3));

        let mut obj = IndexMap::new();
        obj.insert(
            Name::new("_and"),
            GqlValue::List(vec![GqlValue::Object(or_wrapper), GqlValue::Object(prio)]),
        );
        let input = GqlValue::Object(obj);
        let wc = build_where_sql(&input, &test_schema());
        assert_eq!(
            wc.sql,
            r#"(((("status" = ?) OR ("status" = ?))) AND ("priority" >= ?))"#
        );
        assert_eq!(wc.params.len(), 3);
    }

    #[test]
    fn test_empty_where() {
        let input = GqlValue::Object(IndexMap::new());
        let wc = build_where_sql(&input, &test_schema());
        assert!(wc.is_empty());
        assert!(wc.params.is_empty());
    }

    #[test]
    fn test_params_are_parameterized() {
        // Values with SQL injection attempts should never appear in the SQL string
        let input = filter(
            "title",
            "eq",
            GqlValue::String("'; DROP TABLE doogats; --".into()),
        );
        let wc = build_where_sql(&input, &test_schema());
        assert!(!wc.sql.contains("DROP"));
        assert!(!wc.sql.contains("';"));
        assert_eq!(wc.sql, r#""title" = ?"#);
        assert_eq!(
            wc.params,
            vec![SqlValue::Text("'; DROP TABLE doogats; --".into())]
        );
    }

    #[test]
    fn test_unknown_column_ignored() {
        let input = filter("nonexistent", "eq", GqlValue::String("val".into()));
        let wc = build_where_sql(&input, &test_schema());
        assert!(wc.is_empty());
    }

    #[test]
    fn test_multiple_field_filters_and() {
        // { title: { contains: "rust" }, priority: { gte: 3 } } → AND
        let mut obj = IndexMap::new();
        let mut f1 = IndexMap::new();
        f1.insert(Name::new("contains"), GqlValue::String("rust".into()));
        obj.insert(Name::new("title"), GqlValue::Object(f1));
        let mut f2 = IndexMap::new();
        f2.insert(Name::new("gte"), GqlValue::Number(3.into()));
        obj.insert(Name::new("priority"), GqlValue::Object(f2));

        let input = GqlValue::Object(obj);
        let wc = build_where_sql(&input, &test_schema());
        assert!(wc.sql.contains(r#""title" LIKE '%' || ? || '%'"#));
        assert!(wc.sql.contains(r#""priority" >= ?"#));
        assert!(wc.sql.contains(" AND "));
        assert_eq!(wc.params.len(), 2);
    }

    // -- Sorting tests --

    fn order(field: &str, dir: &str) -> GqlValue {
        let mut obj = IndexMap::new();
        obj.insert(Name::new(field), GqlValue::Enum(Name::new(dir)));
        GqlValue::Object(obj)
    }

    #[test]
    fn test_single_sort_asc() {
        let input = order("title", "ASC");
        let sql = build_order_sql(&input, &test_schema());
        assert_eq!(sql.as_deref(), Some(r#""title" ASC"#));
    }

    #[test]
    fn test_single_sort_desc() {
        let input = order("priority", "DESC");
        let sql = build_order_sql(&input, &test_schema());
        assert_eq!(sql.as_deref(), Some(r#""priority" DESC"#));
    }

    #[test]
    fn test_multi_sort() {
        let mut obj = IndexMap::new();
        obj.insert(Name::new("title"), GqlValue::Enum(Name::new("ASC")));
        obj.insert(Name::new("priority"), GqlValue::Enum(Name::new("DESC")));
        let input = GqlValue::Object(obj);
        let sql = build_order_sql(&input, &test_schema()).unwrap();
        assert!(sql.contains(r#""title" ASC"#));
        assert!(sql.contains(r#""priority" DESC"#));
    }

    #[test]
    fn test_default_sort_empty() {
        let input = GqlValue::Object(IndexMap::new());
        assert!(build_order_sql(&input, &test_schema()).is_none());
    }

    #[test]
    fn test_sort_unknown_column_ignored() {
        let input = order("nonexistent", "ASC");
        assert!(build_order_sql(&input, &test_schema()).is_none());
    }

    // -- Aggregation tests --

    #[test]
    fn test_aggregate_sql_count_only() {
        // Schema with only TEXT columns → only COUNT(*)
        let schema = TableSchema {
            table_name: "note".to_string(),
            columns: vec![ColumnDef {
                name: "title".to_string(),
                data_type: "TEXT".to_string(),
                references: None,
                zone: None,
                required: false,
                search_boost: None,
                allowed_values: None,
                default_value: None,
                on_delete: ddb_core::types::OnDeleteAction::Restrict,
            }],
            crdt_strategy: None,
            template_sections: Vec::new(),
            folder: false,
            stale_after_days: None,
            title_template: None,
            origin: None,
            unique_together: None,
        };
        let wc = WhereClause::empty();
        let (sql, names) = build_aggregate_sql("note", &schema, &wc);
        assert_eq!(sql, r#"SELECT COUNT(*) AS count FROM "note""#);
        assert_eq!(names, vec!["count"]);
    }

    #[test]
    fn test_aggregate_sql_with_numeric() {
        let wc = WhereClause::empty();
        let (sql, names) = build_aggregate_sql("bookmark", &test_schema(), &wc);
        // test_schema has "priority" INTEGER column
        assert!(sql.contains("COUNT(*) AS count"));
        assert!(sql.contains(r#"MIN("priority") AS "minPriority""#));
        assert!(sql.contains(r#"MAX("priority") AS "maxPriority""#));
        assert!(sql.contains(r#"SUM("priority") AS "sumPriority""#));
        assert!(sql.contains(r#"AVG("priority") AS "avgPriority""#));
        assert!(names.contains(&"count".to_string()));
        assert!(names.contains(&"minPriority".to_string()));
    }

    #[test]
    fn test_aggregate_sql_with_where() {
        let wc = WhereClause {
            sql: r#""status" = ?"#.to_string(),
            params: vec![SqlValue::Text("done".into())],
        };
        let (sql, _) = build_aggregate_sql("bookmark", &test_schema(), &wc);
        assert!(sql.contains(r#"WHERE "status" = ?"#));
    }

    #[test]
    fn test_aggregate_row_to_value() {
        let row = vec!["42".to_string(), "1.5".to_string(), "10.0".to_string()];
        let names = vec![
            "count".to_string(),
            "minPriority".to_string(),
            "maxPriority".to_string(),
        ];
        let val = aggregate_row_to_value(&row, &names);
        if let GqlValue::Object(map) = val {
            assert_eq!(map.get("count"), Some(&GqlValue::from(42i64)));
            assert_eq!(map.get("minPriority"), Some(&GqlValue::from(1.5f64)));
            assert_eq!(map.get("maxPriority"), Some(&GqlValue::from(10.0f64)));
        } else {
            panic!("expected object");
        }
    }

    // -- Base field (id, title) tests --

    #[test]
    fn test_base_field_id_eq() {
        let input = filter("id", "eq", GqlValue::String("20260401120000".into()));
        let wc = build_where_sql(&input, &test_schema());
        assert_eq!(wc.sql, r#""id" = ?"#);
        assert_eq!(wc.params, vec![SqlValue::Text("20260401120000".into())]);
    }

    #[test]
    fn test_base_field_id_in() {
        let list = GqlValue::List(vec![
            GqlValue::String("20260401120000".into()),
            GqlValue::String("20260401130000".into()),
        ]);
        let input = filter("id", "in", list);
        let wc = build_where_sql(&input, &test_schema());
        assert_eq!(wc.sql, r#""id" IN (?, ?)"#);
        assert_eq!(
            wc.params,
            vec![
                SqlValue::Text("20260401120000".into()),
                SqlValue::Text("20260401130000".into()),
            ]
        );
    }

    #[test]
    fn test_base_field_title_eq() {
        // title IS in test_schema columns — resolved via resolve_column, not BASE_FILTER_FIELDS
        let input = filter("title", "eq", GqlValue::String("My Bookmark".into()));
        let wc = build_where_sql(&input, &test_schema());
        assert_eq!(wc.sql, r#""title" = ?"#);
        assert_eq!(wc.params, vec![SqlValue::Text("My Bookmark".into())]);
    }

    #[test]
    fn test_base_field_compound_id_and_title() {
        // _and: [{id: {eq: "20260401120000"}}, {title: {contains: "rust"}}]
        let mut id_item = IndexMap::new();
        let mut id_filter = IndexMap::new();
        id_filter.insert(Name::new("eq"), GqlValue::String("20260401120000".into()));
        id_item.insert(Name::new("id"), GqlValue::Object(id_filter));

        let mut title_item = IndexMap::new();
        let mut title_filter = IndexMap::new();
        title_filter.insert(Name::new("contains"), GqlValue::String("rust".into()));
        title_item.insert(Name::new("title"), GqlValue::Object(title_filter));

        let mut obj = IndexMap::new();
        obj.insert(
            Name::new("_and"),
            GqlValue::List(vec![
                GqlValue::Object(id_item),
                GqlValue::Object(title_item),
            ]),
        );
        let input = GqlValue::Object(obj);
        let wc = build_where_sql(&input, &test_schema());
        assert_eq!(
            wc.sql,
            r#"(("id" = ?) AND ("title" LIKE '%' || ? || '%' COLLATE NOCASE))"#
        );
        assert_eq!(
            wc.params,
            vec![
                SqlValue::Text("20260401120000".into()),
                SqlValue::Text("rust".into()),
            ]
        );
    }

    #[test]
    fn test_base_field_or_compound() {
        // _or: [{id: {eq: "id1"}}, {id: {eq: "id2"}}]
        let mut or1 = IndexMap::new();
        let mut f1 = IndexMap::new();
        f1.insert(Name::new("eq"), GqlValue::String("20260401120000".into()));
        or1.insert(Name::new("id"), GqlValue::Object(f1));

        let mut or2 = IndexMap::new();
        let mut f2 = IndexMap::new();
        f2.insert(Name::new("eq"), GqlValue::String("20260401130000".into()));
        or2.insert(Name::new("id"), GqlValue::Object(f2));

        let mut obj = IndexMap::new();
        obj.insert(
            Name::new("_or"),
            GqlValue::List(vec![GqlValue::Object(or1), GqlValue::Object(or2)]),
        );
        let input = GqlValue::Object(obj);
        let wc = build_where_sql(&input, &test_schema());
        assert_eq!(wc.sql, r#"(("id" = ?) OR ("id" = ?))"#);
        assert_eq!(
            wc.params,
            vec![
                SqlValue::Text("20260401120000".into()),
                SqlValue::Text("20260401130000".into()),
            ]
        );
    }

    #[test]
    fn test_build_where_input_includes_base_fields() {
        // Verify build_where_input registers id and title on the InputObject.
        // test_schema has columns [title, priority, status, category].
        // title is deduped (already in columns), so base fields add only id.
        // Expected: id + title + priority + status + category + _and + _or = 7 fields.
        let schema = test_schema();
        let input = build_where_input("Bookmark", &schema);
        // InputObject doesn't expose field introspection directly, but we can
        // register it in a dynamic schema and check the SDL.
        let sdl_schema = async_graphql::dynamic::Schema::build("Query", None, None)
            .register(string_filter())
            .register(int_filter())
            .register(id_filter())
            .register(tags_filter())
            .register(input)
            .register(async_graphql::dynamic::Object::new("Query").field(
                Field::new("dummy", TypeRef::named(TypeRef::STRING), |_| {
                    FieldFuture::new(async { Ok(Option::<FieldValue>::None) })
                }),
            ))
            .finish()
            .expect("schema build");
        let sdl = sdl_schema.sdl();
        assert!(sdl.contains("id: IDFilter"), "SDL must contain 'id: IDFilter' in BookmarkWhere, got:\n{sdl}");
        assert!(sdl.contains("title: StringFilter"), "SDL must contain 'title: StringFilter' in BookmarkWhere, got:\n{sdl}");
    }

    // ── PRD 00129 §5: tags filter on per-type Where inputs ──

    fn schema_with_table(name: &str, columns: Vec<&str>) -> TableSchema {
        TableSchema {
            table_name: name.into(),
            columns: columns
                .into_iter()
                .map(|c| ColumnDef {
                    name: c.into(),
                    data_type: "TEXT".into(),
                    references: None,
                    zone: None,
                    required: false,
                    search_boost: None,
                    allowed_values: None,
                    default_value: None,
                    on_delete: ddb_core::types::OnDeleteAction::Restrict,
                })
                .collect(),
            crdt_strategy: None,
            template_sections: vec![],
            folder: false,
            stale_after_days: None,
            title_template: None,
            origin: None,
            unique_together: None,
        }
    }

    #[test]
    fn tags_filter_input_object_exposes_three_operators_prd_00129() {
        let sdl_schema = async_graphql::dynamic::Schema::build("Query", None, None)
            .register(tags_filter())
            .register(async_graphql::dynamic::Object::new("Query").field(
                Field::new("dummy", TypeRef::named(TypeRef::STRING), |_| {
                    FieldFuture::new(async { Ok(Option::<FieldValue>::None) })
                }),
            ))
            .finish()
            .expect("schema build");
        let sdl = sdl_schema.sdl();
        assert!(sdl.contains("input TagsFilter"), "{sdl}");
        assert!(sdl.contains("contains: String"), "{sdl}");
        assert!(sdl.contains("containsAll: [String!]"), "{sdl}");
        assert!(sdl.contains("containsAny: [String!]"), "{sdl}");
    }

    #[test]
    fn build_where_input_includes_tags_field_when_no_collision_prd_00129() {
        // Default: typed `link` table has no `tags` column, so the
        // synthetic `tags: TagsFilter` is added by the where builder.
        let schema = schema_with_table("link", vec!["title", "url"]);
        let input = build_where_input("Link", &schema);
        let sdl_schema = async_graphql::dynamic::Schema::build("Query", None, None)
            .register(string_filter())
            .register(int_filter())
            .register(float_filter())
            .register(bool_filter())
            .register(id_filter())
            .register(tags_filter())
            .register(input)
            .register(async_graphql::dynamic::Object::new("Query").field(
                Field::new("dummy", TypeRef::named(TypeRef::STRING), |_| {
                    FieldFuture::new(async { Ok(Option::<FieldValue>::None) })
                }),
            ))
            .finish()
            .expect("schema build");
        let sdl = sdl_schema.sdl();
        assert!(
            sdl.contains("tags: TagsFilter"),
            "LinkWhere must expose tags: TagsFilter, got:\n{sdl}"
        );
    }

    #[test]
    fn tags_contains_emits_exists_against_ddb_tags_prd_00129() {
        let schema = schema_with_table("link", vec!["title", "url"]);
        let mut filter = IndexMap::new();
        filter.insert(Name::new("contains"), GqlValue::String("rust".into()));
        let mut obj = IndexMap::new();
        obj.insert(Name::new("tags"), GqlValue::Object(filter));
        let input = GqlValue::Object(obj);
        let wc = build_where_sql(&input, &schema);
        assert_eq!(
            wc.sql,
            r#"(EXISTS (SELECT 1 FROM _ddb_tags _t WHERE _t.doogat_id = "link".id AND _t.tag = ?))"#
        );
        assert_eq!(wc.params, vec![SqlValue::Text("rust".into())]);
    }

    #[test]
    fn tags_contains_all_emits_exists_per_tag_prd_00129() {
        let schema = schema_with_table("link", vec!["title", "url"]);
        let mut filter = IndexMap::new();
        filter.insert(
            Name::new("containsAll"),
            GqlValue::List(vec![
                GqlValue::String("a".into()),
                GqlValue::String("b".into()),
            ]),
        );
        let mut obj = IndexMap::new();
        obj.insert(Name::new("tags"), GqlValue::Object(filter));
        let input = GqlValue::Object(obj);
        let wc = build_where_sql(&input, &schema);
        // Two EXISTS clauses joined by AND
        let expected_one = r#"EXISTS (SELECT 1 FROM _ddb_tags _t WHERE _t.doogat_id = "link".id AND _t.tag = ?)"#;
        assert!(
            wc.sql.contains(expected_one),
            "expected EXISTS clauses, got: {}",
            wc.sql
        );
        assert_eq!(wc.params.len(), 2);
        assert_eq!(
            wc.params,
            vec![SqlValue::Text("a".into()), SqlValue::Text("b".into())]
        );
    }

    #[test]
    fn tags_contains_any_emits_single_exists_with_in_prd_00129() {
        let schema = schema_with_table("link", vec!["title", "url"]);
        let mut filter = IndexMap::new();
        filter.insert(
            Name::new("containsAny"),
            GqlValue::List(vec![
                GqlValue::String("a".into()),
                GqlValue::String("b".into()),
            ]),
        );
        let mut obj = IndexMap::new();
        obj.insert(Name::new("tags"), GqlValue::Object(filter));
        let input = GqlValue::Object(obj);
        let wc = build_where_sql(&input, &schema);
        assert_eq!(
            wc.sql,
            r#"(EXISTS (SELECT 1 FROM _ddb_tags _t WHERE _t.doogat_id = "link".id AND _t.tag IN (?, ?)))"#
        );
        assert_eq!(
            wc.params,
            vec![SqlValue::Text("a".into()), SqlValue::Text("b".into())]
        );
    }

    #[test]
    fn tags_contains_any_empty_list_emits_false_prd_00129() {
        // `tags: { containsAny: [] }` mirrors the empty-`in: []` semantics
        // elsewhere — match nothing, not "match everything vacuously".
        let schema = schema_with_table("link", vec!["title"]);
        let mut filter = IndexMap::new();
        filter.insert(Name::new("containsAny"), GqlValue::List(vec![]));
        let mut obj = IndexMap::new();
        obj.insert(Name::new("tags"), GqlValue::Object(filter));
        let input = GqlValue::Object(obj);
        let wc = build_where_sql(&input, &schema);
        assert_eq!(wc.sql, "(0 = 1)");
        assert!(wc.params.is_empty());
    }
}
