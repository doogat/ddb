mod inference;
mod rows;
mod schema_helpers;
mod tables;
#[cfg(test)]
mod tests;

/// Column names reserved for core doogat fields in materialized tables.
///
/// Exposed at the crate root via `ddb_core::indexer::is_core_column` so
/// downstream crates (the GraphQL server) can distinguish core columns from
/// user-defined ones when iterating `TableSchema.columns` post-PRD 00122.
///
/// This is intentionally narrower than `RESERVED_COLUMNS` in
/// `ddb-core/src/sql_engine/dml/mod.rs`: this set drives whether a user-declared
/// column gets a slot in the materialized typed table. `created_at` and
/// `tags` are NOT in this set because pre-PRD 122 users could legally
/// declare them as typed columns; the doogat pipeline itself doesn't write
/// them into the materialized typed table (`created_at` lives in the
/// internal `doogats` index, `tags` lives in `_ddb_tags`), so the
/// declaration was harmless. The GraphQL server uses its own
/// `BASE_DOOGAT_FIELDS` set to decide which typed-column fields would
/// clobber a base doogat field — see `typed_doogat_to_value` in
/// `ddb-server/src/schema/base_types.rs`.
pub fn is_core_column(name: &str) -> bool {
    matches!(name, "id" | "title" | "type" | "date" | "updated_at")
}

/// Normalize a boolean string to "1" or "0" for SQLite storage.
pub(crate) fn normalize_bool_str(val: &str) -> String {
    match val.to_lowercase().as_str() {
        "true" | "1" | "yes" => "1".to_string(),
        "false" | "0" | "no" => "0".to_string(),
        _ => val.to_string(),
    }
}

/// Build SQL column definitions from schema columns, including core columns.
fn build_column_definitions(columns: &[crate::types::ColumnDef]) -> Vec<String> {
    let mut col_defs = vec![
        "id TEXT PRIMARY KEY".to_string(),
        "title TEXT".to_string(),
        "date TEXT".to_string(),
        "updated_at TEXT".to_string(),
    ];
    for col in columns {
        if is_core_column(&col.name) {
            continue;
        }
        let sql_type = match col.data_type.to_uppercase().as_str() {
            "INTEGER" => "INTEGER",
            "REAL" => "REAL",
            "BOOLEAN" => "INTEGER",
            _ => "TEXT",
        };
        let check = if let Some(ref vals) = col.allowed_values {
            let quoted: Vec<String> = vals
                .iter()
                .map(|v| format!("'{}'", v.replace('\'', "''")))
                .collect();
            format!(
                " CHECK(\"{}\" IS NULL OR \"{}\" IN ({}))",
                col.name,
                col.name,
                quoted.join(", ")
            )
        } else {
            String::new()
        };
        col_defs.push(format!("\"{}\" {}{}", col.name, sql_type, check));
    }
    col_defs
}

/// Junction table name for a REFERENCES column (single source of truth).
pub fn junction_table_name(type_name: &str, col_name: &str) -> String {
    format!("{type_name}_{col_name}")
}

/// Parent-id column name in a junction table.
pub fn junction_parent_id_column(type_name: &str) -> String {
    format!("{type_name}_id")
}

/// Referenced-id column name in a junction table.
pub fn junction_ref_id_column(col_name: &str) -> String {
    format!("{col_name}_id")
}

/// DDL for creating a junction table for a REFERENCES column.
pub fn junction_table_ddl(table_name: &str, col_name: &str) -> String {
    let table = junction_table_name(table_name, col_name);
    let parent_id = junction_parent_id_column(table_name);
    let ref_id = junction_ref_id_column(col_name);
    format!(
        "CREATE TABLE IF NOT EXISTS \"{table}\" (\"{parent_id}\" TEXT NOT NULL, \"{ref_id}\" TEXT NOT NULL, PRIMARY KEY (\"{parent_id}\", \"{ref_id}\"))"
    )
}
