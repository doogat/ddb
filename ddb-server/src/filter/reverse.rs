use ddb_core::types::TableSchema;

use crate::schema::{pluralize_preserving_case, sanitize_field_name, sanitize_type_name};

use super::where_build::WhereCtx;

/// If `field` resolves to a REFERENCES column on the current row type whose
/// referenced target is a registered schema, return `(stored_column_name,
/// target_table)`. Returns `None` for non-reference columns or unregistered
/// targets (keeps the legacy direct-column path).
pub(super) fn resolve_reference_target(field: &str, ctx: &WhereCtx) -> Option<(String, String)> {
    let col = ctx
        .schema
        .columns
        .iter()
        .find(|c| c.name == field || sanitize_field_name(&c.name) == field)?;
    let target = col.references.as_ref()?;
    if ctx.schemas.contains_key(target) {
        Some((col.name.clone(), target.clone()))
    } else {
        None
    }
}

/// A reverse relation discovered on the current row type: another registered
/// type `ref_table` references it via `ref_col`, surfaced on the current
/// Where input as the field `field_name` typed `{ref_type_name}MembershipFilter`.
pub(crate) struct ReverseRelation {
    pub field_name: String,
    pub ref_table: String,
    pub ref_col: String,
    pub ref_type_name: String,
}

/// Field names already present on `current`'s Where input: its sanitized
/// columns plus the synthetic base fields (`id`/`title`/`tags`) and the
/// logical combinators (`_and`/`_or`). A reverse-relation name colliding with
/// any of these must fall back or be skipped. Split out of `reverse_relations`
/// to keep that function under the size limit; the set is unchanged.
fn current_field_names(current: &TableSchema) -> std::collections::HashSet<String> {
    let mut existing: std::collections::HashSet<String> = current
        .columns
        .iter()
        .map(|c| sanitize_field_name(&c.name))
        .collect();
    for base in ["id", "title", "tags"] {
        existing.insert(base.to_string());
    }
    existing.insert("_and".to_string());
    existing.insert("_or".to_string());
    existing
}

/// Deterministically enumerate the reverse relations on `current`: every
/// registered type that REFERENCES `current` via some column. Shared by
/// `build_where_input` (to emit the reverse field) and `resolve_reverse_target`
/// (to route the compiler) so the generated name and the routing never drift.
///
/// `registered` is sorted by `table_name` first for a stable collision
/// fallback regardless of caller order. The default field name pluralizes the
/// referencing type; on collision with an existing Where field or an
/// already-assigned reverse field it falls back to `{table}_{col}`; a still
/// colliding name is warned and skipped (never silently duplicated).
pub(crate) fn reverse_relations(
    current: &TableSchema,
    registered: &[&TableSchema],
) -> Vec<ReverseRelation> {
    let existing = current_field_names(current);

    let mut sorted: Vec<&TableSchema> = registered.to_vec();
    sorted.sort_by(|a, b| a.table_name.cmp(&b.table_name));

    let mut taken: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut out = Vec::new();
    for r in sorted {
        // Intentional skip: a self-referencing type gets no reverse membership
        // field. Its reverse name would collide with (and duplicate) the
        // forward field already on the same Where input; reverse
        // self-membership is out of scope.
        if r.table_name == current.table_name {
            continue;
        }
        for rc in &r.columns {
            if rc.references.as_deref() != Some(current.table_name.as_str()) {
                continue;
            }
            let default = pluralize_preserving_case(&sanitize_field_name(&r.table_name));
            let parts = (r.table_name.as_str(), rc.name.as_str());
            let Some(field_name) =
                pick_reverse_field_name(default, parts, &current.table_name, &existing, &taken)
            else {
                continue;
            };
            taken.insert(field_name.clone());
            out.push(ReverseRelation {
                field_name,
                ref_table: r.table_name.clone(),
                ref_col: rc.name.clone(),
                ref_type_name: sanitize_type_name(&r.table_name),
            });
        }
    }
    out
}

/// Resolve the field name for a reverse relation, handling collisions. Returns
/// the pluralized `default` when free; on collision with an existing Where
/// field or an already-`taken` reverse field, falls back to `{table}_{col}`
/// (the `fallback_parts`); if that also collides, warns and returns `None` so
/// the caller skips the relation (never silently duplicated).
fn pick_reverse_field_name(
    default: String,
    fallback_parts: (&str, &str),
    current_table: &str,
    existing: &std::collections::HashSet<String>,
    taken: &std::collections::HashSet<String>,
) -> Option<String> {
    let (table, col) = fallback_parts;
    if existing.contains(&default) || taken.contains(&default) {
        let fallback = sanitize_field_name(&format!("{}_{}", table, col));
        if existing.contains(&fallback) || taken.contains(&fallback) {
            tracing::warn!(
                "skipping reverse relation '{}.{}' -> '{}': field names '{}' and '{}' both collide",
                table,
                col,
                current_table,
                default,
                fallback
            );
            return None;
        }
        Some(fallback)
    } else {
        Some(default)
    }
}

/// If `field` is a reverse-relation field on the current row type, return the
/// referencing `(table, column)` so the compiler can emit the reverse EXISTS.
pub(super) fn resolve_reverse_target(field: &str, ctx: &WhereCtx) -> Option<(String, String)> {
    let registered: Vec<&TableSchema> = ctx.schemas.values().collect();
    reverse_relations(ctx.schema, &registered)
        .into_iter()
        .find(|r| r.field_name == field)
        .map(|r| (r.ref_table, r.ref_col))
}
