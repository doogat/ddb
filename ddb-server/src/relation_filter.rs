//! Forward relation sub-filters for the GraphQL typed `where:` filter.
//!
//! A REFERENCES column can be filtered by a related entity's properties
//! (`{ category: { title: { eq: "X" } } }`), compiled to an EXISTS-over-join
//! against the auto-junction table. id-ops on the same field stay a direct
//! stored-column compare (back-compat). Junction names come from the
//! single-source-of-truth helpers in `ddb_core::indexer`.
//!
//! Reverse/junction membership quantifiers are a separate task; the shared
//! helpers here are structured so reverse can be added later.

use async_graphql::dynamic::{InputObject, InputValue, TypeRef};
use async_graphql::{Name, Value as GqlValue};
use ddb_core::indexer::{junction_parent_id_column, junction_ref_id_column, junction_table_name};
use ddb_core::types::TableSchema;
use indexmap::IndexMap;
use std::collections::HashMap;

use crate::filter::{
    build_conditions_into, build_operator_condition, filter_type_for_column, WhereCtx,
};
use crate::schema::sanitize_field_name;

/// id-ops that compare the parent's stored REFERENCES column directly,
/// preserving today's `category: { eq: id }` semantics.
const ID_OPS: &[&str] = &[
    "eq",
    "neq",
    "gt",
    "gte",
    "lt",
    "lte",
    "contains",
    "startsWith",
    "in",
    "notIn",
    "nin",
    "isNull",
];

/// Quantifier keys that traverse the junction with an EXISTS sub-filter.
const QUANTIFIERS: &[&str] = &["some", "none", "every"];

/// Reserved keys that must not be inlined as target columns in the relation
/// filter input (reachable via `some:`/`none:`/`every:` instead).
const RESERVED_RELATION_KEYS: &[&str] =
    &["eq", "in", "notIn", "nin", "isNull", "some", "none", "every"];

/// Forward-relation emitter for a REFERENCES column `parent_col` on the
/// current row type, pointing at registered `target_table`.
///
/// Per key in the REFERENCES filter object:
/// - id-ops -> direct stored-column compare (no junction).
/// - `some`/`none`/`every` -> quantifier EXISTS over the junction.
/// - any other key -> implicit `some` over `{ <key>: <val> }`.
pub(crate) fn build_forward_relation(
    parent_col: &str,
    target_table: &str,
    value: &GqlValue,
    ctx: &mut WhereCtx,
) -> Result<Vec<String>, String> {
    let GqlValue::Object(filter_obj) = value else {
        return Ok(Vec::new());
    };

    let mut conditions = Vec::new();
    for (key, val) in filter_obj {
        let key = key.as_str();
        if ID_OPS.contains(&key) {
            let col_ref = match ctx.qualifier {
                None => format!("\"{parent_col}\""),
                Some(a) => format!("\"{a}\".\"{parent_col}\""),
            };
            if let Some(cond) = build_operator_condition(&col_ref, key, val, ctx.params) {
                conditions.push(cond);
            }
        } else if QUANTIFIERS.contains(&key) {
            conditions.push(emit_quantifier_exists(
                parent_col,
                target_table,
                key,
                val,
                ctx,
            )?);
        } else {
            // Inline target column -> implicit `some` over `{ key: val }`.
            let mut sub = IndexMap::new();
            sub.insert(Name::new(key), val.clone());
            let sub_obj = GqlValue::Object(sub);
            conditions.push(emit_quantifier_exists(
                parent_col,
                target_table,
                "some",
                &sub_obj,
                ctx,
            )?);
        }
    }
    Ok(conditions)
}

