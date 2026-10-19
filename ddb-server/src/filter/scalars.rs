use async_graphql::dynamic::{Enum, EnumItem, InputObject, InputValue, TypeRef};
use async_graphql::Value as GqlValue;
use ddb_core::types::TableSchema;
use std::collections::HashMap;

use crate::schema::{resolve_column, sanitize_field_name};

use super::reverse::reverse_relations;

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
        .field(InputValue::new(
            "notIn",
            TypeRef::named_list(TypeRef::STRING),
        ))
        .field(InputValue::new("nin", TypeRef::named_list(TypeRef::STRING)))
        .field(InputValue::new("isNull", TypeRef::named(TypeRef::BOOLEAN)))
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
        .field(InputValue::new("notIn", TypeRef::named_list(TypeRef::INT)))
        .field(InputValue::new("nin", TypeRef::named_list(TypeRef::INT)))
        .field(InputValue::new("isNull", TypeRef::named(TypeRef::BOOLEAN)))
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
        .field(InputValue::new(
            "notIn",
            TypeRef::named_list(TypeRef::FLOAT),
        ))
        .field(InputValue::new("nin", TypeRef::named_list(TypeRef::FLOAT)))
        .field(InputValue::new("isNull", TypeRef::named(TypeRef::BOOLEAN)))
}

/// `BoolFilter` — equality on BOOLEAN columns.
pub fn bool_filter() -> InputObject {
    InputObject::new("BoolFilter")
        .field(InputValue::new("eq", TypeRef::named(TypeRef::BOOLEAN)))
        .field(InputValue::new(
            "notIn",
            TypeRef::named_list(TypeRef::BOOLEAN),
        ))
        .field(InputValue::new(
            "nin",
            TypeRef::named_list(TypeRef::BOOLEAN),
        ))
        .field(InputValue::new("isNull", TypeRef::named(TypeRef::BOOLEAN)))
}

/// `IDFilter` — equality and set membership on ID/reference columns.
pub fn id_filter() -> InputObject {
    InputObject::new("IDFilter")
        .field(InputValue::new("eq", TypeRef::named(TypeRef::ID)))
        .field(InputValue::new("in", TypeRef::named_list(TypeRef::ID)))
        .field(InputValue::new("notIn", TypeRef::named_list(TypeRef::ID)))
        .field(InputValue::new("nin", TypeRef::named_list(TypeRef::ID)))
        .field(InputValue::new("isNull", TypeRef::named(TypeRef::BOOLEAN)))
}

