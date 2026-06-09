//! Parser-based classification of SQL for schema-reload decisions.
//!
//! Transport adapters (GraphQL `executeSql`/`executeBatch`, PgWire) reload the
//! dynamic schema after a statement changes a table/typedef. They previously
//! decided this with `sql.to_uppercase().contains("CREATE TABLE")`, which
//! false-fired on string literals (`SELECT 'CREATE TABLE x'`) and comments
//! (`-- CREATE TABLE`). This module parses the SQL and inspects statement kinds
//! instead, so only genuine table DDL triggers a reload.

use sqlparser::ast::{ObjectType, Statement};
use sqlparser::dialect::GenericDialect;
use sqlparser::parser::Parser;

/// Returns `true` when executing `sql` may change the table schema and therefore
/// requires a dynamic-schema reload.
///
/// A statement requires reload when it is `CREATE TABLE`, `ALTER TABLE`, or
/// `DROP TABLE`. SELECT/INSERT/UPDATE/DELETE and non-table DDL (indexes, views)
/// do not. `sql` may hold several `;`-separated statements; the function returns
/// `true` if any one of them mutates the schema.
///
/// Conservative on parse failure: ddb accepts custom schema DDL that
/// `sqlparser` cannot parse (`CREATE TABLE ... SINGLETON`, `ALTER TABLE ... SET
/// ZONE ...`, etc.; see `sql_engine::try_custom_ddl`). Every such custom path is
/// itself a schema mutation, and standard DML/SELECT always parses, so an
/// already-executed statement that fails to parse here is a schema mutation.
/// Returning `true` keeps those reloads firing.
pub fn requires_schema_reload(sql: &str) -> bool {
    match Parser::parse_sql(&GenericDialect {}, sql) {
        Ok(statements) => statements.iter().any(mutates_schema),
        Err(_) => true,
    }
}

/// True for the table-level DDL statements that change the dynamic schema.
fn mutates_schema(stmt: &Statement) -> bool {
    matches!(
        stmt,
        Statement::CreateTable(_)
            | Statement::AlterTable { .. }
            | Statement::Drop {
                object_type: ObjectType::Table,
                ..
            }
    )
}

#[cfg(test)]
mod tests {
    use super::requires_schema_reload;

    #[test]
    fn create_table_requires_reload() {
        assert!(requires_schema_reload(
            "CREATE TABLE book (id TEXT, title TEXT)"
        ));
    }

    #[test]
    fn drop_table_requires_reload() {
        assert!(requires_schema_reload("DROP TABLE book"));
    }

    #[test]
    fn alter_table_requires_reload() {
        assert!(requires_schema_reload(
            "ALTER TABLE book ADD COLUMN author TEXT"
        ));
    }

    #[test]
    fn plain_select_does_not_require_reload() {
        assert!(!requires_schema_reload("SELECT id, title FROM book"));
    }

    #[test]
    fn create_table_in_string_literal_does_not_require_reload() {
        // The exact false positive the naive substring check produced.
        assert!(!requires_schema_reload("SELECT 'CREATE TABLE x' AS note"));
    }

    #[test]
    fn create_table_in_comment_does_not_require_reload() {
        assert!(!requires_schema_reload("-- CREATE TABLE x\nSELECT 1"));
    }

    #[test]
    fn dml_does_not_require_reload() {
        assert!(!requires_schema_reload("INSERT INTO book (id) VALUES ('1')"));
        assert!(!requires_schema_reload(
            "UPDATE book SET title = 'x' WHERE id = '1'"
        ));
        assert!(!requires_schema_reload("DELETE FROM book WHERE id = '1'"));
    }

    #[test]
    fn drop_index_does_not_require_reload() {
        // Only table-level DDL changes the schema; index DDL does not.
        assert!(!requires_schema_reload("DROP INDEX idx_book_title"));
    }

    #[test]
    fn multi_statement_batch_with_ddl_requires_reload() {
        assert!(requires_schema_reload(
            "INSERT INTO book (id) VALUES ('1'); ALTER TABLE book ADD COLUMN author TEXT"
        ));
    }

    #[test]
    fn unparseable_custom_ddl_conservatively_requires_reload() {
        // ddb accepts custom schema DDL sqlparser can't parse (the SINGLETON
        // suffix here). Every custom-DDL path is a schema mutation, so an
        // unparseable-but-executed statement must still trigger reload.
        assert!(requires_schema_reload(
            "CREATE TABLE config (id TEXT) SINGLETON"
        ));
    }
}
