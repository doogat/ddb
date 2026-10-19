use crate::types::SearchResult;

/// Strip a leading ` AND ` or `AND ` prefix from a SQL condition fragment
/// produced by `build_filter_clauses` / `resolve_negation_clauses`, so it
/// can be placed directly after `WHERE`.
pub(super) fn strip_leading_and(s: &str) -> &str {
    s.strip_prefix(" AND ")
        .or_else(|| s.strip_prefix("AND "))
        .unwrap_or(s)
}

pub(super) fn apply_fields_to_hits(
    hits: &mut [SearchResult],
    hit_indices: &[usize],
    mut field_map: std::collections::HashMap<String, std::collections::BTreeMap<String, String>>,
) {
    for &i in hit_indices {
        if let Some(new_fields) = field_map.remove(&hits[i].id) {
            match hits[i].fields.as_mut() {
                Some(existing) => existing.extend(new_fields),
                None => hits[i].fields = Some(new_fields),
            }
        }
    }
}
