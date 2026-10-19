use async_graphql::Value as GqlValue;
use ddb_core::types::QueryValue;
use ddb_core::types::TableSchema;
use std::collections::HashMap;

use crate::schema::resolve_column;

use super::junction::build_tags_filter_condition;
use super::reverse::{resolve_reference_target, resolve_reverse_target};

// -- WHERE clause builder --

/// Lookup of every registered type schema, keyed by `TableSchema::table_name`.
/// Threaded through the compiler for relation resolution (consumed by the
/// forward/reverse relation tasks; this task only plumbs it to the seam).
pub type SchemaLookup = HashMap<String, TableSchema>;

/// Max EXISTS nesting depth for relation sub-filters.
const MAX_RELATION_DEPTH: usize = 5;

/// Recursion context threaded through the WHERE-clause compiler.
pub(crate) struct WhereCtx<'a> {
    pub schema: &'a TableSchema,    // the "current row" type
    pub schemas: &'a SchemaLookup,  // table_name -> TableSchema, for relation resolution
    pub qualifier: Option<&'a str>, // None = top-level (cols unqualified, row-ref = schema.table_name)
    pub depth: usize,               // EXISTS nesting depth, capped at MAX_RELATION_DEPTH
    pub alias_seq: &'a mut u32,     // monotonic counter for unique EXISTS aliases
    pub params: &'a mut Vec<QueryValue>,
}

impl<'a> WhereCtx<'a> {
    /// Table or alias used for an EXISTS `... = <row_ref>.id` correlation.
    pub fn row_ref(&self) -> &str {
        self.qualifier.unwrap_or(&self.schema.table_name)
    }
}

