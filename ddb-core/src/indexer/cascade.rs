//! Schema loading and reference queries used to plan cascade deletes.

use std::collections::BTreeMap;

use rusqlite::{params, Connection, OptionalExtension};

use super::filter::escape_sql_ident;
use crate::error::Result;
use crate::parser;
use crate::sql_engine::schema_from_parsed;
use crate::types::{OnDeleteAction, TableSchema};

/// Preserve the indexer's per-item resilience: log every skipped typedef.
pub(crate) fn load_schemas(
    conn: &Connection,
    mut read_content: impl FnMut(&str) -> Result<String>,
) -> Result<BTreeMap<String, TableSchema>> {
    let mut stmt =
        conn.prepare("SELECT path FROM doogats WHERE type = '_typedef' ORDER BY path")?;
    let paths = stmt.query_map([], |row| row.get::<_, String>(0))?;
    let mut schemas = BTreeMap::new();
    for path in paths {
        let path = match path {
            Ok(path) => path,
            Err(error) => {
                tracing::warn!(%error, "cascade delete preflight: cannot decode typedef path; skipped");
                continue;
            }
        };
        let loaded = (|| {
            let content = read_content(&path)?;
            let parsed = parser::parse(&content, &path)?;
            schema_from_parsed(&parsed)
        })();
        match loaded {
            Ok(schema) => {
                schemas.insert(schema.table_name.clone(), schema);
            }
            Err(error) => {
                tracing::warn!(%path, %error, "cascade delete preflight: cannot load schema; skipped");
            }
        }
    }
    Ok(schemas)
}

pub(crate) fn check_restrict(
    conn: &Connection,
    schemas: &BTreeMap<String, TableSchema>,
    deleted_id: &str,
) -> Result<()> {
    for (table, schema) in schemas {
        for column in &schema.columns {
            if column.references.is_none()
                || !column.required
                || column.on_delete != OnDeleteAction::Restrict
            {
                continue;
            }
            let sql = format!(
                "SELECT id FROM \"{}\" WHERE \"{}\" = ?1 ORDER BY id LIMIT 1",
                escape_sql_ident(table),
                escape_sql_ident(&column.name)
            );
            let blocker: Option<String> = conn
                .query_row(&sql, params![deleted_id], |row| row.get(0))
                .optional()?;
            if let Some(id) = blocker {
                return Err(crate::error::DoogatError::references_violation(
                    deleted_id,
                    &column.name,
                    table,
                    id,
                ));
            }
        }
    }
    Ok(())
}

pub(crate) fn collect_children(
    conn: &Connection,
    schemas: &BTreeMap<String, TableSchema>,
    deleted_id: &str,
) -> Result<Vec<(String, String)>> {
    let mut children = Vec::new();
    for (table, schema) in schemas {
        for column in &schema.columns {
            if column.references.is_none() || column.on_delete != OnDeleteAction::Cascade {
                continue;
            }
            let sql = format!(
                "SELECT id FROM \"{}\" WHERE \"{}\" = ?1 ORDER BY id",
                escape_sql_ident(table),
                escape_sql_ident(&column.name)
            );
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt.query_map(params![deleted_id], |row| row.get::<_, String>(0))?;
            for row in rows {
                match row {
                    Ok(id) => children.push((table.clone(), id)),
                    Err(error) => {
                        tracing::warn!(%table, %error, "cascade delete preflight: cannot decode child id; skipped");
                    }
                }
            }
        }
    }
    Ok(children)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schema(table: &str, column: &str, action: OnDeleteAction) -> TableSchema {
        TableSchema {
            table_name: table.into(),
            columns: vec![crate::types::ColumnDef {
                name: column.into(),
                data_type: "TEXT".into(),
                references: Some("parent".into()),
                zone: None,
                required: true,
                search_boost: None,
                allowed_values: None,
                default_value: None,
                on_delete: action,
            }],
            crdt_strategy: None,
            template_sections: vec![],
            folder: false,
            stale_after_days: None,
            title_template: None,
            origin: None,
            unique_together: None,
            search_key: None,
            singleton: false,
        }
    }

    #[test]
    fn reference_scans_quote_identifiers_and_bind_values() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE \"child\"\"table\" (id TEXT, \"parent\"\"id\" TEXT)")
            .unwrap();
        let id = "parent' OR 1=1 --";
        conn.execute(
            "INSERT INTO \"child\"\"table\" VALUES ('matching', ?1), ('other', 'unrelated')",
            params![id],
        )
        .unwrap();
        let schemas = BTreeMap::from([(
            "child\"table".into(),
            schema("child\"table", "parent\"id", OnDeleteAction::Cascade),
        )]);
        assert_eq!(
            collect_children(&conn, &schemas, id).unwrap(),
            vec![("child\"table".into(), "matching".into())]
        );
        let schemas = BTreeMap::from([(
            "child\"table".into(),
            schema("child\"table", "parent\"id", OnDeleteAction::Restrict),
        )]);
        assert!(check_restrict(&conn, &schemas, "absent").is_ok());
        assert!(matches!(
            check_restrict(&conn, &schemas, id),
            Err(crate::error::DoogatError::Structured {
                code: crate::error::codes::REFERENCES_VIOLATION,
                ..
            })
        ));
    }

    #[test]
    fn malformed_child_id_does_not_hide_valid_children() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE child (id, parent TEXT); INSERT INTO child VALUES (1, 'root'), ('valid', 'root')").unwrap();
        let schemas = BTreeMap::from([(
            "child".into(),
            schema("child", "parent", OnDeleteAction::Cascade),
        )]);
        assert_eq!(
            collect_children(&conn, &schemas, "root").unwrap(),
            vec![("child".into(), "valid".into())]
        );
    }

    #[test]
    fn unreadable_typedef_does_not_hide_valid_schemas() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE doogats (path TEXT, type TEXT); INSERT INTO doogats VALUES ('missing.md', '_typedef'), ('valid.md', '_typedef')").unwrap();
        let schemas = load_schemas(&conn, |path| {
            if path == "missing.md" { return Err(crate::error::DoogatError::NotFound(path.into())); }
            Ok("---\nid: 20260101000000\ntitle: child\ntype: _typedef\ncolumns:\n  - name: parent\n    data_type: TEXT\n    references: parent\n    on_delete: cascade\n---\n".into())
        }).unwrap();
        assert_eq!(schemas.len(), 1);
        assert!(schemas.contains_key("child"));
    }
}
