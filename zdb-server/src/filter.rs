use async_graphql::dynamic::{
    Enum, EnumItem, Field, FieldFuture, FieldValue, InputObject, InputValue, Object, TypeRef,
};
use async_graphql::{Name, Value as GqlValue};
use indexmap::IndexMap;
use rusqlite::types::Value as SqlValue;
use zdb_core::types::TableSchema;

use crate::schema::is_valid_graphql_name;

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

/// Returns the GraphQL type name for the filter matching a column's data type.
pub fn filter_type_for_column(col: &zdb_core::types::ColumnDef) -> &'static str {
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
        if !is_valid_graphql_name(&col.name) {
            continue;
        }
        let filter_type = filter_type_for_column(col);
        input = input.field(InputValue::new(&col.name, TypeRef::named(filter_type)));
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
        if !is_valid_graphql_name(&col.name) {
            continue;
        }
        input = input.field(InputValue::new(&col.name, TypeRef::named("SortOrder")));
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
        let col = name.as_str();
        // Validate column exists in schema
        if !schema.columns.iter().any(|c| c.name == col) {
            continue;
        }
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

fn capitalize_first(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        None => String::new(),
        Some(f) => f.to_uppercase().to_string() + c.as_str(),
    }
}

/// Check if a column type is numeric.
fn is_numeric(data_type: &str) -> bool {
    matches!(data_type.to_uppercase().as_str(), "INTEGER" | "REAL")
}

/// Generate a `{TypeName}Aggregate` object type.
///
/// Always has `count: Int!`. For each numeric column, adds
/// `min{Col}`, `max{Col}`, `sum{Col}`, `avg{Col}` as nullable Float fields.
pub fn build_aggregate_type(type_name: &str, schema: &TableSchema) -> Object {
    let mut obj = Object::new(format!("{type_name}Aggregate"));

    // count: Int!
    obj = obj.field(Field::new(
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
        if !is_numeric(&col.data_type) || !is_valid_graphql_name(&col.name) {
            continue;
        }
        let cap = capitalize_first(&col.name);
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

/// Build the SQL for an aggregate query on a materialized table.
///
/// Returns (sql, column_names) where column_names maps positionally
/// to the GraphQL aggregate field names.
pub fn build_aggregate_sql(
    table_name: &str,
    schema: &TableSchema,
    where_clause: &WhereClause,
) -> (String, Vec<String>) {
    let mut selects = vec!["COUNT(*) AS count".to_string()];
    let mut names = vec!["count".to_string()];

    for col in &schema.columns {
        if !is_numeric(&col.data_type) {
            continue;
        }
        let cap = capitalize_first(&col.name);
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

    let where_part = if where_clause.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", where_clause.sql)
    };

    let sql = format!(
        "SELECT {} FROM \"{table_name}\"{where_part}",
        selects.join(", ")
    );

    (sql, names)
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
            "_and" => {
                if let GqlValue::List(items) = value {
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
                    if !sub.is_empty() {
                        conditions.push(format!("({})", sub.join(" AND ")));
                    }
                }
            }
            "_or" => {
                if let GqlValue::List(items) = value {
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
                    if !sub.is_empty() {
                        conditions.push(format!("({})", sub.join(" OR ")));
                    }
                }
            }
            _ => {
                // Column filter — validate name exists in schema
                if schema.columns.iter().any(|c| c.name == field) {
                    if let GqlValue::Object(filter_obj) = value {
                        for (op, val) in filter_obj {
                            if let Some(cond) =
                                build_operator_condition(field, op.as_str(), val, &mut params)
                            {
                                conditions.push(cond);
                            }
                        }
                    }
                }
            }
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
        "in" => {
            if let GqlValue::List(items) = value {
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
            } else {
                None
            }
        }
        _ => None, // unknown operator — skip
    }
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
    use zdb_core::types::{ColumnDef, TableSchema};

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
                },
            ],
            crdt_strategy: None,
            template_sections: Vec::new(),
            folder: false,
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
            GqlValue::String("'; DROP TABLE zettels; --".into()),
        );
        let wc = build_where_sql(&input, &test_schema());
        assert!(!wc.sql.contains("DROP"));
        assert!(!wc.sql.contains("';"));
        assert_eq!(wc.sql, r#""title" = ?"#);
        assert_eq!(
            wc.params,
            vec![SqlValue::Text("'; DROP TABLE zettels; --".into())]
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
            }],
            crdt_strategy: None,
            template_sections: Vec::new(),
            folder: false,
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
}
