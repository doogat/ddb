//! Index schema version detection.
//!
//! Decides whether an on-disk index was built by a different `SCHEMA_DDL`
//! shape and, when it was, performs the destructive drop that lets the current
//! DDL recreate it. Split out of `indexer/mod.rs` so the open path reads as a
//! sequence of named steps.

use rusqlite::Connection;

use crate::error::Result;

use super::Index;

/// Does this database need dropping before `SCHEMA_DDL` is (re)applied?
///
/// A `user_version` of 0 means unstamped (every index created before this
/// change) — defer to the legacy FTS-column-text probe so existing on-disk
/// indexes upgrade exactly as before. A non-zero value that isn't
/// `SCHEMA_VERSION` means this DB was stamped by a different schema shape and
/// must be dropped unconditionally, without consulting the legacy probe.
pub(super) fn needs_drop(conn: &Connection) -> Result<bool> {
    let stamped: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    Ok(if stamped == 0 {
        needs_schema_upgrade(conn)
    } else {
        stamped != Index::SCHEMA_VERSION
    })
}

/// Re-check the upgrade decision and, if it still holds, drop every table so
/// `SCHEMA_DDL` can recreate them.
///
/// The re-check is why this is not a bare drop: callers run it under the
/// rebuild lock, and another process may have finished the upgrade while they
/// waited. Without it, the loser drops and recreates tables the winner has
/// already rebuilt — and if the winner has started indexing into them, the
/// loser's drop destroys that work.
pub(super) fn drop_tables_if_still_outdated(conn: &Connection) -> Result<()> {
    if !needs_drop(conn)? {
        return Ok(());
    }
    tracing::info!("index schema outdated, dropping tables for upgrade");
    conn.execute_batch("PRAGMA foreign_keys = OFF;")?;
    let mut stmt = conn.prepare(
        "SELECT name FROM sqlite_master \
         WHERE type IN ('table', 'view') AND name NOT LIKE 'sqlite_%'",
    )?;
    let tables: Vec<String> = stmt
        .query_map([], |row| row.get(0))?
        .filter_map(|r| r.ok())
        .collect();
    drop(stmt);
    for table in &tables {
        conn.execute_batch(&format!("DROP TABLE IF EXISTS \"{table}\""))?;
    }
    conn.execute_batch("PRAGMA foreign_keys = ON;")?;
    Ok(())
}

/// Check whether the existing schema needs upgrading (e.g. old 3-column
/// FTS5 table missing `fields`, or missing `_ddb_boost` table).
fn needs_schema_upgrade(conn: &Connection) -> bool {
    let fts_exists: bool = conn
        .query_row(
            "SELECT COUNT(*) > 0 FROM sqlite_master \
             WHERE type='table' AND name='_ddb_fts'",
            [],
            |row| row.get(0),
        )
        .unwrap_or(false);
    if !fts_exists {
        return false; // fresh DB, SCHEMA_DDL will create everything
    }
    // Check FTS5 column list via sqlite_master DDL for the `fields` column
    let sql: String = conn
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type='table' AND name='_ddb_fts'",
            [],
            |row| row.get(0),
        )
        .unwrap_or_default();
    !sql.contains("fields")
}