/// Emit a single quantifier EXISTS (`some`/`none`/`every`) over the junction
/// for the given REFERENCES column. Consumes one alias for both `_j{n}` and
/// `_r{n}`, then compiles the sub-`where` object against the target schema
/// with a child ctx (alias `_r{n}`, depth+1).
fn emit_quantifier_exists(
    parent_col: &str,
    target_table: &str,
    quantifier: &str,
    sub_value: &GqlValue,
    ctx: &mut WhereCtx,
) -> Result<String, String> {
    let n = *ctx.alias_seq;
    *ctx.alias_seq += 1;
    let j_alias = format!("_j{n}");
    let r_alias = format!("_r{n}");

    let junction = junction_table_name(&ctx.schema.table_name, parent_col);
    let parent_id = junction_parent_id_column(&ctx.schema.table_name);
    let ref_id = junction_ref_id_column(parent_col);
    let row_ref = ctx.row_ref().to_string();

    let correlation = format!("\"{j_alias}\".\"{parent_id}\" = \"{row_ref}\".id");

    // Empty `none {}` -> "uncategorized": no JOIN, just absence of any junction row.
    let is_empty_sub = matches!(sub_value, GqlValue::Object(o) if o.is_empty());
    if quantifier == "none" && is_empty_sub {
        return Ok(format!(
            "NOT EXISTS (SELECT 1 FROM \"{junction}\" \"{j_alias}\" WHERE {correlation})"
        ));
    }

    // The target schema's presence is verified before routing, so this is
    // guaranteed by the caller; resolve it for the child ctx.
    let target_schema = ctx
        .schemas
        .get(target_table)
        .expect("target schema presence checked before routing")
        .clone();

    // Compile the sub-`where` object against the target schema, qualified by
    // the relation alias and one level deeper.
    let mut sub_conditions = Vec::new();
    {
        let mut child = WhereCtx {
            schema: &target_schema,
            schemas: ctx.schemas,
            qualifier: Some(&r_alias),
            depth: ctx.depth + 1,
            alias_seq: &mut *ctx.alias_seq,
            params: &mut *ctx.params,
        };
        build_conditions_into(sub_value, &mut child, &mut sub_conditions)?;
    }
    let sub_sql = sub_conditions.join(" AND ");

    let join_and_where = format!(
        "SELECT 1 FROM \"{junction}\" \"{j_alias}\" \
         JOIN \"{target_table}\" \"{r_alias}\" ON \"{r_alias}\".id = \"{j_alias}\".\"{ref_id}\" \
         WHERE {correlation}"
    );

    let sql = match quantifier {
        "none" => format!("NOT EXISTS ({join_and_where} AND ({sub_sql}))"),
        "every" => format!("NOT EXISTS ({join_and_where} AND NOT ({sub_sql}))"),
        // `some` and implicit-some.
        _ => format!("EXISTS ({join_and_where} AND ({sub_sql}))"),
    };
    Ok(sql)
}

// -- Input-type generation --

/// `{TargetTypeName}RelationFilter` input: id-ops on the parent's stored
/// column, quantifiers over the junction, and inlined target columns.
pub(crate) fn relation_filter_input(
    target_type_name: &str,
    target_schema: &TableSchema,
) -> InputObject {
    let mut input = InputObject::new(format!("{target_type_name}RelationFilter"));

    // id-ops on the parent stored column (back-compat with the old IDFilter).
    input = input
        .field(InputValue::new("eq", TypeRef::named(TypeRef::ID)))
        .field(InputValue::new("in", TypeRef::named_list(TypeRef::ID)))
        .field(InputValue::new("notIn", TypeRef::named_list(TypeRef::ID)))
        .field(InputValue::new("nin", TypeRef::named_list(TypeRef::ID)))
        .field(InputValue::new("isNull", TypeRef::named(TypeRef::BOOLEAN)));

    // Quantifiers over the junction, reusing the target's Where input.
    let where_type = format!("{target_type_name}Where");
    input = input
        .field(InputValue::new("some", TypeRef::named(&where_type)))
        .field(InputValue::new("none", TypeRef::named(&where_type)))
        .field(InputValue::new("every", TypeRef::named(&where_type)));

    // Inlined target columns (implicit `some`), skipping reserved-op names.
    for col in &target_schema.columns {
        let gql_name = sanitize_field_name(&col.name);
        if RESERVED_RELATION_KEYS.contains(&gql_name.as_str()) {
            continue;
        }
        let filter_type = filter_type_for_column(col);
        input = input.field(InputValue::new(&gql_name, TypeRef::named(filter_type)));
    }

    input
}

/// Every forward `RelationFilter` input object to register, deduped by name.
/// Keyed off `known_types` so it agrees with `build_where_input` on which
/// targets get a `RelationFilter`.
pub(crate) fn relation_input_objects(
    type_schemas: &[TableSchema],
    known_types: &HashMap<String, String>,
) -> Vec<InputObject> {
    let mut objects = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let by_name: HashMap<&str, &TableSchema> = type_schemas
        .iter()
        .map(|s| (s.table_name.as_str(), s))
        .collect();

    for schema in type_schemas {
        for col in &schema.columns {
            let Some(target) = col.references.as_ref() else {
                continue;
            };
            let Some(target_type_name) = known_types.get(target) else {
                continue;
            };
            let Some(target_schema) = by_name.get(target.as_str()) else {
                continue;
            };
            if !seen.insert(target_type_name.clone()) {
                continue;
            }
            objects.push(relation_filter_input(target_type_name, target_schema));
        }
    }
    objects
}