/// Parameterized SQL WHERE clause.
#[derive(Debug)]
pub struct WhereClause {
    pub sql: String,
    pub params: Vec<QueryValue>,
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

/// Build a parameterized WHERE clause from a GraphQL filter input value,
/// surfacing user-input validation errors (e.g. an empty `TagsFilter`)
/// to the caller. Resolvers prefer this entry point so they can return
/// a clean GraphQL error instead of silently dropping the clause.
///
/// The `input` should be the resolved `{Type}Where` object value.
/// Column names are validated against the schema to prevent injection.
pub fn try_build_where_sql(
    input: &GqlValue,
    schema: &TableSchema,
    schemas: &SchemaLookup,
) -> Result<WhereClause, String> {
    let mut conditions = Vec::new();
    let mut params = Vec::new();
    let mut alias_seq = 0u32;

    let mut ctx = WhereCtx {
        schema,
        schemas,
        qualifier: None,
        depth: 0,
        alias_seq: &mut alias_seq,
        params: &mut params,
    };
    build_conditions_into(input, &mut ctx, &mut conditions)?;

    let wc = if conditions.is_empty() {
        WhereClause::empty()
    } else {
        WhereClause {
            sql: conditions.join(" AND "),
            params,
        }
    };
    Ok(wc)
}

/// Tests-and-internal-callers entry point. Discards validation errors and
/// returns an empty clause instead — production resolvers should call
/// `try_build_where_sql` so user-input errors surface to the GraphQL
/// response.
pub fn build_where_sql(
    input: &GqlValue,
    schema: &TableSchema,
    schemas: &SchemaLookup,
) -> WhereClause {
    try_build_where_sql(input, schema, schemas).unwrap_or_else(|_| WhereClause::empty())
}

/// Walk a `{Type}Where` object value, appending each field's SQL condition to
/// `conditions`. Shared by the top-level entry and the logical combinators, so
/// every recursion level threads the same `WhereCtx` (one params Vec, one alias
/// counter) instead of allocating fresh state per branch.
pub(crate) fn build_conditions_into(
    obj: &GqlValue,
    ctx: &mut WhereCtx,
    conditions: &mut Vec<String>,
) -> Result<(), String> {
    if ctx.depth > MAX_RELATION_DEPTH {
        return Err("relation filter nested too deep (max 5)".to_string());
    }

    let GqlValue::Object(obj) = obj else {
        return Ok(());
    };

    for (name, value) in obj {
        let field = name.as_str();
        match field {
            "_and" | "_or" => {
                let combinator = if field == "_and" { "AND" } else { "OR" };
                let GqlValue::List(items) = value else {
                    continue;
                };
                if let Some(cond) = build_logical_combinator(items, combinator, ctx)? {
                    conditions.push(cond);
                }
            }
            // PRD 00129 §5: `tags` is a synthetic field backed by the
            // doogat-level `_ddb_tags` index, not a column on the
            // materialized type table. It only fires when the typedef
            // doesn't have its own `tags` column (the where-input
            // generator hides `TagsFilter` in that case so this branch
            // only runs for the synthetic path).
            "tags" if !ctx.schema.columns.iter().any(|c| c.name == "tags") => {
                if let Some(cond) = build_tags_filter_condition(value, ctx)? {
                    conditions.push(cond);
                }
            }
            _ => {
                route_field_condition(field, value, ctx, conditions)?;
            }
        }
    }

    Ok(())
}

/// Route the `_ =>` arm of [`build_conditions_into`]: a field resolving to a
/// REFERENCES column whose target type is registered routes to the forward
/// junction EXISTS emitter; a reverse-relation field routes to the reverse
/// EXISTS emitter; everything else keeps the existing direct-column path.
fn route_field_condition(
    field: &str,
    value: &GqlValue,
    ctx: &mut WhereCtx,
    conditions: &mut Vec<String>,
) -> Result<(), String> {
    if let Some((parent_col, target_table)) = resolve_reference_target(field, ctx) {
        for cond in crate::relation_filter::build_forward_relation(
            &parent_col,
            &target_table,
            value,
            ctx,
        )? {
            conditions.push(cond);
        }
    } else if let Some((ref_type, ref_col)) = resolve_reverse_target(field, ctx) {
        for cond in
            crate::relation_filter::build_reverse_relation(&ref_type, &ref_col, value, ctx)?
        {
            conditions.push(cond);
        }
    } else {
        build_column_conditions(field, value, ctx, conditions);
    }
    Ok(())
}

/// Resolve a column name and apply its operator conditions.
fn build_column_conditions(
    field: &str,
    value: &GqlValue,
    ctx: &mut WhereCtx,
    conditions: &mut Vec<String>,
) {
    let col_name = resolve_column(&ctx.schema.columns, field)
        .or_else(|| BASE_FILTER_FIELDS.iter().find(|&&f| f == field).copied());
    let Some(col_name) = col_name else { return };
    let GqlValue::Object(filter_obj) = value else {
        return;
    };
    let col_ref = match ctx.qualifier {
        None => format!("\"{col_name}\""),
        Some(a) => format!("\"{a}\".\"{col_name}\""),
    };
    for (op, val) in filter_obj {
        if let Some(cond) = build_operator_condition(&col_ref, op.as_str(), val, ctx.params) {
            conditions.push(cond);
        }
    }
}

/// Recursively build a compound AND/OR clause from a list of filter items.
///
/// Each item recurses through `build_conditions_into` on a child ctx that keeps
/// the parent's `qualifier`/`depth` and SHARES the same `alias_seq`/`params`, so
/// bound parameters accumulate in left-to-right source order across nesting.
fn build_logical_combinator(
    items: &[GqlValue],
    combinator: &str,
    ctx: &mut WhereCtx,
) -> Result<Option<String>, String> {
    let mut sub = Vec::new();
    for item in items {
        let mut item_conditions = Vec::new();
        let mut child = WhereCtx {
            schema: ctx.schema,
            schemas: ctx.schemas,
            qualifier: ctx.qualifier,
            depth: ctx.depth,
            alias_seq: &mut *ctx.alias_seq,
            params: &mut *ctx.params,
        };
        build_conditions_into(item, &mut child, &mut item_conditions)?;
        if !item_conditions.is_empty() {
            sub.push(format!("({})", item_conditions.join(" AND ")));
        }
    }
    if sub.is_empty() {
        Ok(None)
    } else {
        Ok(Some(format!("({})", sub.join(&format!(" {combinator} ")))))
    }
}

/// Translate a single filter operator (eq, neq, contains, etc.) into SQL.
///
/// `col_ref` is the already-quoted column reference (`"col"` at top level,
/// `"alias"."col"` when qualified), so the operator text is column-agnostic.
pub(crate) fn build_operator_condition(
    col_ref: &str,
    op: &str,
    value: &GqlValue,
    params: &mut Vec<QueryValue>,
) -> Option<String> {
    match op {
        "eq" => {
            params.push(gql_to_sql(value));
            Some(format!("{col_ref} = ?"))
        }
        "neq" => {
            params.push(gql_to_sql(value));
            Some(format!("{col_ref} != ?"))
        }
        "gt" => {
            params.push(gql_to_sql(value));
            Some(format!("{col_ref} > ?"))
        }
        "gte" => {
            params.push(gql_to_sql(value));
            Some(format!("{col_ref} >= ?"))
        }
        "lt" => {
            params.push(gql_to_sql(value));
            Some(format!("{col_ref} < ?"))
        }
        "lte" => {
            params.push(gql_to_sql(value));
            Some(format!("{col_ref} <= ?"))
        }
        "contains" => {
            params.push(gql_to_sql(value));
            Some(format!("{col_ref} LIKE '%' || ? || '%' COLLATE NOCASE"))
        }
        "startsWith" => {
            params.push(gql_to_sql(value));
            Some(format!("{col_ref} LIKE ? || '%' COLLATE NOCASE"))
        }
        "in" => build_in_condition(col_ref, value, params),
        "notIn" | "nin" => build_not_in_condition(col_ref, value, params),
        "isNull" => match value {
            GqlValue::Boolean(true) => Some(format!("{col_ref} IS NULL")),
            GqlValue::Boolean(false) => Some(format!("{col_ref} IS NOT NULL")),
            _ => None,
        },
        _ => None,
    }
}

fn build_in_condition(
    col_ref: &str,
    value: &GqlValue,
    params: &mut Vec<QueryValue>,
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
    Some(format!("{col_ref} IN ({})", placeholders.join(", ")))
}

fn build_not_in_condition(
    col_ref: &str,
    value: &GqlValue,
    params: &mut Vec<QueryValue>,
) -> Option<String> {
    let items = match value {
        GqlValue::List(items) => items,
        _ => return None,
    };
    if items.is_empty() {
        // An empty `NOT IN ()` is invalid SQL, so return the always-true
        // sentinel "1" instead of emitting it.
        return Some("1".to_string());
    }
    let placeholders: Vec<&str> = items
        .iter()
        .map(|v| {
            params.push(gql_to_sql(v));
            "?"
        })
        .collect();
    Some(format!("{col_ref} NOT IN ({})", placeholders.join(", ")))
}

/// Convert a GraphQL value to an adapter-neutral query parameter value.
fn gql_to_sql(value: &GqlValue) -> QueryValue {
    match value {
        GqlValue::Number(n) => {
            if let Some(i) = n.as_i64() {
                QueryValue::Integer(i)
            } else if let Some(f) = n.as_f64() {
                QueryValue::Real(f)
            } else {
                QueryValue::Text(n.to_string())
            }
        }
        GqlValue::String(s) => QueryValue::Text(s.clone()),
        GqlValue::Boolean(b) => QueryValue::Integer(if *b { 1 } else { 0 }),
        GqlValue::Null => QueryValue::Null,
        // Enum values (SortOrder etc.) come as Name strings
        GqlValue::Enum(name) => QueryValue::Text(name.to_string()),
        _ => QueryValue::Text(value.to_string()),
    }
}