/// `TagsFilter` — match the tag set on the doogat-level `_ddb_tags`
/// index. Available on every per-type query's `where` input. PRD 00129
/// §5. The filter composes with column filters via the same conjunction
/// the rest of the where clause uses; combinator semantics:
/// - `contains`: row has the named tag.
/// - `containsAll`: row has every listed tag (one EXISTS per tag,
///   AND-ed).
/// - `containsAny`: row has at least one of the listed tags.
///
/// Issue #11: all three operators are nullable. Callers supply at least
/// one; resolve-time validation rejects an empty filter (no operator
/// supplied) or empty `containsAll` / `containsAny` arrays. The earlier
/// schema marked `containsAll` / `containsAny` as `[String!]!`, forcing
/// every caller to pass all three even when only `contains` was meant.
pub fn tags_filter() -> InputObject {
    InputObject::new("TagsFilter")
        .description(
            "Filter doogats by tag set. Supply at least one of: `contains` (single tag), \
             `containsAll` (every listed tag), `containsAny` (any one of the listed tags). \
             Empty filter or empty `containsAll` / `containsAny` arrays are rejected.",
        )
        .field(InputValue::new("contains", TypeRef::named(TypeRef::STRING)))
        .field(InputValue::new(
            "containsAll",
            TypeRef::named_nn_list(TypeRef::STRING),
        ))
        .field(InputValue::new(
            "containsAny",
            TypeRef::named_nn_list(TypeRef::STRING),
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
///
/// A REFERENCES column whose target type is registered in `known_types` is
/// typed `{TargetTypeName}RelationFilter` (forward relation traversal);
/// otherwise it keeps the legacy bare `IDFilter` so there is no dangling type
/// reference.
pub fn build_where_input(
    type_name: &str,
    schema: &TableSchema,
    schemas: &[TableSchema],
    known_types: &HashMap<String, String>,
) -> InputObject {
    let name = format!("{type_name}Where");
    let mut input = InputObject::new(&name);

    for col in &schema.columns {
        let gql_name = sanitize_field_name(&col.name);
        let filter_type = match &col.references {
            Some(target) => match known_types.get(target) {
                Some(target_type_name) => format!("{target_type_name}RelationFilter"),
                None => "IDFilter".to_string(),
            },
            None => filter_type_for_column(col).to_string(),
        };
        input = input.field(InputValue::new(&gql_name, TypeRef::named(&filter_type)));
    }

    // Base doogat fields (always present in materialized tables)
    let sanitized: Vec<String> = schema
        .columns
        .iter()
        .map(|c| sanitize_field_name(&c.name))
        .collect();
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

    // Reverse-relation fields: every registered type that REFERENCES this one
    // surfaces as a `{Type}MembershipFilter` quantifier field.
    let registered: Vec<&TableSchema> = schemas
        .iter()
        .filter(|s| known_types.contains_key(&s.table_name))
        .collect();
    for rev in reverse_relations(schema, &registered) {
        input = input.field(InputValue::new(
            &rev.field_name,
            TypeRef::named(format!("{}MembershipFilter", rev.ref_type_name)),
        ));
    }

    input
}

// -- Sorting types --

/// `SortOrder` GraphQL enum — ASC or DESC.
pub fn sort_order_enum() -> Enum {
    Enum::new("SortOrder")
        .item(EnumItem::new("ASC"))
        .item(EnumItem::new("DESC"))
}

/// Base doogat sort keys injected into every `{Type}OrderBy` input, paired
/// with the materialized column each resolves to in `build_order_sql`.
///
/// `created_at` backs onto the `date` column on purpose: the typed object's
/// `created_at` field returns the doogat's `date` value, so ordering by
/// `date` matches the value the client actually sees. `updated_at` and `id`
/// back onto their own columns.
const BASE_ORDER_FIELDS: &[(&str, &str)] = &[
    ("id", "id"),
    ("created_at", "date"),
    ("updated_at", "updated_at"),
];

/// Resolve a base sort key to its materialized backing column.
fn base_order_column(name: &str) -> Option<&'static str> {
    BASE_ORDER_FIELDS
        .iter()
        .find(|(key, _)| *key == name)
        .map(|(_, col)| *col)
}

/// Generate a `{TypeName}OrderBy` input type from a `TableSchema`.
pub fn build_order_by_input(type_name: &str, schema: &TableSchema) -> InputObject {
    let mut input = InputObject::new(format!("{type_name}OrderBy"));

    for col in &schema.columns {
        let gql_name = sanitize_field_name(&col.name);
        input = input.field(InputValue::new(&gql_name, TypeRef::named("SortOrder")));
    }

    // Base doogat sort keys, collision-guarded against typedef columns of the
    // same sanitized name (mirrors build_where_input's base-field injection).
    let sanitized: Vec<String> = schema
        .columns
        .iter()
        .map(|c| sanitize_field_name(&c.name))
        .collect();
    for (key, _) in BASE_ORDER_FIELDS {
        if !sanitized.iter().any(|n| n == key) {
            input = input.field(InputValue::new(*key, TypeRef::named("SortOrder")));
        }
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
    let mut has_id = false;
    let mut last_dir = "ASC";
    for (name, value) in obj {
        let Some(col) = resolve_column(&schema.columns, name.as_str())
            .or_else(|| base_order_column(name.as_str()))
        else {
            continue;
        };
        let dir = match value {
            GqlValue::Enum(e) => e.as_str(),
            GqlValue::String(s) => s.as_str(),
            _ => continue,
        };
        match dir {
            "ASC" | "DESC" => {
                if col == "id" {
                    has_id = true;
                }
                last_dir = dir;
                parts.push(format!("\"{col}\" {dir}"));
            }
            _ => continue,
        }
    }

    if parts.is_empty() {
        None
    } else {
        // Deterministic total order for offset pagination: append the unique
        // primary key as a final tiebreaker unless the caller already sorts by id.
        // The tiebreaker mirrors the last sort term's direction so ties extend the
        // ordering naturally — `created_at DESC` keeps same-date rows newest-id
        // first, not oldest-first. `date` is day-level, so created_at ties are the
        // common case. Either direction is a stable total order, so pagination
        // stays gap/dupe-free regardless.
        if !has_id {
            parts.push(format!("\"id\" {last_dir}"));
        }
        Some(parts.join(", "))
    }
}
