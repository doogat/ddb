use ddb_core::types::ColumnDef;

pub(crate) fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        None => String::new(),
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
    }
}

/// Strip trailing `_id` suffix from a column name.
/// Returns the input unchanged if it doesn't end with `_id` or is just `"id"`.
pub(crate) fn strip_id_suffix(s: &str) -> &str {
    if s.len() > 3 && s.ends_with("_id") {
        &s[..s.len() - 3]
    } else {
        s
    }
}

pub(crate) fn pluralize(s: &str) -> String {
    let s = s.to_lowercase();
    if s.ends_with('s') {
        format!("{s}es")
    } else if s.ends_with('y') {
        format!("{}ies", &s[..s.len() - 1])
    } else {
        format!("{s}s")
    }
}

/// Pluralize without lowercasing - preserves camelCase.
pub(crate) fn pluralize_preserving_case(s: &str) -> String {
    let lower = s.to_ascii_lowercase();
    if lower.ends_with('s')
        || lower.ends_with("sh")
        || lower.ends_with("ch")
        || lower.ends_with('x')
        || lower.ends_with('z')
    {
        format!("{s}es")
    } else if lower.ends_with('y') && s.len() > 1 {
        format!("{}ies", &s[..s.len() - 1])
    } else {
        format!("{s}s")
    }
}

/// Convert a kebab-case type name to PascalCase for GraphQL type names.
/// If no hyphens, capitalizes the first character only.
pub(crate) fn sanitize_type_name(s: &str) -> String {
    if s.is_empty() {
        return String::new();
    }
    if s.contains('-') {
        s.split('-').map(capitalize).collect()
    } else {
        capitalize(s)
    }
}

/// Convert a typedef table name into the snake_case field base used by
/// SINGLETON singular queries and mutations.
///
/// GraphQL field names must start with `[A-Za-z_]` and continue with
/// `[A-Za-z0-9_]`. `is_safe_sql_identifier` already enforces both for
/// typedefs created via `CREATE TABLE`, but typedefs loaded from a
/// hand-written `ddb/_typedef/*.md` skip that gate. Strip any leading
/// underscores produced by leading-hyphen typedef names so the emitted
/// field name is always a valid GraphQL identifier; the collision check
/// in `queries.rs`/`mutations.rs` will fail schema build if the cleaned
/// base then collides with another field.
pub(crate) fn singleton_field_base(s: &str) -> String {
    let raw = s.replace('-', "_");
    raw.trim_start_matches('_').to_string()
}

/// Convert a kebab-case field name to camelCase for GraphQL field names.
/// First segment is lowercased, subsequent segments are capitalized.
pub(crate) fn sanitize_field_name(s: &str) -> String {
    if s.is_empty() {
        return String::new();
    }
    if s.contains('-') {
        let mut parts = s.split('-');
        let first = parts
            .next()
            .expect("split always yields at least one")
            .to_lowercase();
        let rest: String = parts.map(capitalize).collect();
        format!("{first}{rest}")
    } else {
        let mut c = s.chars();
        match c.next() {
            None => String::new(),
            Some(f) => f.to_lowercase().collect::<String>() + c.as_str(),
        }
    }
}

/// Find a column by its GraphQL name (original or sanitized).
/// Returns the original column name for use in SQL.
pub(crate) fn resolve_column<'a>(columns: &'a [ColumnDef], gql_name: &str) -> Option<&'a str> {
    columns
        .iter()
        .find(|c| c.name == gql_name || sanitize_field_name(&c.name) == gql_name)
        .map(|c| c.name.as_str())
}
