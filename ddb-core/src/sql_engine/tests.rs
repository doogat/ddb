
use super::helpers::{
    data_type_to_string, eval_expr, is_literal_expr, parse_title_template, value_to_sql,
    TemplatePlaceholder,
};
use super::*;
use crate::git_ops::GitRepo;
use crate::indexer::Index;
use crate::types::{ColumnDef, DoogatMeta, ParsedDoogat, TableSchema, Value, Zone};
use sqlparser::ast::{Expr, SetExpr};
use std::collections::{BTreeMap, BTreeSet};
use tempfile::TempDir;

// Test helpers
fn engine_exec_ok(repo: &crate::git_ops::GitRepo, index: &crate::indexer::Index, sql: &str) {
    let mut engine = SqlEngine::new(index, repo);
    engine.execute(sql).unwrap();
}

fn engine_exec_id(
    repo: &crate::git_ops::GitRepo,
    index: &crate::indexer::Index,
    sql: &str,
) -> String {
    let mut engine = SqlEngine::new(index, repo);
    match engine.execute(sql).unwrap() {
        SqlResult::Ok(id) => id,
        _ => panic!("expected Ok"),
    }
}

fn setup() -> (TempDir, GitRepo, Index) {
    let dir = TempDir::new().unwrap();
    let repo = GitRepo::init(dir.path()).unwrap();
    let db_path = dir.path().join(".ddb/index.db");
    std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
    let index = Index::open(&db_path).unwrap();
    (dir, repo, index)
}

#[test]
fn create_table_produces_typedef_doogat() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    let result = engine
        .execute("CREATE TABLE projects (name TEXT, priority INTEGER)")
        .unwrap();

    match result {
        SqlResult::Ok(msg) => assert!(msg.contains("projects")),
        _ => panic!("expected Ok"),
    }

    // Typedef doogat should be in index
    let rows = index
        .query_raw("SELECT title, type FROM doogats WHERE type = '_typedef'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0][0], "projects");
    assert_eq!(rows[0][1], "_typedef");

    // Materialized table should exist
    let rows = index.query_raw("SELECT COUNT(*) FROM projects").unwrap();
    assert_eq!(rows[0][0], "0");
}

#[test]
fn create_table_rejects_reserved_names() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    let err = engine
        .execute("CREATE TABLE doogats (name TEXT)")
        .unwrap_err();
    assert!(format!("{err}").contains("reserved"));

    let err = engine
        .execute("CREATE TABLE _ddb_foo (name TEXT)")
        .unwrap_err();
    assert!(format!("{err}").contains("reserved"));
}

#[test]
fn create_table_rejects_duplicate() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine.execute("CREATE TABLE projects (name TEXT)").unwrap();
    let err = engine
        .execute("CREATE TABLE projects (name TEXT)")
        .unwrap_err();
    assert!(format!("{err}").contains("already exists"));
}

#[test]
fn create_table_if_not_exists_is_idempotent() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE IF NOT EXISTS projects (name TEXT)")
        .unwrap();
    // Second call with IF NOT EXISTS should succeed (no-op)
    let result = engine
        .execute("CREATE TABLE IF NOT EXISTS projects (name TEXT)")
        .unwrap();
    match &result {
        SqlResult::Ok(msg) => assert!(msg.contains("skipped")),
        other => panic!("expected SqlResult::Ok, got {other:?}"),
    }

    // Without IF NOT EXISTS should still error
    let err = engine
        .execute("CREATE TABLE projects (name TEXT)")
        .unwrap_err();
    assert!(format!("{err}").contains("already exists"));
}

#[test]
fn create_table_with_references() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine.execute("CREATE TABLE people (name TEXT)").unwrap();
    engine
        .execute("CREATE TABLE tasks (name TEXT, assignee TEXT REFERENCES people(id))")
        .unwrap();

    // Check materialized table has correct columns
    let rows = index.query_raw("PRAGMA table_info(tasks)").unwrap();
    let col_names: Vec<&str> = rows.iter().map(|r| r[1].as_str()).collect();
    assert!(col_names.contains(&"id"));
    assert!(col_names.contains(&"name"));
    assert!(col_names.contains(&"assignee"));
}

#[test]
fn insert_creates_doogat_and_materialized_row() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE projects (title TEXT, status TEXT, priority INTEGER)")
        .unwrap();

    let result = engine
        .execute(
            "INSERT INTO projects (title, status, priority) VALUES ('Alpha', 'active', 1)",
        )
        .unwrap();

    let doogat_id = match result {
        SqlResult::Ok(id) => id,
        _ => panic!("expected Ok with id"),
    };

    // Check materialized table
    let rows = index
        .query_raw("SELECT title, status, priority FROM projects")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0][0], "Alpha");
    assert_eq!(rows[0][1], "active");
    assert_eq!(rows[0][2], "1");

    // Check doogat exists in index
    let rows = index
        .query_raw(&format!(
            "SELECT title, type FROM doogats WHERE id = '{doogat_id}'"
        ))
        .unwrap();
    assert_eq!(rows[0][0], "Alpha");
    assert_eq!(rows[0][1], "projects");

    // Check doogat file in Git (no folder: true → flat path)
    let path = index.resolve_path(&doogat_id).unwrap();
    assert!(path.starts_with("ddb/") && !path.contains("projects/"));
    let content = repo.read_file(&path).unwrap();
    assert!(content.contains("type: projects"));
    assert!(content.contains("priority: 1"));
    assert!(content.contains("title: Alpha"));
}

#[test]
fn insert_multi_row_creates_n_doogats() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE items (name TEXT, score INTEGER)")
        .unwrap();

    let result = engine
        .execute(
            "INSERT INTO items (name, score) VALUES ('alpha', 10), ('beta', 20), ('gamma', 30)",
        )
        .unwrap();

    // Returns comma-separated IDs
    let ids_str = match result {
        SqlResult::Ok(ids) => ids,
        _ => panic!("expected Ok with ids"),
    };
    let ids: Vec<&str> = ids_str.split(',').collect();
    assert_eq!(ids.len(), 3, "should return 3 IDs");

    // All IDs are distinct 14-digit timestamps
    for id in &ids {
        assert_eq!(id.len(), 14, "ID should be 14 digits: {id}");
        assert!(
            id.chars().all(|c| c.is_ascii_digit()),
            "ID should be numeric: {id}"
        );
    }
    let unique: std::collections::HashSet<&&str> = ids.iter().collect();
    assert_eq!(unique.len(), 3, "all IDs should be unique");

    // 3 rows in materialized table
    let rows = index
        .query_raw("SELECT name, score FROM items ORDER BY name")
        .unwrap();
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0][0], "alpha");
    assert_eq!(rows[0][1], "10");
    assert_eq!(rows[1][0], "beta");
    assert_eq!(rows[1][1], "20");
    assert_eq!(rows[2][0], "gamma");
    assert_eq!(rows[2][1], "30");

    // 3 doogats in index
    let count = index
        .query_raw("SELECT COUNT(*) FROM doogats WHERE type = 'items'")
        .unwrap();
    assert_eq!(count[0][0], "3");
}

#[test]
fn insert_multi_row_single_commit() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine.execute("CREATE TABLE things (label TEXT)").unwrap();

    let head_before = repo.head_oid().unwrap();

    engine
        .execute("INSERT INTO things (label) VALUES ('a'), ('b'), ('c')")
        .unwrap();

    let head_after = repo.head_oid().unwrap();
    // Head moved (commit happened)
    assert_ne!(head_before.0, head_after.0);

    // The single commit contains all 3 files
    let diff = repo.diff_paths(&head_before.0, &head_after.0).unwrap();
    assert_eq!(diff.len(), 3, "single commit should contain 3 new files");
}

#[test]
fn select_returns_materialized_data() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE items (name TEXT, count INTEGER)")
        .unwrap();
    engine
        .execute("INSERT INTO items (name, count) VALUES ('Widget', 42)")
        .unwrap();

    let result = engine.execute("SELECT name, count FROM items").unwrap();
    match result {
        SqlResult::Rows { rows, .. } => {
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0][0], "Widget");
            assert_eq!(rows[0][1], "42");
        }
        _ => panic!("expected Rows"),
    }
}

#[test]
fn update_modifies_doogat_and_materialized_row() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE projects (name TEXT, priority INTEGER)")
        .unwrap();
    let id = match engine
        .execute("INSERT INTO projects (name, priority) VALUES ('Alpha', 1)")
        .unwrap()
    {
        SqlResult::Ok(id) => id,
        _ => panic!("expected Ok"),
    };

    engine
        .execute(&format!(
            "UPDATE projects SET priority = 5 WHERE id = '{id}'"
        ))
        .unwrap();

    // Check materialized table
    let rows = index.query_raw("SELECT priority FROM projects").unwrap();
    assert_eq!(rows[0][0], "5");

    // Check doogat file (resolve via index since typed → subfolder)
    let path = index.resolve_path(&id).unwrap();
    let content = repo.read_file(&path).unwrap();
    assert!(content.contains("priority: 5"));
}

#[test]
fn delete_removes_doogat_and_materialized_row() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine.execute("CREATE TABLE projects (name TEXT)").unwrap();
    let id = match engine
        .execute("INSERT INTO projects (name) VALUES ('Alpha')")
        .unwrap()
    {
        SqlResult::Ok(id) => id,
        _ => panic!("expected Ok"),
    };

    engine
        .execute(&format!("DELETE FROM projects WHERE id = '{id}'"))
        .unwrap();

    // Materialized table should be empty
    let rows = index.query_raw("SELECT COUNT(*) FROM projects").unwrap();
    assert_eq!(rows[0][0], "0");

    // Doogat should be gone from index
    let rows = index
        .query_raw(&format!("SELECT COUNT(*) FROM doogats WHERE id = '{id}'"))
        .unwrap();
    assert_eq!(rows[0][0], "0");

    // File should be gone from Git
    let result = repo.read_file(&format!("ddb/projects/{id}.md"));
    assert!(result.is_err());
}

#[test]
fn full_create_insert_select_update_delete_cycle() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    // CREATE
    engine
        .execute("CREATE TABLE tasks (name TEXT, status TEXT, priority INTEGER)")
        .unwrap();

    // INSERT
    let id = match engine
        .execute("INSERT INTO tasks (name, status, priority) VALUES ('Build feature', 'todo', 3)")
        .unwrap()
    {
        SqlResult::Ok(id) => id,
        _ => panic!("expected Ok"),
    };

    // SELECT
    let result = engine
        .execute("SELECT name, status, priority FROM tasks")
        .unwrap();
    match result {
        SqlResult::Rows { rows, .. } => {
            assert_eq!(rows[0][0], "Build feature");
            assert_eq!(rows[0][1], "todo");
            assert_eq!(rows[0][2], "3");
        }
        _ => panic!("expected Rows"),
    }

    // UPDATE
    engine
        .execute(&format!(
            "UPDATE tasks SET status = 'done', priority = 1 WHERE id = '{id}'"
        ))
        .unwrap();

    let result = engine
        .execute("SELECT status, priority FROM tasks")
        .unwrap();
    match result {
        SqlResult::Rows { rows, .. } => {
            assert_eq!(rows[0][0], "done");
            assert_eq!(rows[0][1], "1");
        }
        _ => panic!("expected Rows"),
    }

    // DELETE
    engine
        .execute(&format!("DELETE FROM tasks WHERE id = '{id}'"))
        .unwrap();
    let result = engine.execute("SELECT COUNT(*) FROM tasks").unwrap();
    match result {
        SqlResult::Rows { rows, .. } => assert_eq!(rows[0][0], "0"),
        _ => panic!("expected Rows"),
    }
}

#[test]
fn insert_with_fk_validates_reference() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine.execute("CREATE TABLE people (name TEXT)").unwrap();
    engine
        .execute("CREATE TABLE tasks (name TEXT, assignee TEXT REFERENCES people(id))")
        .unwrap();

    // Insert with non-existent reference should fail with structured
    // REFERENCES_VIOLATION code (PRD 00133 unify-typed-write-paths).
    let err = engine
        .execute("INSERT INTO tasks (name, assignee) VALUES ('Fix bug', '99999999999999')")
        .unwrap_err();
    let msg = format!("{err}");
    assert!(
        msg.contains("references non-existent people"),
        "expected dangling reference message, got: {msg}"
    );
}

#[test]
fn insert_produces_correct_zone_mapping() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute(
            "CREATE TABLE projects (title TEXT, name TEXT, status TEXT, priority INTEGER)",
        )
        .unwrap();

    let id = match engine
        .execute(
            "INSERT INTO projects (title, name, status, priority) VALUES ('Alpha', 'Alpha', 'active', 1)",
        )
        .unwrap()
    {
        SqlResult::Ok(id) => id,
        _ => panic!("expected Ok"),
    };

    let path = index.resolve_path(&id).unwrap();
    let content = repo.read_file(&path).unwrap();

    // priority (INTEGER) → frontmatter
    assert!(content.contains("priority: 1"));
    // name (TEXT) → body section
    assert!(content.contains("## name\n\nAlpha"));
    // status (TEXT) → body section
    assert!(content.contains("## status\n\nactive"));
    // type should be table name
    assert!(content.contains("type: projects"));
    // explicit title set
    assert!(content.contains("title: Alpha"));
}

#[test]
fn typed_doogat_stored_in_subfolder_and_crud_works() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine.execute("CREATE TABLE docs (name TEXT)").unwrap();

    // Add folder: true to the typedef
    let typedef_rows = index
        .query_raw("SELECT id, path FROM doogats WHERE type = '_typedef' AND title = 'docs'")
        .unwrap();
    let typedef_path = &typedef_rows[0][1];
    let typedef_content = repo.read_file(typedef_path).unwrap();
    let updated = typedef_content.replace("type: _typedef", "type: _typedef\nfolder: true");
    repo.commit_file(typedef_path, &updated, "add folder to docs typedef")
        .unwrap();
    let parsed = crate::parser::parse(&updated, typedef_path).unwrap();
    index.index_doogat(&parsed).unwrap();
    // Recreate engine to pick up updated typedef
    let mut engine = SqlEngine::new(&index, &repo);

    // INSERT → should go to ddb/docs/{id}.md (folder: true)
    let id = match engine
        .execute("INSERT INTO docs (name) VALUES ('Guide')")
        .unwrap()
    {
        SqlResult::Ok(id) => id,
        _ => panic!("expected Ok"),
    };
    let path = index.resolve_path(&id).unwrap();
    assert!(
        path.starts_with("ddb/docs/"),
        "path should be in type subfolder: {path}"
    );

    // UPDATE via SQL → should find it in subfolder
    engine
        .execute(&format!(
            "UPDATE docs SET name = 'Manual' WHERE id = '{id}'"
        ))
        .unwrap();
    let content = repo.read_file(&path).unwrap();
    assert!(content.contains("Manual"));

    // DELETE via SQL → should remove from subfolder
    engine
        .execute(&format!("DELETE FROM docs WHERE id = '{id}'"))
        .unwrap();
    assert!(repo.read_file(&path).is_err());
}

#[test]
fn insert_fills_default_value() {
    let (_dir, repo, index) = setup();

    // Manually create typedef with allowed_values + default_value
    let typedef = "---\nid: 20260301110000\ntitle: task\ntype: _typedef\ncolumns:\n  - name: status\n    data_type: TEXT\n    zone: frontmatter\n    allowed_values:\n      - todo\n      - doing\n      - done\n    default_value: todo\n  - name: name\n    data_type: TEXT\n    zone: frontmatter\n---\n";
    let typedef_path = "ddb/_typedef/20260301110000.md";
    repo.commit_file(typedef_path, typedef, "add typedef")
        .unwrap();
    let parsed = crate::parser::parse(typedef, typedef_path).unwrap();
    index.index_doogat(&parsed).unwrap();
    index.materialize_all_types(&repo).unwrap();

    let mut engine = SqlEngine::new(&index, &repo);

    // INSERT omitting status → should get default "todo"
    let id = match engine
        .execute("INSERT INTO task (name) VALUES ('Write tests')")
        .unwrap()
    {
        SqlResult::Ok(id) => id,
        _ => panic!("expected Ok"),
    };
    let path = index.resolve_path(&id).unwrap();
    let content = repo.read_file(&path).unwrap();
    assert!(
        content.contains("status: todo"),
        "expected default status in:\n{content}"
    );
}

#[test]
fn insert_rejects_invalid_allowed_value() {
    let (_dir, repo, index) = setup();

    let typedef = "---\nid: 20260301110100\ntitle: task2\ntype: _typedef\ncolumns:\n  - name: status\n    data_type: TEXT\n    zone: frontmatter\n    allowed_values:\n      - todo\n      - doing\n      - done\n  - name: name\n    data_type: TEXT\n    zone: frontmatter\n---\n";
    let typedef_path = "ddb/_typedef/20260301110100.md";
    repo.commit_file(typedef_path, typedef, "add typedef")
        .unwrap();
    let parsed = crate::parser::parse(typedef, typedef_path).unwrap();
    index.index_doogat(&parsed).unwrap();
    index.materialize_all_types(&repo).unwrap();

    let mut engine = SqlEngine::new(&index, &repo);

    // INSERT with invalid value → should error
    let result = engine.execute("INSERT INTO task2 (name, status) VALUES ('Test', 'invalid')");
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("not in allowed values"),
        "expected validation error: {err}"
    );
}

#[test]
fn update_rejects_invalid_allowed_value() {
    let (_dir, repo, index) = setup();

    let typedef = "---\nid: 20260301110200\ntitle: task3\ntype: _typedef\ncolumns:\n  - name: status\n    data_type: TEXT\n    zone: frontmatter\n    allowed_values:\n      - todo\n      - doing\n      - done\n    default_value: todo\n  - name: name\n    data_type: TEXT\n    zone: frontmatter\n---\n";
    let typedef_path = "ddb/_typedef/20260301110200.md";
    repo.commit_file(typedef_path, typedef, "add typedef")
        .unwrap();
    let parsed = crate::parser::parse(typedef, typedef_path).unwrap();
    index.index_doogat(&parsed).unwrap();
    index.materialize_all_types(&repo).unwrap();

    let mut engine = SqlEngine::new(&index, &repo);

    // INSERT valid
    let id = match engine
        .execute("INSERT INTO task3 (name, status) VALUES ('Test', 'todo')")
        .unwrap()
    {
        SqlResult::Ok(id) => id,
        _ => panic!("expected Ok"),
    };

    // UPDATE with invalid value
    let result = engine.execute(&format!(
        "UPDATE task3 SET status = 'bad' WHERE id = '{id}'"
    ));
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("not in allowed values"),
        "expected validation error: {err}"
    );
}

#[test]
fn drop_table_cascade_deletes_all() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine.execute("CREATE TABLE dropme (name TEXT)").unwrap();
    engine
        .execute("INSERT INTO dropme (name) VALUES ('a')")
        .unwrap();
    engine
        .execute("INSERT INTO dropme (name) VALUES ('b')")
        .unwrap();

    engine.execute("DROP TABLE dropme CASCADE").unwrap();

    // Typedef gone
    let rows = index
        .query_raw("SELECT id FROM doogats WHERE type = '_typedef' AND title = 'dropme'")
        .unwrap();
    assert!(rows.is_empty());

    // Data doogats gone
    let rows = index
        .query_raw("SELECT id FROM doogats WHERE type = 'dropme'")
        .unwrap();
    assert!(rows.is_empty());

    // Materialized table gone
    let result = index.query_raw("SELECT * FROM dropme");
    assert!(result.is_err());
}

#[test]
fn drop_table_strips_type_from_data_doogats() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine.execute("CREATE TABLE stripme (name TEXT)").unwrap();
    let id = match engine
        .execute("INSERT INTO stripme (name) VALUES ('keep')")
        .unwrap()
    {
        SqlResult::Ok(id) => id,
        _ => panic!("expected Ok"),
    };

    engine.execute("DROP TABLE stripme").unwrap();

    // Typedef gone
    let rows = index
        .query_raw("SELECT id FROM doogats WHERE type = '_typedef' AND title = 'stripme'")
        .unwrap();
    assert!(rows.is_empty());

    // Data doogat still exists but type is cleared
    let path = index.resolve_path(&id).unwrap();
    let content = repo.read_file(&path).unwrap();
    assert!(!content.contains("type: stripme"));
}

#[test]
fn drop_table_removes_typedef_and_materialized() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE removeme (status TEXT)")
        .unwrap();
    engine
        .execute("INSERT INTO removeme (status) VALUES ('x')")
        .unwrap();

    // Materialized table exists before drop
    assert!(index.query_raw("SELECT * FROM removeme").is_ok());

    engine.execute("DROP TABLE removeme").unwrap();

    // Typedef removed from index
    let rows = index
        .query_raw("SELECT id FROM doogats WHERE type = '_typedef' AND title = 'removeme'")
        .unwrap();
    assert!(rows.is_empty(), "typedef should be removed");

    // Materialized table dropped
    assert!(
        index.query_raw("SELECT * FROM removeme").is_err(),
        "materialized table should be dropped"
    );
}

#[test]
fn drop_table_if_exists_no_error() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    let result = engine.execute("DROP TABLE IF EXISTS nonexistent");
    assert!(result.is_ok());
}

#[test]
fn drop_table_rejects_non_table() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    let result = engine.execute("DROP VIEW something");
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("not supported"));
}

#[test]
fn alter_table_add_column_extends_schema() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine.execute("CREATE TABLE addcol (name TEXT)").unwrap();
    engine
        .execute("ALTER TABLE addcol ADD COLUMN priority INTEGER")
        .unwrap();

    // Verify column exists in materialized table
    let result = engine.execute("SELECT * FROM addcol").unwrap();
    match result {
        SqlResult::Rows { columns, .. } => {
            assert!(columns.contains(&"priority".to_string()));
        }
        _ => panic!("expected Rows"),
    }
}

#[test]
fn alter_table_add_column_infers_zone_and_allowed_values() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE altadd (name VARCHAR(100))")
        .unwrap();
    engine
        .execute("ALTER TABLE altadd ADD COLUMN status ENUM('todo','doing','done') DEFAULT 'todo'")
        .unwrap();
    engine
        .execute("ALTER TABLE altadd ADD COLUMN notes TEXT")
        .unwrap();

    let schema = engine.load_schema("altadd").unwrap();

    let status = schema.columns.iter().find(|c| c.name == "status").unwrap();
    assert_eq!(status.zone, Some(Zone::Frontmatter));
    assert_eq!(
        status.allowed_values,
        Some(vec!["todo".into(), "doing".into(), "done".into()])
    );
    assert_eq!(status.default_value.as_deref(), Some("todo"));

    let notes = schema.columns.iter().find(|c| c.name == "notes").unwrap();
    assert_eq!(notes.zone, Some(Zone::Body));
}

#[test]
fn create_table_propagates_not_null_into_required() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute(
            "CREATE TABLE notnull_create (a VARCHAR(255) NOT NULL, b INTEGER, c TEXT NOT NULL)",
        )
        .unwrap();

    let schema = engine.load_schema("notnull_create").unwrap();
    let a = schema.columns.iter().find(|c| c.name == "a").unwrap();
    let b = schema.columns.iter().find(|c| c.name == "b").unwrap();
    let c = schema.columns.iter().find(|c| c.name == "c").unwrap();

    assert!(a.required, "column a should be required (NOT NULL)");
    assert!(!b.required, "column b should be nullable");
    assert!(c.required, "column c should be required (NOT NULL)");
}

#[test]
fn alter_table_add_column_propagates_not_null_into_required() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE notnull_alter (name TEXT)")
        .unwrap();
    engine
        .execute("ALTER TABLE notnull_alter ADD COLUMN code VARCHAR(50) NOT NULL")
        .unwrap();
    engine
        .execute("ALTER TABLE notnull_alter ADD COLUMN priority INTEGER")
        .unwrap();

    let schema = engine.load_schema("notnull_alter").unwrap();
    let code = schema.columns.iter().find(|c| c.name == "code").unwrap();
    let priority = schema
        .columns
        .iter()
        .find(|c| c.name == "priority")
        .unwrap();

    assert!(code.required, "added NOT NULL column should be required");
    assert!(
        !priority.required,
        "added nullable column should not be required"
    );
}

#[test]
fn alter_table_add_column_existing_data_gets_null() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine.execute("CREATE TABLE addcol2 (name TEXT)").unwrap();
    engine
        .execute("INSERT INTO addcol2 (name) VALUES ('test')")
        .unwrap();
    engine
        .execute("ALTER TABLE addcol2 ADD COLUMN score INTEGER")
        .unwrap();

    let result = engine.execute("SELECT name, score FROM addcol2").unwrap();
    match result {
        SqlResult::Rows { rows, .. } => {
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0][0], "test");
            assert_eq!(rows[0][1], "NULL"); // NULL column
        }
        _ => panic!("expected Rows"),
    }
}

#[test]
fn alter_table_drop_column_removes_from_schema() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE dropcol (name TEXT, extra TEXT)")
        .unwrap();
    engine
        .execute("ALTER TABLE dropcol DROP COLUMN extra")
        .unwrap();

    let result = engine.execute("SELECT * FROM dropcol").unwrap();
    match result {
        SqlResult::Rows { columns, .. } => {
            assert!(!columns.contains(&"extra".to_string()));
            assert!(columns.contains(&"name".to_string()));
        }
        _ => panic!("expected Rows"),
    }
}

#[test]
fn bulk_delete_removes_matching_rows() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE bulkdel (name TEXT, status TEXT)")
        .unwrap();
    engine
        .execute("INSERT INTO bulkdel (name, status) VALUES ('a', 'done')")
        .unwrap();
    engine
        .execute("INSERT INTO bulkdel (name, status) VALUES ('b', 'todo')")
        .unwrap();
    engine
        .execute("INSERT INTO bulkdel (name, status) VALUES ('c', 'done')")
        .unwrap();

    let result = engine
        .execute("DELETE FROM bulkdel WHERE status = 'done'")
        .unwrap();
    match result {
        SqlResult::Affected(n) => assert_eq!(n, 2),
        _ => panic!("expected Affected"),
    }

    let rows = engine.execute("SELECT name FROM bulkdel").unwrap();
    match rows {
        SqlResult::Rows { rows, .. } => {
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0][0], "b");
        }
        _ => panic!("expected Rows"),
    }
}

#[test]
fn bulk_delete_all_rows_when_no_where() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine.execute("CREATE TABLE bulkdel2 (name TEXT)").unwrap();
    engine
        .execute("INSERT INTO bulkdel2 (name) VALUES ('a')")
        .unwrap();
    engine
        .execute("INSERT INTO bulkdel2 (name) VALUES ('b')")
        .unwrap();

    let result = engine.execute("DELETE FROM bulkdel2").unwrap();
    match result {
        SqlResult::Affected(n) => assert_eq!(n, 2),
        _ => panic!("expected Affected"),
    }

    let rows = engine.execute("SELECT * FROM bulkdel2").unwrap();
    match rows {
        SqlResult::Rows { rows, .. } => assert!(rows.is_empty()),
        _ => panic!("expected Rows"),
    }
}

#[test]
fn bulk_update_modifies_matching_rows() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE bulkupd (name TEXT, priority INTEGER)")
        .unwrap();
    engine
        .execute("INSERT INTO bulkupd (name, priority) VALUES ('a', 1)")
        .unwrap();
    engine
        .execute("INSERT INTO bulkupd (name, priority) VALUES ('b', 2)")
        .unwrap();
    engine
        .execute("INSERT INTO bulkupd (name, priority) VALUES ('c', 1)")
        .unwrap();

    let result = engine
        .execute("UPDATE bulkupd SET priority = 9 WHERE priority = 1")
        .unwrap();
    match result {
        SqlResult::Affected(n) => assert_eq!(n, 2),
        _ => panic!("expected Affected"),
    }

    let rows = engine
        .execute("SELECT name, priority FROM bulkupd ORDER BY name")
        .unwrap();
    match rows {
        SqlResult::Rows { rows, .. } => {
            assert_eq!(rows[0][1], "9"); // a: was 1 → 9
            assert_eq!(rows[1][1], "2"); // b: unchanged
            assert_eq!(rows[2][1], "9"); // c: was 1 → 9
        }
        _ => panic!("expected Rows"),
    }
}

#[test]
fn bulk_update_all_rows_when_no_where() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE bulkupd2 (name TEXT, flag TEXT)")
        .unwrap();
    engine
        .execute("INSERT INTO bulkupd2 (name, flag) VALUES ('a', 'old')")
        .unwrap();
    engine
        .execute("INSERT INTO bulkupd2 (name, flag) VALUES ('b', 'old')")
        .unwrap();

    let result = engine.execute("UPDATE bulkupd2 SET flag = 'new'").unwrap();
    match result {
        SqlResult::Affected(n) => assert_eq!(n, 2),
        _ => panic!("expected Affected"),
    }

    let rows = engine.execute("SELECT flag FROM bulkupd2").unwrap();
    match rows {
        SqlResult::Rows { rows, .. } => {
            assert!(rows.iter().all(|r| r[0] == "new"));
        }
        _ => panic!("expected Rows"),
    }
}

#[test]
fn alter_table_rename_column_rewrites_frontmatter() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE renamefm (status TEXT, priority INTEGER)")
        .unwrap();
    let id = match engine
        .execute("INSERT INTO renamefm (status, priority) VALUES ('active', 5)")
        .unwrap()
    {
        SqlResult::Ok(id) => id,
        _ => panic!("expected Ok"),
    };

    engine
        .execute("ALTER TABLE renamefm RENAME COLUMN priority TO importance")
        .unwrap();

    // Verify doogat file has renamed key
    let path = index.resolve_path(&id).unwrap();
    let content = repo.read_file(&path).unwrap();
    assert!(
        content.contains("importance: 5"),
        "expected renamed key in frontmatter: {content}"
    );
    assert!(
        !content.contains("priority:"),
        "old key should be gone: {content}"
    );

    // Verify materialized table has renamed column
    let result = engine.execute("SELECT importance FROM renamefm").unwrap();
    match result {
        SqlResult::Rows { rows, .. } => {
            assert_eq!(rows[0][0], "5");
        }
        _ => panic!("expected Rows"),
    }
}

#[test]
fn alter_table_rename_column_rewrites_body_heading() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    // Body zone column (TEXT, first column = body zone by default)
    engine
        .execute("CREATE TABLE renamebody (description TEXT)")
        .unwrap();
    let id = match engine
        .execute("INSERT INTO renamebody (description) VALUES ('hello world')")
        .unwrap()
    {
        SqlResult::Ok(id) => id,
        _ => panic!("expected Ok"),
    };

    engine
        .execute("ALTER TABLE renamebody RENAME COLUMN description TO summary")
        .unwrap();

    let path = index.resolve_path(&id).unwrap();
    let content = repo.read_file(&path).unwrap();
    assert!(
        content.contains("## summary"),
        "expected renamed heading: {content}"
    );
    assert!(
        !content.contains("## description"),
        "old heading should be gone: {content}"
    );
}

#[test]
fn alter_table_rename_column_rewrites_reference() {
    let (_dir, repo, index) = setup();

    // Create referenced type first
    engine_exec_ok(&repo, &index, "CREATE TABLE person (name TEXT)");
    let person_id = engine_exec_id(&repo, &index, "INSERT INTO person (name) VALUES ('Alice')");

    // Create type with reference column and insert with the person's doogat id
    engine_exec_ok(
        &repo,
        &index,
        "CREATE TABLE task (title TEXT, assignee TEXT REFERENCES person)",
    );
    let id = engine_exec_id(
        &repo,
        &index,
        &format!("INSERT INTO task (title, assignee) VALUES ('Fix bug', '{person_id}')"),
    );

    let mut engine = SqlEngine::new(&index, &repo);
    engine
        .execute("ALTER TABLE task RENAME COLUMN assignee TO owner")
        .unwrap();

    let path = index.resolve_path(&id).unwrap();
    let content = repo.read_file(&path).unwrap();
    assert!(
        content.contains("- owner::"),
        "expected renamed reference key: {content}"
    );
    assert!(
        !content.contains("- assignee::"),
        "old reference key should be gone: {content}"
    );
}

#[test]
fn alter_table_rename_column_rejects_collision() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);
    engine
        .execute("CREATE TABLE coltest (name TEXT, status TEXT)")
        .unwrap();

    let err = engine
        .execute("ALTER TABLE coltest RENAME COLUMN name TO status")
        .unwrap_err();
    assert!(
        err.to_string().contains("column already exists: status"),
        "{err}"
    );
}

#[test]
fn alter_table_rename_to_renames_empty_typedef() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);
    engine
        .execute("CREATE TABLE renamesrc (title TEXT)")
        .unwrap();

    let result = engine
        .execute("ALTER TABLE renamesrc RENAME TO renamedst")
        .unwrap();
    let msg = match &result {
        SqlResult::Ok(m) => m.clone(),
        _ => panic!("unexpected result variant: {result:?}"),
    };
    assert!(msg.contains("renamesrc") && msg.contains("renamedst"), "{msg}");

    // Old name no longer resolves; new name does.
    assert!(engine.load_typedef_location("renamesrc").is_err());
    assert!(engine.load_typedef_location("renamedst").is_ok());
}

#[test]
fn alter_table_rename_to_rejects_invalid_target_name() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);
    engine
        .execute("CREATE TABLE renamesrc2 (title TEXT)")
        .unwrap();

    let err = engine
        .execute("ALTER TABLE renamesrc2 RENAME TO doogats")
        .unwrap_err();
    assert!(err.to_string().contains("reserved"), "{err}");
}

#[test]
fn alter_table_rename_to_rewrites_type_field_and_renames_table() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);
    engine
        .execute("CREATE TABLE rdata (title VARCHAR(64))")
        .unwrap();
    engine
        .execute("INSERT INTO rdata (id, title) VALUES ('20260428000001', 'one')")
        .unwrap();
    engine
        .execute("INSERT INTO rdata (id, title) VALUES ('20260428000002', 'two')")
        .unwrap();

    engine
        .execute("ALTER TABLE rdata RENAME TO rmoved")
        .unwrap();

    // Materialized table now lives under the new name and rows survived.
    let count: i64 = engine
        .index
        .sql_conn()
        .query_row("SELECT COUNT(*) FROM rmoved", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 2);

    // Old materialized table is gone.
    let exists: i64 = engine
        .index
        .sql_conn()
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='rdata'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(exists, 0);

    // After rename, the index rows for both data records reflect the new type.
    let typed_count: i64 = engine
        .index
        .sql_conn()
        .query_row(
            "SELECT COUNT(*) FROM doogats WHERE type = 'rmoved'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(typed_count, 2, "two data rows should now have type=rmoved");

    let stale_count: i64 = engine
        .index
        .sql_conn()
        .query_row(
            "SELECT COUNT(*) FROM doogats WHERE type = 'rdata'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(stale_count, 0, "no data rows should retain the old type");

    // Read one data file from its on-disk path and confirm `type:` frontmatter was rewritten.
    let path: String = engine
        .index
        .sql_conn()
        .query_row(
            "SELECT path FROM doogats WHERE type = 'rmoved' LIMIT 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let abs_path = repo.path.join(&path);
    let content = std::fs::read_to_string(&abs_path).expect("read data file");
    assert!(content.contains("type: rmoved"), "{content}");
    assert!(!content.contains("type: rdata\n"), "stale type in {content}");
}

#[test]
fn alter_table_rename_to_rejects_target_already_exists() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);
    engine.execute("CREATE TABLE rconflict_a (title TEXT)").unwrap();
    engine.execute("CREATE TABLE rconflict_b (title TEXT)").unwrap();

    let err = engine
        .execute("ALTER TABLE rconflict_a RENAME TO rconflict_b")
        .unwrap_err();
    assert!(
        err.to_string().contains("already exists"),
        "{err}"
    );

    // Both typedefs intact.
    assert!(engine.load_typedef_location("rconflict_a").is_ok());
    assert!(engine.load_typedef_location("rconflict_b").is_ok());
}

#[test]
fn alter_table_rename_to_rejects_unknown_source() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    let err = engine
        .execute("ALTER TABLE rmissing RENAME TO rmissing_new")
        .unwrap_err();
    assert!(
        err.to_string().contains("table not found"),
        "{err}"
    );
}

#[test]
fn alter_table_rename_to_rejects_invalid_identifier_shapes() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);
    engine.execute("CREATE TABLE rinvalid (title TEXT)").unwrap();

    let err_typedef = engine
        .execute("ALTER TABLE rinvalid RENAME TO _typedef")
        .unwrap_err();
    assert!(err_typedef.to_string().contains("reserved"), "{err_typedef}");

    let err_ddb = engine
        .execute("ALTER TABLE rinvalid RENAME TO _ddb_links")
        .unwrap_err();
    assert!(err_ddb.to_string().contains("reserved"), "{err_ddb}");

    // Source is intact after each rejection.
    assert!(engine.load_typedef_location("rinvalid").is_ok());
}

#[test]
fn mysql_rename_table_alias_rejected_with_clear_message() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);
    engine
        .execute("CREATE TABLE rmysql (title TEXT)")
        .unwrap();

    let err = engine
        .execute("RENAME TABLE rmysql TO rmysql_renamed")
        .unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("RENAME TABLE not supported"),
        "expected explicit rejection, got: {msg}"
    );
    assert!(
        msg.contains("ALTER TABLE"),
        "rejection should hint at the supported form: {msg}"
    );
    assert!(!msg.contains("internal"), "should not surface internal error: {msg}");
}

#[test]
fn rebuild_drops_orphan_materialized_tables() {
    // Simulates the partial-rename crash case: a SQLite table exists with no
    // matching typedef. The rebuild path should drop it.
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);
    engine
        .execute("CREATE TABLE rorphan_keep (title TEXT)")
        .unwrap();
    drop(engine);

    // Inject an orphan table by hand.
    index
        .sql_conn()
        .execute("CREATE TABLE \"rorphan_stale\" (id TEXT, title TEXT)", [])
        .unwrap();
    let exists_before: i64 = index
        .sql_conn()
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='rorphan_stale'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(exists_before, 1);

    // Trigger rebuild via materialize_all_types — the rebuild path called
    // automatically when typedefs change.
    let (_, _) = index.materialize_all_types(&repo).unwrap();

    let exists_after: i64 = index
        .sql_conn()
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='rorphan_stale'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(exists_after, 0, "orphan table should have been dropped");

    // Legitimate typedef table is still present.
    let kept: i64 = index
        .sql_conn()
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='rorphan_keep'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(kept, 1);
}

#[test]
fn alter_table_rename_to_rewrites_references_in_other_typedefs() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);
    engine
        .execute("CREATE TABLE rrefsrc (title VARCHAR(64))")
        .unwrap();
    engine
        .execute(
            "CREATE TABLE rrefdst (title VARCHAR(64), parent VARCHAR(14) REFERENCES rrefsrc(id))",
        )
        .unwrap();

    engine
        .execute("ALTER TABLE rrefsrc RENAME TO rrefnew")
        .unwrap();

    // Reload the rrefdst typedef and assert its parent column now references rrefnew.
    let schema = engine.load_schema("rrefdst").unwrap();
    let parent = schema
        .columns
        .iter()
        .find(|c| c.name == "parent")
        .expect("parent column missing");
    assert_eq!(parent.references.as_deref(), Some("rrefnew"));
}

/// Count git commits by walking the HEAD log.
fn count_commits(repo: &GitRepo) -> usize {
    let git = git2::Repository::open(&repo.path).unwrap();
    let mut revwalk = git.revwalk().unwrap();
    revwalk.push_head().unwrap();
    revwalk.count()
}

#[test]
fn begin_commit_batches_writes() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);
    engine.execute("CREATE TABLE items (name TEXT)").unwrap();
    let before = count_commits(&repo);

    engine.execute("BEGIN").unwrap();
    engine
        .execute("INSERT INTO items (name) VALUES ('a')")
        .unwrap();
    engine
        .execute("INSERT INTO items (name) VALUES ('b')")
        .unwrap();
    engine.execute("COMMIT").unwrap();

    let after = count_commits(&repo);
    // Should produce exactly one additional git commit for the transaction
    assert_eq!(
        after - before,
        1,
        "expected single git commit for transaction"
    );

    let rows = index
        .query_raw("SELECT name FROM items ORDER BY name")
        .unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0][0], "a");
    assert_eq!(rows[1][0], "b");
}

#[test]
fn begin_rollback_discards() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);
    engine.execute("CREATE TABLE items (name TEXT)").unwrap();
    let before = count_commits(&repo);

    engine.execute("BEGIN").unwrap();
    engine
        .execute("INSERT INTO items (name) VALUES ('gone')")
        .unwrap();
    engine.execute("ROLLBACK").unwrap();

    let after = count_commits(&repo);
    assert_eq!(after, before, "rollback should not produce git commits");

    let rows = index.query_raw("SELECT name FROM items").unwrap();
    assert!(rows.is_empty(), "rollback should discard inserts");
}

#[test]
fn read_your_writes_within_txn() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);
    engine.execute("CREATE TABLE items (name TEXT)").unwrap();

    engine.execute("BEGIN").unwrap();
    engine
        .execute("INSERT INTO items (name) VALUES ('visible')")
        .unwrap();

    // SELECT within the same transaction should see the inserted row
    let result = engine.execute("SELECT name FROM items").unwrap();
    match result {
        SqlResult::Rows { rows, .. } => {
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0][0], "visible");
        }
        _ => panic!("expected Rows"),
    }

    engine.execute("COMMIT").unwrap();
}

#[test]
fn drop_auto_rollback() {
    let (_dir, repo, index) = setup();
    {
        let mut engine = SqlEngine::new(&index, &repo);
        engine.execute("CREATE TABLE items (name TEXT)").unwrap();
        engine.execute("BEGIN").unwrap();
        engine
            .execute("INSERT INTO items (name) VALUES ('orphan')")
            .unwrap();
        // engine dropped here without COMMIT
    }

    // After drop, SQLite savepoint should be rolled back
    let rows = index.query_raw("SELECT name FROM items").unwrap();
    assert!(rows.is_empty(), "drop should auto-rollback");
}

#[test]
fn nested_begin_rejected() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine.execute("BEGIN").unwrap();
    let err = engine.execute("BEGIN").unwrap_err();
    assert!(err.to_string().contains("already active"), "{err}");
    engine.execute("ROLLBACK").unwrap();
}

#[test]
fn insert_then_update_within_txn() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);
    engine.execute("CREATE TABLE items (name TEXT)").unwrap();

    engine.execute("BEGIN").unwrap();
    let id = match engine
        .execute("INSERT INTO items (name) VALUES ('old')")
        .unwrap()
    {
        SqlResult::Ok(id) => id,
        _ => panic!("expected Ok"),
    };
    engine
        .execute(&format!("UPDATE items SET name = 'new' WHERE id = '{id}'"))
        .unwrap();
    engine.execute("COMMIT").unwrap();

    let rows = index.query_raw("SELECT name FROM items").unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0][0], "new");

    // Verify git also has the updated content
    let path = index.resolve_path(&id).unwrap();
    let content = repo.read_file(&path).unwrap();
    assert!(content.contains("new"), "git should have updated content");
}

#[test]
fn insert_then_delete_within_txn() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);
    engine.execute("CREATE TABLE items (name TEXT)").unwrap();

    engine.execute("BEGIN").unwrap();
    let id = match engine
        .execute("INSERT INTO items (name) VALUES ('temp')")
        .unwrap()
    {
        SqlResult::Ok(id) => id,
        _ => panic!("expected Ok"),
    };
    engine
        .execute(&format!("DELETE FROM items WHERE id = '{id}'"))
        .unwrap();
    engine.execute("COMMIT").unwrap();

    let rows = index.query_raw("SELECT name FROM items").unwrap();
    assert!(rows.is_empty(), "insert+delete should cancel out");
}

#[test]
fn error_preserves_active_txn() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);
    engine.execute("CREATE TABLE items (name TEXT)").unwrap();

    engine.execute("BEGIN").unwrap();
    engine
        .execute("INSERT INTO items (name) VALUES ('keep')")
        .unwrap();

    // Trigger an error (insert into nonexistent table)
    let err = engine.execute("INSERT INTO nonexistent (name) VALUES ('fail')");
    assert!(err.is_err());

    // Transaction should still be active — can still ROLLBACK
    engine.execute("ROLLBACK").unwrap();

    let rows = index.query_raw("SELECT name FROM items").unwrap();
    assert!(rows.is_empty(), "rollback after error should discard all");
}

#[test]
fn insert_delete_read_content_returns_not_found() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);
    engine.execute("CREATE TABLE items (name TEXT)").unwrap();

    engine.execute("BEGIN").unwrap();
    let id = match engine
        .execute("INSERT INTO items (name) VALUES ('ghost')")
        .unwrap()
    {
        SqlResult::Ok(id) => id,
        _ => panic!("expected Ok"),
    };

    // Delete within same txn
    engine
        .execute(&format!("DELETE FROM items WHERE id = '{id}'"))
        .unwrap();

    // SELECT should return no rows (SQLite already removed)
    let result = engine.execute("SELECT name FROM items").unwrap();
    match result {
        SqlResult::Rows { rows, .. } => {
            assert!(rows.is_empty(), "deleted row should not appear in SELECT")
        }
        _ => panic!("expected Rows"),
    }

    engine.execute("COMMIT").unwrap();

    // Git should have no commit for cancelled write+delete
    let rows = index.query_raw("SELECT name FROM items").unwrap();
    assert!(rows.is_empty());
}

#[test]
fn two_inserts_one_deleted_commits_survivor() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);
    engine.execute("CREATE TABLE items (name TEXT)").unwrap();

    engine.execute("BEGIN").unwrap();

    let id1 = match engine
        .execute("INSERT INTO items (name) VALUES ('keep')")
        .unwrap()
    {
        SqlResult::Ok(id) => id,
        _ => panic!("expected Ok"),
    };
    std::thread::sleep(std::time::Duration::from_secs(1));
    let id2 = match engine
        .execute("INSERT INTO items (name) VALUES ('remove')")
        .unwrap()
    {
        SqlResult::Ok(id) => id,
        _ => panic!("expected Ok"),
    };

    engine
        .execute(&format!("DELETE FROM items WHERE id = '{id2}'"))
        .unwrap();
    engine.execute("COMMIT").unwrap();

    // Only first insert should survive
    let rows = index.query_raw("SELECT name FROM items").unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0][0], "keep");

    // Verify git file exists for survivor (no folder: true → flat path)
    assert!(repo.read_file(&format!("ddb/{id1}.md")).is_ok());
    // Deleted doogat should not be in git (it was buffer-only)
    assert!(repo.read_file(&format!("ddb/{id2}.md")).is_err());
}

#[test]
fn create_index_rejected_with_reason() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);
    let err = engine
        .execute("CREATE INDEX idx ON doogats(title)")
        .unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("CREATE INDEX not supported"), "{msg}");
}

#[test]
fn create_index_if_not_exists_accepted_as_no_op_prd_00129() {
    // PRD 00129 §3b: `CREATE INDEX IF NOT EXISTS` is tolerated as a no-op
    // so apps with legacy startup migrations (jink today) keep booting
    // after upgrade. The intended path is `UNIQUE(...)` in the typedef.
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);
    engine
        .execute("CREATE TABLE link (title TEXT, url VARCHAR(255))")
        .unwrap();
    let result = engine
        .execute("CREATE INDEX IF NOT EXISTS idx_link_url ON link(url)")
        .expect("CREATE INDEX IF NOT EXISTS should be tolerated as no-op");
    let msg = format!("{result:?}");
    assert!(
        msg.contains("ignored") && msg.contains("idx_link_url"),
        "expected no-op message, got: {msg}"
    );
}

#[test]
fn create_unique_index_if_not_exists_accepted_as_no_op_prd_00129() {
    // jink's actual migration is `CREATE UNIQUE INDEX IF NOT EXISTS` —
    // verify the unique flavor takes the same no-op path.
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);
    engine
        .execute(
            "CREATE TABLE \"category-membership\" (title TEXT, link VARCHAR(255), category VARCHAR(255))",
        )
        .unwrap();
    let result = engine
        .execute(
            "CREATE UNIQUE INDEX IF NOT EXISTS idx_membership_unique ON \"category-membership\"(link, category)",
        )
        .expect("CREATE UNIQUE INDEX IF NOT EXISTS should be tolerated as no-op");
    let msg = format!("{result:?}");
    assert!(msg.contains("ignored"), "expected no-op message, got: {msg}");
}

// ── PRD 00129 §3a + §6: typedef UNIQUE produces UNIQUE_VIOLATION code ──

#[test]
fn unique_constraint_failure_emits_structured_unique_violation_prd_00129() {
    // Per PRD 00129 §3a + §6, a typedef-declared UNIQUE that fires at
    // insert_materialized_row produces DoogatError::Structured with
    // code UNIQUE_VIOLATION and the {table, columns, values} extension
    // fields. Pre-PRD 00129 the error came through as
    // DoogatError::SqlEngine(<sqlite message>) with no code.
    use crate::error::{DoogatError, ErrorValue};
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);
    engine
        .execute(
            "CREATE TABLE membership (title TEXT, link_id VARCHAR(255) NOT NULL, cat VARCHAR(255) NOT NULL, UNIQUE(link_id, cat))",
        )
        .unwrap();
    engine
        .execute("INSERT INTO membership (title, link_id, cat) VALUES ('a', 'L1', 'C1')")
        .unwrap();
    let err = engine
        .execute("INSERT INTO membership (title, link_id, cat) VALUES ('b', 'L1', 'C1')")
        .expect_err("duplicate must reject");
    match err {
        DoogatError::Structured {
            code,
            message,
            context,
        } => {
            assert_eq!(code, "UNIQUE_VIOLATION");
            assert!(
                message.contains("UNIQUE constraint failed: membership.link_id"),
                "message preserves SQLite wording: {message}"
            );
            let table = context.iter().find(|(k, _)| k == "table").unwrap();
            assert_eq!(table.1, ErrorValue::String("membership".into()));
            let cols = context.iter().find(|(k, _)| k == "columns").unwrap();
            assert_eq!(
                cols.1,
                ErrorValue::List(vec!["link_id".into(), "cat".into()])
            );
            let vals = context.iter().find(|(k, _)| k == "values").unwrap();
            assert_eq!(vals.1, ErrorValue::List(vec!["L1".into(), "C1".into()]));
        }
        other => panic!("expected Structured UNIQUE_VIOLATION, got: {other:?}"),
    }
}

// ── PRD 00129 §2: ON DELETE action parsing ──

#[test]
fn create_table_references_on_delete_cascade_parses_and_persists_prd_00129() {
    // The typed DDL accepts `REFERENCES t(id) ON DELETE CASCADE` and the
    // action is stored on the typedef column. Default (clause absent) is
    // RESTRICT — the existing #10 behavior.
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE link (title TEXT, url VARCHAR(255))")
        .unwrap();
    engine
        .execute(
            "CREATE TABLE membership (title TEXT, \
             link VARCHAR(255) NOT NULL REFERENCES link(id) ON DELETE CASCADE, \
             cat VARCHAR(255) NOT NULL REFERENCES link(id))",
        )
        .unwrap();

    let schemas = index.load_all_typedefs(&repo);
    let membership = schemas
        .get("membership")
        .expect("membership typedef should be materialized");

    let link_col = membership
        .columns
        .iter()
        .find(|c| c.name == "link")
        .expect("link column present");
    assert_eq!(
        link_col.on_delete,
        crate::types::OnDeleteAction::Cascade,
        "explicit ON DELETE CASCADE must store Cascade"
    );

    let cat_col = membership
        .columns
        .iter()
        .find(|c| c.name == "cat")
        .expect("cat column present");
    assert_eq!(
        cat_col.on_delete,
        crate::types::OnDeleteAction::Restrict,
        "omitted ON DELETE clause must default to Restrict"
    );
}

#[test]
fn create_table_references_on_delete_restrict_explicit_parses_prd_00129() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);
    engine
        .execute("CREATE TABLE link (title TEXT, url VARCHAR(255))")
        .unwrap();
    engine
        .execute(
            "CREATE TABLE blocker (title TEXT, \
             link VARCHAR(255) NOT NULL REFERENCES link(id) ON DELETE RESTRICT)",
        )
        .unwrap();
    let schemas = index.load_all_typedefs(&repo);
    let blocker = schemas.get("blocker").unwrap();
    let link_col = blocker.columns.iter().find(|c| c.name == "link").unwrap();
    assert_eq!(
        link_col.on_delete,
        crate::types::OnDeleteAction::Restrict,
        "explicit RESTRICT must store Restrict"
    );
}

#[test]
fn create_table_references_on_delete_set_null_rejected_prd_00129() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);
    engine
        .execute("CREATE TABLE link (title TEXT, url VARCHAR(255))")
        .unwrap();
    let err = engine
        .execute(
            "CREATE TABLE bad (title TEXT, \
             link VARCHAR(255) NOT NULL REFERENCES link(id) ON DELETE SET NULL)",
        )
        .expect_err("SET NULL must reject in v1");
    let msg = format!("{err}");
    assert!(
        msg.contains("SET NULL not supported")
            && msg.contains("CASCADE | RESTRICT"),
        "expected v1-scope rejection message, got: {msg}"
    );
}

#[test]
fn create_table_references_on_update_rejected_prd_00129() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);
    engine
        .execute("CREATE TABLE link (title TEXT, url VARCHAR(255))")
        .unwrap();
    let err = engine
        .execute(
            "CREATE TABLE bad (title TEXT, \
             link VARCHAR(255) NOT NULL REFERENCES link(id) ON UPDATE CASCADE)",
        )
        .expect_err("ON UPDATE must reject in v1");
    let msg = format!("{err}");
    assert!(
        msg.contains("ON UPDATE") && msg.contains("not supported"),
        "expected ON UPDATE rejection, got: {msg}"
    );
}

#[test]
fn on_delete_action_typedef_yaml_roundtrip_prd_00129() {
    // CREATE TABLE -> typedef YAML -> schema_from_parsed roundtrip
    // preserves the ON DELETE action.
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);
    engine
        .execute("CREATE TABLE link (title TEXT, url VARCHAR(255))")
        .unwrap();
    engine
        .execute(
            "CREATE TABLE m (title TEXT, \
             link VARCHAR(255) NOT NULL REFERENCES link(id) ON DELETE CASCADE)",
        )
        .unwrap();
    let schemas = index.load_all_typedefs(&repo);
    let m = schemas.get("m").unwrap().clone();
    // Re-serialize and re-parse
    let doogat = super::builders::build_typedef_doogat(
        &crate::types::DoogatId("00000000000000".to_string()),
        &m,
    );
    let roundtripped = super::builders::schema_from_parsed(&doogat).unwrap();
    let link_col = roundtripped
        .columns
        .iter()
        .find(|c| c.name == "link")
        .unwrap();
    assert_eq!(link_col.on_delete, crate::types::OnDeleteAction::Cascade);
}

#[test]
fn plain_create_index_still_rejects_after_prd_00129() {
    // Regression: PRD 00129 §3b only relaxes `IF NOT EXISTS`; the bare
    // form continues to reject so callers learn to drop redundant
    // declarations.
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);
    engine
        .execute("CREATE TABLE link (title TEXT, url VARCHAR(255))")
        .unwrap();
    let err = engine
        .execute("CREATE UNIQUE INDEX idx_link_url ON link(url)")
        .expect_err("plain CREATE [UNIQUE] INDEX must still reject");
    let msg = format!("{err}");
    assert!(msg.contains("CREATE INDEX not supported"), "{msg}");
}

#[test]
fn create_view_rejected_with_reason() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);
    let err = engine
        .execute("CREATE VIEW v AS SELECT * FROM doogats")
        .unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("CREATE VIEW not supported"), "{msg}");
}

#[test]
fn create_trigger_rejected_with_reason() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);
    let err = engine
        .execute("CREATE TRIGGER t AFTER INSERT ON doogats FOR EACH ROW EXECUTE PROCEDURE noop()")
        .unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("CREATE TRIGGER not supported"), "{msg}");
}

#[test]
fn create_virtual_table_rejected_with_reason() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);
    let err = engine
        .execute("CREATE VIRTUAL TABLE vt USING fts5(content)")
        .unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("CREATE VIRTUAL TABLE not supported"), "{msg}");
}

#[test]
fn drop_index_rejected() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);
    let err = engine.execute("DROP INDEX idx").unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("DROP INDEX not supported"), "{msg}");
}

#[test]
fn drop_view_rejected() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);
    let err = engine.execute("DROP VIEW v").unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("DROP VIEW not supported"), "{msg}");
}

#[test]
fn insert_or_replace_rejected() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);
    engine.execute("CREATE TABLE items (name TEXT)").unwrap();
    let err = engine
        .execute("INSERT OR REPLACE INTO items (name) VALUES ('x')")
        .unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("not supported"), "{msg}");
}

#[test]
fn update_from_rejected() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);
    engine.execute("CREATE TABLE items (name TEXT)").unwrap();
    engine.execute("CREATE TABLE src (name TEXT)").unwrap();
    let err = engine
        .execute("UPDATE items SET name = src.name FROM src WHERE items.id = src.id")
        .unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("UPDATE...FROM not supported"), "{msg}");
}

// Issue #8 group E: PRD 00123 was archived as obsolete on 2026-04-11 after
// empirical verification that JOIN actually works in the current build. The
// tests below pin JOIN's working behavior and audit the four adjacent SQL
// features the PRD called out (CTE, subquery in FROM, UNION, window).

#[test]
fn select_join_returns_joined_rows_issue_8_e1() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE link_e1 (url VARCHAR(255))")
        .unwrap();
    engine
        .execute("CREATE TABLE num_e1 (count INTEGER)")
        .unwrap();
    engine
        .execute("INSERT INTO link_e1 (title, url) VALUES ('a', 'https://a.com')")
        .unwrap();
    engine
        .execute("INSERT INTO num_e1 (title, count) VALUES ('a', 1)")
        .unwrap();

    // JOIN over the title column must return the joined row.
    let joined = index
        .query_raw(
            "SELECT l.title, n.count FROM link_e1 l JOIN num_e1 n ON l.title = n.title",
        )
        .unwrap();
    assert_eq!(joined.len(), 1, "JOIN should return 1 row, got {joined:?}");
    assert_eq!(joined[0][0], "a");
    assert_eq!(joined[0][1], "1");

    // JOIN with no matching rows must return 0 rows and NOT error.
    engine
        .execute("INSERT INTO link_e1 (title, url) VALUES ('b', 'https://b.com')")
        .unwrap();
    // num_e1 still has only 'a', so the join on title='x' (non-existent) yields 0.
    let empty = index
        .query_raw(
            "SELECT l.title, n.count FROM link_e1 l JOIN num_e1 n ON l.title = 'nonexistent'",
        )
        .unwrap();
    assert_eq!(empty.len(), 0, "JOIN with no matches should return 0 rows");
}

#[test]
fn select_cte_current_behavior_issue_8() {
    // Audit: pin whatever the engine does with a simple CTE. If this changes,
    // the test needs updating and we need to decide whether the new behavior
    // is desirable.
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);
    engine
        .execute("CREATE TABLE cte_probe_t (label VARCHAR(255))")
        .unwrap();
    engine
        .execute("INSERT INTO cte_probe_t (title, label) VALUES ('row', 'x')")
        .unwrap();
    let result = index
        .query_raw("WITH w AS (SELECT label FROM cte_probe_t) SELECT label FROM w");
    // Whatever the current behavior is, pin it. A regression that flips Ok to
    // Err or changes the row count will fail this test.
    match result {
        Ok(rows) => {
            assert_eq!(rows.len(), 1, "CTE current behavior: expected 1 row, got {rows:?}");
            assert_eq!(rows[0][0], "x");
        }
        Err(e) => panic!(
            "CTE previously worked (pinned by select_cte_current_behavior_issue_8); \
             now errors: {e}. If CTEs were intentionally disabled, update this test \
             to assert the rejection message."
        ),
    }
}

#[test]
fn select_subquery_in_from_current_behavior_issue_8() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);
    engine
        .execute("CREATE TABLE subq_probe (label VARCHAR(255))")
        .unwrap();
    engine
        .execute("INSERT INTO subq_probe (title, label) VALUES ('row', 'y')")
        .unwrap();
    let result =
        index.query_raw("SELECT t.label FROM (SELECT label FROM subq_probe) t");
    match result {
        Ok(rows) => {
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0][0], "y");
        }
        Err(e) => panic!(
            "subquery-in-FROM previously worked; now errors: {e}. If intentionally \
             disabled, update this test to assert the rejection message."
        ),
    }
}

#[test]
fn select_union_current_behavior_issue_8() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);
    engine
        .execute("CREATE TABLE union_a (label VARCHAR(255))")
        .unwrap();
    engine
        .execute("CREATE TABLE union_b (label VARCHAR(255))")
        .unwrap();
    engine
        .execute("INSERT INTO union_a (title, label) VALUES ('a', 'one')")
        .unwrap();
    engine
        .execute("INSERT INTO union_b (title, label) VALUES ('b', 'two')")
        .unwrap();
    let result = index.query_raw("SELECT label FROM union_a UNION SELECT label FROM union_b");
    match result {
        Ok(rows) => {
            assert_eq!(rows.len(), 2, "UNION should return 2 distinct rows, got {rows:?}");
        }
        Err(e) => panic!(
            "UNION previously worked; now errors: {e}. If intentionally disabled, \
             update this test to assert the rejection message."
        ),
    }
}

#[test]
fn select_window_function_current_behavior_issue_8() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);
    engine
        .execute("CREATE TABLE win_probe (label VARCHAR(255))")
        .unwrap();
    engine
        .execute("INSERT INTO win_probe (title, label) VALUES ('one', 'a')")
        .unwrap();
    engine
        .execute("INSERT INTO win_probe (title, label) VALUES ('two', 'b')")
        .unwrap();
    let result =
        index.query_raw("SELECT label, ROW_NUMBER() OVER (ORDER BY label) FROM win_probe");
    match result {
        Ok(rows) => {
            assert_eq!(rows.len(), 2, "window fn should return 2 rows, got {rows:?}");
        }
        Err(e) => panic!(
            "window function previously worked; now errors: {e}. If intentionally \
             disabled, update this test to assert the rejection message."
        ),
    }
}

#[test]
fn delete_from_hyphenated_table() {
    let (_dir, repo, index) = setup();
    engine_exec_ok(&repo, &index, "CREATE TABLE \"my-items\" (name TEXT)");
    let id = engine_exec_id(
        &repo,
        &index,
        r#"INSERT INTO "my-items" (name) VALUES ('test')"#,
    );
    let mut engine = SqlEngine::new(&index, &repo);
    let result = engine
        .execute(&format!(r#"DELETE FROM "my-items" WHERE id = '{id}'"#))
        .unwrap();
    match result {
        SqlResult::Affected(n) => assert_eq!(n, 1),
        _ => panic!("expected Affected"),
    }
}

#[test]
fn references_to_hyphenated_table() {
    let (_dir, repo, index) = setup();
    engine_exec_ok(&repo, &index, "CREATE TABLE \"my-people\" (name TEXT)");
    engine_exec_ok(
        &repo,
        &index,
        r#"CREATE TABLE tasks (title TEXT, assignee TEXT REFERENCES "my-people")"#,
    );
    // Verify the typedef stored unquoted reference target
    let mut engine = SqlEngine::new(&index, &repo);
    let schema = engine.load_schema("tasks").unwrap();
    let ref_col = schema
        .columns
        .iter()
        .find(|c| c.name == "assignee")
        .expect("assignee column");
    assert_eq!(
        ref_col.references.as_deref(),
        Some("my-people"),
        "reference target should be unquoted"
    );
}

#[test]
fn select_still_passes_through() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);
    let result = engine.execute("SELECT 1 AS val").unwrap();
    match result {
        SqlResult::Rows { columns, rows, .. } => {
            assert_eq!(columns, vec!["val"]);
            assert_eq!(rows.len(), 1);
        }
        _ => panic!("expected Rows"),
    }
}

#[test]
fn schema_roundtrips_title_template_and_origin() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE widgets (name VARCHAR(100), weight REAL)")
        .unwrap();

    // Load the typedef, patch in title_template and origin, rewrite, reload
    let (_td_id, td_path) = engine.load_typedef_location("widgets").unwrap();
    let content = repo.read_file(&td_path).unwrap();
    let mut parsed = parser::parse(&content, &td_path).unwrap();
    parsed.meta.extra.insert(
        "title_template".to_string(),
        Value::String("name-widget".into()),
    );
    parsed
        .meta
        .extra
        .insert("origin".to_string(), Value::String("prd-00030".into()));
    let new_content = parser::serialize(&parsed);
    repo.commit_file(&td_path, &new_content, "add title_template and origin")
        .unwrap();
    let reparsed = parser::parse(&new_content, &td_path).unwrap();
    index.index_doogat(&reparsed).unwrap();

    let schema = engine.load_schema("widgets").unwrap();
    assert_eq!(schema.title_template.as_deref(), Some("name-widget"));
    assert_eq!(schema.origin.as_deref(), Some("prd-00030"));
}

#[test]
fn zone_inference_by_sql_type() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute(
            "CREATE TABLE items (\
                 short_name VARCHAR(100), \
                 description TEXT, \
                 priority INTEGER, \
                 active BOOLEAN, \
                 score REAL, \
                 bio MEDIUMTEXT\
                 )",
        )
        .unwrap();

    let schema = engine.load_schema("items").unwrap();
    let zone_of = |name: &str| -> Zone {
        schema
            .columns
            .iter()
            .find(|c| c.name == name)
            .unwrap()
            .zone
            .clone()
            .unwrap()
    };

    assert_eq!(zone_of("short_name"), Zone::Frontmatter); // VARCHAR(100) → frontmatter
    assert_eq!(zone_of("description"), Zone::Body); // TEXT → body
    assert_eq!(zone_of("priority"), Zone::Frontmatter); // INTEGER → frontmatter
    assert_eq!(zone_of("active"), Zone::Frontmatter); // BOOLEAN → frontmatter
    assert_eq!(zone_of("score"), Zone::Frontmatter); // REAL → frontmatter
    assert_eq!(zone_of("bio"), Zone::Body); // MEDIUMTEXT → body
}

#[test]
fn varchar_255_boundary() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE boundary (short VARCHAR(255), long VARCHAR(256))")
        .unwrap();

    let schema = engine.load_schema("boundary").unwrap();
    let short = schema.columns.iter().find(|c| c.name == "short").unwrap();
    let long = schema.columns.iter().find(|c| c.name == "long").unwrap();

    assert_eq!(short.zone, Some(Zone::Frontmatter));
    assert_eq!(short.data_type, "VARCHAR(255)");
    assert_eq!(long.zone, Some(Zone::Body));
    assert_eq!(long.data_type, "VARCHAR(256)");
}

#[test]
fn char_types_frontmatter() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE chars (code CHAR(10), tiny TINYTEXT)")
        .unwrap();

    let schema = engine.load_schema("chars").unwrap();
    let code = schema.columns.iter().find(|c| c.name == "code").unwrap();
    let tiny = schema.columns.iter().find(|c| c.name == "tiny").unwrap();

    assert_eq!(code.zone, Some(Zone::Frontmatter));
    assert_eq!(code.data_type, "CHAR(10)");
    assert_eq!(tiny.zone, Some(Zone::Frontmatter));
    assert_eq!(tiny.data_type, "TINYTEXT");
}

#[test]
fn enum_creates_allowed_values() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
            .execute(
                "CREATE TABLE tasks (summary VARCHAR(200), status ENUM('todo','doing','done') DEFAULT 'todo')",
            )
            .unwrap();

    let schema = engine.load_schema("tasks").unwrap();
    let status = schema.columns.iter().find(|c| c.name == "status").unwrap();

    assert_eq!(status.zone, Some(Zone::Frontmatter));
    assert_eq!(
        status.allowed_values,
        Some(vec!["todo".into(), "doing".into(), "done".into()])
    );
    assert_eq!(status.default_value.as_deref(), Some("todo"));
}

#[test]
fn set_creates_allowed_values() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE prefs (tags SET('x','y','z'))")
        .unwrap();

    let schema = engine.load_schema("prefs").unwrap();
    let tags = schema.columns.iter().find(|c| c.name == "tags").unwrap();

    assert_eq!(tags.zone, Some(Zone::Frontmatter));
    assert_eq!(
        tags.allowed_values,
        Some(vec!["x".into(), "y".into(), "z".into()])
    );
}

#[test]
fn blob_types_body() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE binaries (data BLOB, big MEDIUMBLOB, huge LONGBLOB)")
        .unwrap();

    let schema = engine.load_schema("binaries").unwrap();
    for col_name in &["data", "big", "huge"] {
        let col = schema
            .columns
            .iter()
            .find(|c| c.name == *col_name)
            .unwrap_or_else(|| panic!("missing column {col_name}"));
        assert_eq!(col.zone, Some(Zone::Body), "{col_name} should be body zone");
    }
}

#[test]
fn data_type_to_string_preserves_sizes() {
    use sqlparser::ast::{CharacterLength, DataType};

    let cases = vec![
        (
            DataType::Varchar(Some(CharacterLength::IntegerLength {
                length: 100,
                unit: None,
            })),
            "VARCHAR(100)",
        ),
        (DataType::Varchar(None), "VARCHAR"),
        (
            DataType::Char(Some(CharacterLength::IntegerLength {
                length: 1,
                unit: None,
            })),
            "CHAR(1)",
        ),
        (DataType::Char(None), "CHAR"),
        (DataType::Text, "TEXT"),
        (DataType::TinyText, "TINYTEXT"),
        (DataType::MediumText, "MEDIUMTEXT"),
        (DataType::LongText, "LONGTEXT"),
        (DataType::Blob(None), "BLOB"),
        (DataType::TinyBlob, "TINYBLOB"),
        (DataType::MediumBlob, "MEDIUMBLOB"),
        (DataType::LongBlob, "LONGBLOB"),
        (DataType::Boolean, "BOOLEAN"),
        (DataType::Integer(None), "INTEGER"),
        (DataType::Real, "REAL"),
    ];

    for (dt, expected) in cases {
        assert_eq!(data_type_to_string(&dt), expected, "for {dt:?}");
    }
}

#[test]
fn alter_table_set_zone() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE ztest (description TEXT, priority INTEGER)")
        .unwrap();

    // TEXT defaults to body; change to frontmatter
    engine
        .execute("ALTER TABLE ztest SET ZONE frontmatter FOR description")
        .unwrap();

    let schema = engine.load_schema("ztest").unwrap();
    let desc = schema
        .columns
        .iter()
        .find(|c| c.name == "description")
        .unwrap();
    assert_eq!(desc.zone, Some(Zone::Frontmatter));
}

#[test]
fn alter_table_set_zone_invalid_column() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine.execute("CREATE TABLE ztest2 (name TEXT)").unwrap();
    let err = engine
        .execute("ALTER TABLE ztest2 SET ZONE frontmatter FOR nonexistent")
        .unwrap_err();
    assert!(format!("{err}").contains("column not found"));
}

#[test]
fn alter_table_set_zone_to_reference() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE ztest3 (link VARCHAR(100))")
        .unwrap();
    engine
        .execute("ALTER TABLE ztest3 SET ZONE reference FOR link")
        .unwrap();

    let schema = engine.load_schema("ztest3").unwrap();
    let link = schema.columns.iter().find(|c| c.name == "link").unwrap();
    assert_eq!(link.zone, Some(Zone::Reference));
}

#[test]
fn alter_table_set_zone_case_insensitive() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine.execute("CREATE TABLE ztest4 (notes TEXT)").unwrap();
    engine
        .execute("alter table ztest4 set zone FRONTMATTER for notes")
        .unwrap();

    let schema = engine.load_schema("ztest4").unwrap();
    let notes = schema.columns.iter().find(|c| c.name == "notes").unwrap();
    assert_eq!(notes.zone, Some(Zone::Frontmatter));
}

#[test]
fn alter_table_set_zone_rematerializes() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE ztest5 (url VARCHAR(100), priority INTEGER)")
        .unwrap();

    // Insert with url in frontmatter zone (VARCHAR(100) → frontmatter)
    engine
        .execute("INSERT INTO ztest5 (url, priority) VALUES ('https://example.com', 1)")
        .unwrap();

    // Change url to body — this triggers rematerialization
    engine
        .execute("ALTER TABLE ztest5 SET ZONE body FOR url")
        .unwrap();

    // Materialized table should still exist (rematerialize succeeded)
    let result = engine.execute("SELECT COUNT(*) FROM ztest5").unwrap();
    match result {
        SqlResult::Rows { rows, .. } => assert_eq!(rows[0][0], "1"),
        _ => panic!("expected Rows"),
    }
}

#[test]
fn alter_table_title_template() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE tmpl (name VARCHAR(100))")
        .unwrap();

    // SET
    engine
        .execute("ALTER TABLE tmpl SET TITLE TEMPLATE 'name-template'")
        .unwrap();
    let schema = engine.load_schema("tmpl").unwrap();
    assert_eq!(schema.title_template.as_deref(), Some("name-template"));

    // DROP
    engine
        .execute("ALTER TABLE tmpl DROP TITLE TEMPLATE")
        .unwrap();
    let schema = engine.load_schema("tmpl").unwrap();
    assert_eq!(schema.title_template, None);
}

#[test]
fn alter_table_title_template_persists() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE tmpl2 (name VARCHAR(100))")
        .unwrap();
    engine
        .execute("ALTER TABLE tmpl2 SET TITLE TEMPLATE 'my-template'")
        .unwrap();

    // Create a new engine to verify persistence
    let mut engine2 = SqlEngine::new(&index, &repo);
    let schema = engine2.load_schema("tmpl2").unwrap();
    assert_eq!(schema.title_template.as_deref(), Some("my-template"));
}

#[test]
fn alter_table_set_zone_quoted_identifiers() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE \"my-items\" (\"long-desc\" TEXT)")
        .unwrap();
    engine
        .execute("ALTER TABLE \"my-items\" SET ZONE frontmatter FOR \"long-desc\"")
        .unwrap();

    let schema = engine.load_schema("my-items").unwrap();
    let col = schema
        .columns
        .iter()
        .find(|c| c.name == "long-desc")
        .unwrap();
    assert_eq!(col.zone, Some(Zone::Frontmatter));
}

#[test]
fn insert_explicit_title_wins() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE contact (name TEXT, role VARCHAR(100))")
        .unwrap();
    let id = match engine
        .execute("INSERT INTO contact (title, name, role) VALUES ('Dr. Alice', 'Alice', 'doctor')")
        .unwrap()
    {
        SqlResult::Ok(id) => id,
        _ => panic!("expected Ok"),
    };

    let path = index.resolve_path(&id).unwrap();
    let content = repo.read_file(&path).unwrap();
    assert!(
        content.contains("title: Dr. Alice"),
        "explicit title should win: {content}"
    );
}

#[test]
fn insert_title_from_template() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE person (name VARCHAR(100), role VARCHAR(100))")
        .unwrap();
    engine
        .execute("ALTER TABLE person SET TITLE TEMPLATE 'name-role'")
        .unwrap();
    let id = match engine
        .execute("INSERT INTO person (name, role) VALUES ('Alice', 'engineer')")
        .unwrap()
    {
        SqlResult::Ok(id) => id,
        _ => panic!("expected Ok"),
    };

    let path = index.resolve_path(&id).unwrap();
    let content = repo.read_file(&path).unwrap();
    // Template doesn't have {placeholders} because of YAML quoting limitation,
    // so it uses the literal template string. Testing non-interpolated template.
    assert!(
        content.contains("title: name-role"),
        "template title: {content}"
    );
}

#[test]
fn insert_title_fallback_type_id() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    // Table with only numeric columns — no string source for title
    engine
        .execute("CREATE TABLE counter (count INTEGER, active BOOLEAN)")
        .unwrap();
    let id = match engine
        .execute("INSERT INTO counter (count, active) VALUES (42, true)")
        .unwrap()
    {
        SqlResult::Ok(id) => id,
        _ => panic!("expected Ok"),
    };

    let path = index.resolve_path(&id).unwrap();
    let content = repo.read_file(&path).unwrap();
    let expected = format!("title: counter {}", id);
    assert!(content.contains(&expected), "type+id fallback: {content}");
}

#[test]
fn insert_explicit_title_overrides_template() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE widget (name VARCHAR(100))")
        .unwrap();
    engine
        .execute("ALTER TABLE widget SET TITLE TEMPLATE 'template-name'")
        .unwrap();
    let id = match engine
        .execute("INSERT INTO widget (title, name) VALUES ('Explicit Title', 'Widget A')")
        .unwrap()
    {
        SqlResult::Ok(id) => id,
        _ => panic!("expected Ok"),
    };

    let path = index.resolve_path(&id).unwrap();
    let content = repo.read_file(&path).unwrap();
    assert!(
        content.contains("title: Explicit Title"),
        "explicit overrides template: {content}"
    );
}

#[test]
fn create_table_stamps_origin_ddl() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE origtest (name VARCHAR(100))")
        .unwrap();
    let schema = engine.load_schema("origtest").unwrap();
    assert_eq!(schema.origin.as_deref(), Some("ddl"));
}

#[test]
fn origin_ddl_persists_in_yaml() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE origpersist (name VARCHAR(100))")
        .unwrap();

    // Read the typedef doogat content directly
    let (_, path) = engine.load_typedef_location("origpersist").unwrap();
    let content = repo.read_file(&path).unwrap();
    assert!(
        content.contains("origin: ddl"),
        "YAML should contain origin: ddl\n{content}"
    );
}

#[test]
fn origin_preserved_after_alter() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE origalter (name VARCHAR(100), desc TEXT)")
        .unwrap();

    // ALTER TABLE should preserve origin
    engine
        .execute("ALTER TABLE origalter SET ZONE frontmatter FOR desc")
        .unwrap();
    let schema = engine.load_schema("origalter").unwrap();
    assert_eq!(
        schema.origin.as_deref(),
        Some("ddl"),
        "origin should survive ALTER TABLE"
    );
}

#[test]
fn insert_into_junction_writes_through() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    // Create a type with a REFERENCES column
    engine
        .execute("CREATE TABLE bookmark (url TEXT, category TEXT REFERENCES category)")
        .unwrap();

    // Create a category type
    engine
        .execute("CREATE TABLE category (label VARCHAR(100))")
        .unwrap();

    // Insert a bookmark
    let bm_id = match engine
        .execute("INSERT INTO bookmark (url) VALUES ('https://example.com')")
        .unwrap()
    {
        SqlResult::Ok(id) => id,
        other => panic!("expected Ok, got {other:?}"),
    };

    // Insert a category
    let cat_id = match engine
        .execute("INSERT INTO category (label) VALUES ('tech')")
        .unwrap()
    {
        SqlResult::Ok(id) => id,
        other => panic!("expected Ok, got {other:?}"),
    };

    // INSERT into junction table
    let result = engine
            .execute(&format!(
                "INSERT INTO bookmark_category (bookmark_id, category_id) VALUES ('{bm_id}', '{cat_id}')"
            ))
            .unwrap();
    assert!(matches!(result, SqlResult::Affected(1)));

    // Verify reference line in doogat
    let path = index.resolve_path(&bm_id).unwrap();
    let content = repo.read_file(&path).unwrap();
    assert!(
        content.contains(&format!("- category:: [[{cat_id}]]")),
        "doogat should contain reference line: {content}"
    );

    // Verify junction table row
    let rows = index
        .query_raw(&format!(
            "SELECT category_id FROM bookmark_category WHERE bookmark_id = '{bm_id}'"
        ))
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0][0], cat_id);
}

#[test]
fn delete_from_junction_writes_through() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE bookmark (url TEXT, category TEXT REFERENCES category)")
        .unwrap();
    engine
        .execute("CREATE TABLE category (label VARCHAR(100))")
        .unwrap();

    let bm_id = match engine
        .execute("INSERT INTO bookmark (url) VALUES ('https://example.com')")
        .unwrap()
    {
        SqlResult::Ok(id) => id,
        other => panic!("expected Ok, got {other:?}"),
    };
    let cat_id = match engine
        .execute("INSERT INTO category (label) VALUES ('tech')")
        .unwrap()
    {
        SqlResult::Ok(id) => id,
        other => panic!("expected Ok, got {other:?}"),
    };

    // Add reference via junction INSERT
    engine
            .execute(&format!(
                "INSERT INTO bookmark_category (bookmark_id, category_id) VALUES ('{bm_id}', '{cat_id}')"
            ))
            .unwrap();

    // Verify it's there
    let path = index.resolve_path(&bm_id).unwrap();
    let content = repo.read_file(&path).unwrap();
    assert!(content.contains(&format!("- category:: [[{cat_id}]]")));

    // DELETE from junction
    let result = engine
            .execute(&format!(
                "DELETE FROM bookmark_category WHERE bookmark_id = '{bm_id}' AND category_id = '{cat_id}'"
            ))
            .unwrap();
    assert!(matches!(result, SqlResult::Affected(1)));

    // Verify reference line removed from doogat
    let content = repo.read_file(&path).unwrap();
    assert!(
        !content.contains(&format!("- category:: [[{cat_id}]]")),
        "reference line should be removed: {content}"
    );

    // Verify junction table empty
    let rows = index
        .query_raw(&format!(
            "SELECT category_id FROM bookmark_category WHERE bookmark_id = '{bm_id}'"
        ))
        .unwrap();
    assert_eq!(rows.len(), 0, "junction table should be empty");
}

#[test]
fn drop_table_cascades_junction_tables() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE bookmark (url TEXT, category TEXT REFERENCES category)")
        .unwrap();
    engine
        .execute("CREATE TABLE category (label VARCHAR(100))")
        .unwrap();

    // Verify junction table exists
    let tables = index
        .query_raw("SELECT name FROM sqlite_master WHERE type='table' AND name='bookmark_category'")
        .unwrap();
    assert_eq!(tables.len(), 1, "junction table should exist before drop");

    // DROP TABLE CASCADE
    engine.execute("DROP TABLE bookmark CASCADE").unwrap();

    // Junction table should be gone
    let tables = index
        .query_raw("SELECT name FROM sqlite_master WHERE type='table' AND name='bookmark_category'")
        .unwrap();
    assert_eq!(
        tables.len(),
        0,
        "junction table should be dropped after cascade"
    );

    // Main table should also be gone
    let tables = index
        .query_raw("SELECT name FROM sqlite_master WHERE type='table' AND name='bookmark'")
        .unwrap();
    assert_eq!(tables.len(), 0, "main table should be dropped");
}

#[test]
fn boolean_materialized_as_integer() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE flagged (pinned BOOLEAN)")
        .unwrap();
    engine
        .execute("INSERT INTO flagged (pinned) VALUES (true)")
        .unwrap();
    engine
        .execute("INSERT INTO flagged (pinned) VALUES (false)")
        .unwrap();

    // Materialized table stores as INTEGER but SELECT coerces to "true"/"false"
    let result = engine
        .execute("SELECT pinned FROM flagged WHERE pinned = 1")
        .unwrap();
    match result {
        SqlResult::Rows { rows, .. } => {
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0][0], "true");
        }
        _ => panic!("expected Rows"),
    }

    let result = engine
        .execute("SELECT pinned FROM flagged WHERE pinned = 0")
        .unwrap();
    match result {
        SqlResult::Rows { rows, .. } => {
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0][0], "false");
        }
        _ => panic!("expected Rows"),
    }
}

#[test]
fn core_fields_in_materialized_table() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE widget (name VARCHAR(100))")
        .unwrap();
    let id = match engine
        .execute("INSERT INTO widget (title, name) VALUES ('My Widget', 'sprocket')")
        .unwrap()
    {
        SqlResult::Ok(id) => id,
        _ => panic!("expected Ok"),
    };

    // Query title from type table without JOIN
    let result = engine.execute("SELECT title, name FROM widget").unwrap();
    match result {
        SqlResult::Rows { columns, rows, .. } => {
            assert_eq!(columns[0], "title");
            assert_eq!(columns[1], "name");
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0][0], "My Widget");
            assert_eq!(rows[0][1], "sprocket");
        }
        _ => panic!("expected Rows"),
    }

    // date and updated_at should be present
    let result = engine
        .execute(&format!(
            "SELECT date, updated_at FROM widget WHERE id = '{id}'"
        ))
        .unwrap();
    match result {
        SqlResult::Rows { rows, .. } => {
            assert_eq!(rows.len(), 1);
            // date comes from frontmatter, may be empty
            // updated_at should be populated by indexer
            assert!(!rows[0][1].is_empty(), "updated_at should be populated");
        }
        _ => panic!("expected Rows"),
    }
}

#[test]
fn test_cascade_junction_single_delete() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE category (label VARCHAR(100))")
        .unwrap();
    engine
        .execute("CREATE TABLE bookmark (url TEXT, category TEXT REFERENCES category)")
        .unwrap();

    let cat_id = match engine
        .execute("INSERT INTO category (label) VALUES ('tech')")
        .unwrap()
    {
        SqlResult::Ok(id) => id,
        other => panic!("expected Ok, got {other:?}"),
    };
    let bm_id = match engine
        .execute("INSERT INTO bookmark (url) VALUES ('https://example.com')")
        .unwrap()
    {
        SqlResult::Ok(id) => id,
        other => panic!("expected Ok, got {other:?}"),
    };

    // Link bookmark -> category via junction
    engine
            .execute(&format!(
                "INSERT INTO bookmark_category (bookmark_id, category_id) VALUES ('{bm_id}', '{cat_id}')"
            ))
            .unwrap();

    // Verify junction row exists
    let rows = index
        .query_raw(&format!(
            "SELECT COUNT(*) FROM bookmark_category WHERE category_id = '{cat_id}'"
        ))
        .unwrap();
    assert_eq!(rows[0][0], "1", "junction row should exist before delete");

    // Delete the category
    engine
        .execute(&format!("DELETE FROM category WHERE id = '{cat_id}'"))
        .unwrap();

    // Junction row should be cascade-deleted
    let rows = index
        .query_raw(&format!(
            "SELECT COUNT(*) FROM bookmark_category WHERE category_id = '{cat_id}'"
        ))
        .unwrap();
    assert_eq!(
        rows[0][0], "0",
        "junction row should be removed after deleting referenced category"
    );
}

#[test]
fn test_cascade_junction_multi_parent() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE category (label VARCHAR(100))")
        .unwrap();
    engine
        .execute("CREATE TABLE bookmark (url TEXT, category TEXT REFERENCES category)")
        .unwrap();

    let cat_id = match engine
        .execute("INSERT INTO category (label) VALUES ('tech')")
        .unwrap()
    {
        SqlResult::Ok(id) => id,
        other => panic!("expected Ok, got {other:?}"),
    };
    let bm1_id = match engine
        .execute("INSERT INTO bookmark (url) VALUES ('https://one.com')")
        .unwrap()
    {
        SqlResult::Ok(id) => id,
        other => panic!("expected Ok, got {other:?}"),
    };
    let bm2_id = match engine
        .execute("INSERT INTO bookmark (url) VALUES ('https://two.com')")
        .unwrap()
    {
        SqlResult::Ok(id) => id,
        other => panic!("expected Ok, got {other:?}"),
    };

    // Both bookmarks reference the same category
    engine
            .execute(&format!(
                "INSERT INTO bookmark_category (bookmark_id, category_id) VALUES ('{bm1_id}', '{cat_id}')"
            ))
            .unwrap();
    engine
            .execute(&format!(
                "INSERT INTO bookmark_category (bookmark_id, category_id) VALUES ('{bm2_id}', '{cat_id}')"
            ))
            .unwrap();

    // Verify both junction rows exist
    let rows = index
        .query_raw(&format!(
            "SELECT COUNT(*) FROM bookmark_category WHERE category_id = '{cat_id}'"
        ))
        .unwrap();
    assert_eq!(
        rows[0][0], "2",
        "both junction rows should exist before delete"
    );

    // Delete the category
    engine
        .execute(&format!("DELETE FROM category WHERE id = '{cat_id}'"))
        .unwrap();

    // Both junction rows should be removed
    let rows = index
        .query_raw(&format!(
            "SELECT COUNT(*) FROM bookmark_category WHERE category_id = '{cat_id}'"
        ))
        .unwrap();
    assert_eq!(
        rows[0][0], "0",
        "all junction rows referencing deleted category should be removed"
    );
}

#[test]
fn test_cascade_junction_selective() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE category (label VARCHAR(100))")
        .unwrap();
    engine
        .execute("CREATE TABLE bookmark (url TEXT, category TEXT REFERENCES category)")
        .unwrap();

    let cat_a = match engine
        .execute("INSERT INTO category (label) VALUES ('tech')")
        .unwrap()
    {
        SqlResult::Ok(id) => id,
        other => panic!("expected Ok, got {other:?}"),
    };
    let cat_b = match engine
        .execute("INSERT INTO category (label) VALUES ('science')")
        .unwrap()
    {
        SqlResult::Ok(id) => id,
        other => panic!("expected Ok, got {other:?}"),
    };
    let bm_id = match engine
        .execute("INSERT INTO bookmark (url) VALUES ('https://example.com')")
        .unwrap()
    {
        SqlResult::Ok(id) => id,
        other => panic!("expected Ok, got {other:?}"),
    };

    // Bookmark references both categories
    engine
            .execute(&format!(
                "INSERT INTO bookmark_category (bookmark_id, category_id) VALUES ('{bm_id}', '{cat_a}')"
            ))
            .unwrap();
    engine
            .execute(&format!(
                "INSERT INTO bookmark_category (bookmark_id, category_id) VALUES ('{bm_id}', '{cat_b}')"
            ))
            .unwrap();

    // Delete only category A
    engine
        .execute(&format!("DELETE FROM category WHERE id = '{cat_a}'"))
        .unwrap();

    // Category A's junction row should be gone
    let rows = index
        .query_raw(&format!(
            "SELECT COUNT(*) FROM bookmark_category WHERE category_id = '{cat_a}'"
        ))
        .unwrap();
    assert_eq!(
        rows[0][0], "0",
        "junction row for deleted category A should be removed"
    );

    // Category B's junction row should still exist
    let rows = index
        .query_raw(&format!(
            "SELECT COUNT(*) FROM bookmark_category WHERE category_id = '{cat_b}'"
        ))
        .unwrap();
    assert_eq!(
        rows[0][0], "1",
        "junction row for category B should be preserved"
    );
}

#[test]
fn test_cascade_junction_no_false_positives() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE category (label VARCHAR(100))")
        .unwrap();
    engine
        .execute("CREATE TABLE bookmark (url TEXT, category TEXT REFERENCES category)")
        .unwrap();

    let cat_id = match engine
        .execute("INSERT INTO category (label) VALUES ('tech')")
        .unwrap()
    {
        SqlResult::Ok(id) => id,
        other => panic!("expected Ok, got {other:?}"),
    };
    let bm_id = match engine
        .execute("INSERT INTO bookmark (url) VALUES ('https://example.com')")
        .unwrap()
    {
        SqlResult::Ok(id) => id,
        other => panic!("expected Ok, got {other:?}"),
    };

    // Link bookmark -> category
    engine
            .execute(&format!(
                "INSERT INTO bookmark_category (bookmark_id, category_id) VALUES ('{bm_id}', '{cat_id}')"
            ))
            .unwrap();

    // Delete the bookmark (the junction's owner-side parent).
    engine
        .execute(&format!("DELETE FROM bookmark WHERE id = '{bm_id}'"))
        .unwrap();

    // PRD 00137 (contract change): cascade now sweeps BOTH directions, so
    // deleting the bookmark removes the auto-junction row it owns. Pre-fix
    // this test asserted the row stayed (count == 1), enshrining the
    // owner-side leak as a "separate concern". That concern is no longer
    // separate. Querying by `category_id` still picks up the same junction
    // row that was inserted (`bookmark_id=bm, category_id=cat`), and that
    // row is now gone with the parent.
    let rows = index
        .query_raw(&format!(
            "SELECT COUNT(*) FROM bookmark_category WHERE category_id = '{cat_id}'"
        ))
        .unwrap();
    assert_eq!(
        rows[0][0], "0",
        "owner-side junction row must be cleaned when its parent doogat is deleted (PRD 00137)"
    );
}

#[test]
fn test_cascade_ref_single_removal() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE category (label VARCHAR(100))")
        .unwrap();
    engine
        .execute("CREATE TABLE bookmark (url TEXT, category TEXT REFERENCES category)")
        .unwrap();

    let cat_id = match engine
        .execute("INSERT INTO category (label) VALUES ('tech')")
        .unwrap()
    {
        SqlResult::Ok(id) => id,
        other => panic!("expected Ok, got {other:?}"),
    };
    let bm_id = match engine
        .execute("INSERT INTO bookmark (url) VALUES ('https://example.com')")
        .unwrap()
    {
        SqlResult::Ok(id) => id,
        other => panic!("expected Ok, got {other:?}"),
    };

    // Link bookmark -> category via junction (writes wikilink to reference section)
    engine
            .execute(&format!(
                "INSERT INTO bookmark_category (bookmark_id, category_id) VALUES ('{bm_id}', '{cat_id}')"
            ))
            .unwrap();

    // Verify wikilink exists in bookmark file before delete
    let bm_path = index.resolve_path(&bm_id).unwrap();
    let content_before = repo.read_file(&bm_path).unwrap();
    assert!(
        content_before.contains(&format!("[[{cat_id}]]")),
        "bookmark should reference category before delete"
    );

    // Delete the category
    engine
        .execute(&format!("DELETE FROM category WHERE id = '{cat_id}'"))
        .unwrap();

    // Bookmark file should no longer contain the wikilink to deleted category
    let content_after = repo.read_file(&bm_path).unwrap();
    assert!(
        !content_after.contains(&format!("[[{cat_id}]]")),
        "wikilink to deleted category should be removed from bookmark file"
    );
}

#[test]
fn test_cascade_ref_multi_reference_preservation() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE category (label VARCHAR(100))")
        .unwrap();
    engine
        .execute("CREATE TABLE bookmark (url TEXT, category TEXT REFERENCES category)")
        .unwrap();

    let cat_a = match engine
        .execute("INSERT INTO category (label) VALUES ('tech')")
        .unwrap()
    {
        SqlResult::Ok(id) => id,
        other => panic!("expected Ok, got {other:?}"),
    };
    let cat_b = match engine
        .execute("INSERT INTO category (label) VALUES ('science')")
        .unwrap()
    {
        SqlResult::Ok(id) => id,
        other => panic!("expected Ok, got {other:?}"),
    };
    let bm_id = match engine
        .execute("INSERT INTO bookmark (url) VALUES ('https://example.com')")
        .unwrap()
    {
        SqlResult::Ok(id) => id,
        other => panic!("expected Ok, got {other:?}"),
    };

    // Bookmark references both categories
    engine
            .execute(&format!(
                "INSERT INTO bookmark_category (bookmark_id, category_id) VALUES ('{bm_id}', '{cat_a}')"
            ))
            .unwrap();
    engine
            .execute(&format!(
                "INSERT INTO bookmark_category (bookmark_id, category_id) VALUES ('{bm_id}', '{cat_b}')"
            ))
            .unwrap();

    // Delete only category A
    engine
        .execute(&format!("DELETE FROM category WHERE id = '{cat_a}'"))
        .unwrap();

    // Bookmark file should still contain wikilink to category B
    let bm_path = index.resolve_path(&bm_id).unwrap();
    let content = repo.read_file(&bm_path).unwrap();
    assert!(
        !content.contains(&format!("[[{cat_a}]]")),
        "wikilink to deleted category A should be removed"
    );
    assert!(
        content.contains(&format!("[[{cat_b}]]")),
        "wikilink to surviving category B should be preserved"
    );
}

#[test]
fn test_cascade_ref_multiple_referencing_doogats() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE category (label VARCHAR(100))")
        .unwrap();
    engine
        .execute("CREATE TABLE bookmark (url TEXT, category TEXT REFERENCES category)")
        .unwrap();

    let cat_id = match engine
        .execute("INSERT INTO category (label) VALUES ('tech')")
        .unwrap()
    {
        SqlResult::Ok(id) => id,
        other => panic!("expected Ok, got {other:?}"),
    };
    let bm1_id = match engine
        .execute("INSERT INTO bookmark (url) VALUES ('https://one.com')")
        .unwrap()
    {
        SqlResult::Ok(id) => id,
        other => panic!("expected Ok, got {other:?}"),
    };
    let bm2_id = match engine
        .execute("INSERT INTO bookmark (url) VALUES ('https://two.com')")
        .unwrap()
    {
        SqlResult::Ok(id) => id,
        other => panic!("expected Ok, got {other:?}"),
    };

    // Both bookmarks reference the same category
    engine
            .execute(&format!(
                "INSERT INTO bookmark_category (bookmark_id, category_id) VALUES ('{bm1_id}', '{cat_id}')"
            ))
            .unwrap();
    engine
            .execute(&format!(
                "INSERT INTO bookmark_category (bookmark_id, category_id) VALUES ('{bm2_id}', '{cat_id}')"
            ))
            .unwrap();

    // Delete the category
    engine
        .execute(&format!("DELETE FROM category WHERE id = '{cat_id}'"))
        .unwrap();

    // Both bookmark files should have the wikilink removed
    let bm1_path = index.resolve_path(&bm1_id).unwrap();
    let bm1_content = repo.read_file(&bm1_path).unwrap();
    assert!(
        !bm1_content.contains(&format!("[[{cat_id}]]")),
        "wikilink to deleted category should be removed from bookmark 1"
    );

    let bm2_path = index.resolve_path(&bm2_id).unwrap();
    let bm2_content = repo.read_file(&bm2_path).unwrap();
    assert!(
        !bm2_content.contains(&format!("[[{cat_id}]]")),
        "wikilink to deleted category should be removed from bookmark 2"
    );
}

#[test]
fn test_cascade_atomic_single_commit() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE category (label VARCHAR(100))")
        .unwrap();
    engine
        .execute("CREATE TABLE bookmark (url TEXT, category TEXT REFERENCES category)")
        .unwrap();

    let cat_id = match engine
        .execute("INSERT INTO category (label) VALUES ('tech')")
        .unwrap()
    {
        SqlResult::Ok(id) => id,
        other => panic!("expected Ok, got {other:?}"),
    };
    let bm_id = match engine
        .execute("INSERT INTO bookmark (url) VALUES ('https://example.com')")
        .unwrap()
    {
        SqlResult::Ok(id) => id,
        other => panic!("expected Ok, got {other:?}"),
    };

    engine
            .execute(&format!(
                "INSERT INTO bookmark_category (bookmark_id, category_id) VALUES ('{bm_id}', '{cat_id}')"
            ))
            .unwrap();

    // Record head before delete
    let head_before = repo.head_oid().unwrap();

    // Delete the category (should cascade both junction + ref removal)
    engine
        .execute(&format!("DELETE FROM category WHERE id = '{cat_id}'"))
        .unwrap();

    let head_after = repo.head_oid().unwrap();

    // Exactly one new commit should have been created (atomic batch)
    assert_ne!(head_before, head_after, "delete should create a commit");

    // Walk back one commit - should reach head_before
    let commit = repo
        .repo
        .find_commit(git2::Oid::from_str(&head_after.0).unwrap())
        .unwrap();
    let parent_oid = commit.parent(0).unwrap().id().to_string();
    assert_eq!(
        parent_oid, head_before.0,
        "cascade delete + ref removal should be one atomic commit"
    );
}

#[test]
fn select_coerces_boolean_columns_to_true_false() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE flags (name TEXT, active BOOLEAN)")
        .unwrap();
    engine
        .execute("INSERT INTO flags (name, active) VALUES ('alpha', true)")
        .unwrap();

    let result = engine.execute("SELECT name, active FROM flags").unwrap();
    match result {
        SqlResult::Rows { rows, .. } => {
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0][0], "alpha");
            assert_eq!(rows[0][1], "true");
        }
        _ => panic!("expected Rows"),
    }
}

#[test]
fn select_coerces_boolean_false_to_false_string() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE flags (name TEXT, active BOOLEAN)")
        .unwrap();
    engine
        .execute("INSERT INTO flags (name, active) VALUES ('beta', false)")
        .unwrap();

    let result = engine.execute("SELECT name, active FROM flags").unwrap();
    match result {
        SqlResult::Rows { rows, .. } => {
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0][0], "beta");
            assert_eq!(rows[0][1], "false");
        }
        _ => panic!("expected Rows"),
    }
}

#[test]
fn select_boolean_null_stays_null() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE flags (name TEXT, active BOOLEAN)")
        .unwrap();
    engine
        .execute("INSERT INTO flags (name) VALUES ('gamma')")
        .unwrap();

    let result = engine.execute("SELECT name, active FROM flags").unwrap();
    match result {
        SqlResult::Rows { rows, .. } => {
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0][0], "gamma");
            assert_eq!(rows[0][1], "NULL");
        }
        _ => panic!("expected Rows"),
    }
}

#[test]
fn select_boolean_coercion_preserves_non_boolean_columns() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE mixed (name TEXT, active BOOLEAN, count INTEGER)")
        .unwrap();
    engine
        .execute("INSERT INTO mixed (name, active, count) VALUES ('delta', true, 7)")
        .unwrap();

    let result = engine
        .execute("SELECT name, active, count FROM mixed")
        .unwrap();
    match result {
        SqlResult::Rows { rows, .. } => {
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0][0], "delta");
            assert_eq!(rows[0][1], "true");
            assert_eq!(rows[0][2], "7");
        }
        _ => panic!("expected Rows"),
    }
}

#[test]
fn select_star_coerces_boolean_columns() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE flags (name TEXT, active BOOLEAN)")
        .unwrap();
    engine
        .execute("INSERT INTO flags (name, active) VALUES ('epsilon', true)")
        .unwrap();

    let result = engine.execute("SELECT * FROM flags").unwrap();
    match result {
        SqlResult::Rows { columns, rows, .. } => {
            assert_eq!(rows.len(), 1);
            let active_idx = columns.iter().position(|c| c == "active").unwrap();
            assert_eq!(rows[0][active_idx], "true");
        }
        _ => panic!("expected Rows"),
    }
}

#[test]
fn select_join_bypasses_boolean_coercion() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE jtbl (active BOOLEAN)")
        .unwrap();
    engine
        .execute("INSERT INTO jtbl (active) VALUES (true)")
        .unwrap();

    // JOIN query should not apply coercion (returns raw "1")
    let result = engine
        .execute("SELECT j.active FROM jtbl j JOIN doogats d ON d.id = j.id")
        .unwrap();
    match result {
        SqlResult::Rows {
            rows, column_types, ..
        } => {
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0][0], "1");
            assert!(column_types.is_none());
        }
        _ => panic!("expected Rows"),
    }
}

#[test]
fn create_table_next_default_stores_marker() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE foo (pos INTEGER DEFAULT NEXT)")
        .unwrap();

    let schema = engine.load_schema("foo").unwrap();
    let col = schema.columns.iter().find(|c| c.name == "pos").unwrap();
    assert_eq!(col.default_value, Some("NEXT".to_string()));
    assert_eq!(col.data_type, "INTEGER");
}

#[test]
fn create_table_next_scoped_default_stores_expression() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE foo (category_id TEXT, pos INTEGER DEFAULT NEXT(category_id))")
        .unwrap();

    let schema = engine.load_schema("foo").unwrap();
    let col = schema.columns.iter().find(|c| c.name == "pos").unwrap();
    assert_eq!(col.default_value, Some("NEXT(category_id)".to_string()));
}

#[test]
fn create_table_next_scoped_rejects_nonexistent_column() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    let err = engine
        .execute("CREATE TABLE foo (pos INTEGER DEFAULT NEXT(nonexistent))")
        .unwrap_err();
    assert!(
        format!("{err}").contains("not found"),
        "expected 'not found' error, got: {err}"
    );
}

#[test]
fn create_table_next_rejects_empty_args() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    let err = engine
        .execute("CREATE TABLE foo (pos INTEGER DEFAULT NEXT())")
        .unwrap_err();
    assert!(
        format!("{err}").contains("exactly one"),
        "expected 'exactly one' error, got: {err}"
    );
}

#[test]
fn create_table_next_rejects_multiple_args() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    let err = engine
        .execute("CREATE TABLE foo (a TEXT, b TEXT, pos INTEGER DEFAULT NEXT(a, b))")
        .unwrap_err();
    assert!(
        format!("{err}").contains("only one"),
        "expected 'only one' error, got: {err}"
    );
}

#[test]
fn create_table_next_default_rejected_on_non_integer() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    let err = engine
        .execute("CREATE TABLE foo (pos VARCHAR(255) DEFAULT NEXT)")
        .unwrap_err();
    assert!(
        format!("{err}").to_lowercase().contains("integer"),
        "expected error about INTEGER requirement, got: {err}"
    );
}

#[test]
fn create_table_next_default_roundtrip() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE roundtrip (pos INTEGER DEFAULT NEXT)")
        .unwrap();

    // Load schema from stored typedef and verify default survives
    let schema = engine.load_schema("roundtrip").unwrap();
    let col = schema.columns.iter().find(|c| c.name == "pos").unwrap();
    assert_eq!(col.default_value, Some("NEXT".to_string()));

    // Also verify via a fresh engine to ensure persistence
    let mut engine2 = SqlEngine::new(&index, &repo);
    let schema2 = engine2.load_schema("roundtrip").unwrap();
    let col2 = schema2.columns.iter().find(|c| c.name == "pos").unwrap();
    assert_eq!(col2.default_value, Some("NEXT".to_string()));
}

#[test]
fn create_table_mixed_static_and_next_defaults() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
            .execute(
                "CREATE TABLE mixed (name TEXT DEFAULT 'untitled', pos INTEGER DEFAULT NEXT, priority INTEGER DEFAULT 0)",
            )
            .unwrap();

    let schema = engine.load_schema("mixed").unwrap();

    let name_col = schema.columns.iter().find(|c| c.name == "name").unwrap();
    assert_eq!(name_col.default_value, Some("untitled".to_string()));

    let pos_col = schema.columns.iter().find(|c| c.name == "pos").unwrap();
    assert_eq!(pos_col.default_value, Some("NEXT".to_string()));

    let prio_col = schema
        .columns
        .iter()
        .find(|c| c.name == "priority")
        .unwrap();
    assert_eq!(prio_col.default_value, Some("0".to_string()));
}

#[test]
fn insert_next_default_persists_in_git() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE foo (name TEXT, pos INTEGER DEFAULT NEXT)")
        .unwrap();

    let id = match engine
        .execute("INSERT INTO foo (name) VALUES ('a')")
        .unwrap()
    {
        SqlResult::Ok(id) => id,
        _ => panic!("expected Ok"),
    };

    // Verify the value is in the materialized table
    let rows = index.query_raw("SELECT pos FROM foo").unwrap();
    assert_eq!(rows[0][0], "1");

    // Verify the value is persisted in the git doogat
    let path = index.resolve_path(&id).unwrap();
    let content = repo.read_file(&path).unwrap();
    assert!(
        content.contains("pos: 1"),
        "pos not found in doogat content:\n{content}"
    );
}

#[test]
fn insert_next_default_auto_increments() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE foo (name TEXT, pos INTEGER DEFAULT NEXT)")
        .unwrap();

    engine
        .execute("INSERT INTO foo (name) VALUES ('a')")
        .unwrap();
    std::thread::sleep(std::time::Duration::from_secs(1));
    engine
        .execute("INSERT INTO foo (name) VALUES ('b')")
        .unwrap();
    std::thread::sleep(std::time::Duration::from_secs(1));
    engine
        .execute("INSERT INTO foo (name) VALUES ('c')")
        .unwrap();

    let rows = index.query_raw("SELECT pos FROM foo ORDER BY pos").unwrap();
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0][0], "1");
    assert_eq!(rows[1][0], "2");
    assert_eq!(rows[2][0], "3");
}

#[test]
fn insert_next_default_after_delete_uses_max_plus_one() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE foo (name TEXT, pos INTEGER DEFAULT NEXT)")
        .unwrap();

    engine
        .execute("INSERT INTO foo (name) VALUES ('a')")
        .unwrap();
    std::thread::sleep(std::time::Duration::from_secs(1));
    let id2 = match engine
        .execute("INSERT INTO foo (name) VALUES ('b')")
        .unwrap()
    {
        SqlResult::Ok(id) => id,
        _ => panic!("expected Ok"),
    };
    std::thread::sleep(std::time::Duration::from_secs(1));
    engine
        .execute("INSERT INTO foo (name) VALUES ('c')")
        .unwrap();

    // Delete row with pos=2
    engine
        .execute(&format!("DELETE FROM foo WHERE id = '{id2}'"))
        .unwrap();

    // Next insert should get 4, not 2 (no gap-filling)
    std::thread::sleep(std::time::Duration::from_secs(1));
    engine
        .execute("INSERT INTO foo (name) VALUES ('d')")
        .unwrap();

    let rows = index.query_raw("SELECT pos FROM foo ORDER BY pos").unwrap();
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0][0], "1");
    assert_eq!(rows[1][0], "3");
    assert_eq!(rows[2][0], "4");
}

#[test]
fn insert_next_default_partitioned_independent_sequences() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute(
            "CREATE TABLE items (category_id TEXT, sort_order INTEGER DEFAULT NEXT(category_id))",
        )
        .unwrap();

    // cat1 first insert -> sort_order=1
    engine
        .execute("INSERT INTO items (category_id) VALUES ('cat1')")
        .unwrap();
    std::thread::sleep(std::time::Duration::from_secs(1));
    // cat2 first insert -> sort_order=1
    engine
        .execute("INSERT INTO items (category_id) VALUES ('cat2')")
        .unwrap();
    std::thread::sleep(std::time::Duration::from_secs(1));
    // cat1 second insert -> sort_order=2
    engine
        .execute("INSERT INTO items (category_id) VALUES ('cat1')")
        .unwrap();

    let rows = index
        .query_raw("SELECT category_id, sort_order FROM items ORDER BY category_id, sort_order")
        .unwrap();
    assert_eq!(rows.len(), 3);
    // cat1 rows
    assert_eq!(rows[0][0], "cat1");
    assert_eq!(rows[0][1], "1");
    assert_eq!(rows[1][0], "cat1");
    assert_eq!(rows[1][1], "2");
    // cat2 row
    assert_eq!(rows[2][0], "cat2");
    assert_eq!(rows[2][1], "1");
}

#[test]
fn insert_next_default_explicit_override() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE foo (name TEXT, pos INTEGER DEFAULT NEXT)")
        .unwrap();

    // Explicit value should be respected
    engine
        .execute("INSERT INTO foo (name, pos) VALUES ('x', 99)")
        .unwrap();

    let rows = index.query_raw("SELECT pos FROM foo ORDER BY pos").unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0][0], "99");

    // Next auto insert should get 100
    std::thread::sleep(std::time::Duration::from_secs(1));
    engine
        .execute("INSERT INTO foo (name) VALUES ('y')")
        .unwrap();

    let rows = index.query_raw("SELECT pos FROM foo ORDER BY pos").unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0][0], "99");
    assert_eq!(rows[1][0], "100");
}

#[test]
fn insert_next_default_empty_table_starts_at_one() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE foo (name TEXT, pos INTEGER DEFAULT NEXT)")
        .unwrap();

    engine
        .execute("INSERT INTO foo (name) VALUES ('first')")
        .unwrap();

    let rows = index.query_raw("SELECT pos FROM foo").unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0][0], "1");
}

#[test]
fn insert_next_default_multi_row_assigns_sequential() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE foo (name TEXT, pos INTEGER DEFAULT NEXT)")
        .unwrap();

    engine
        .execute("INSERT INTO foo (name) VALUES ('a'), ('b'), ('c')")
        .unwrap();

    let rows = index.query_raw("SELECT pos FROM foo ORDER BY pos").unwrap();
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0][0], "1");
    assert_eq!(rows[1][0], "2");
    assert_eq!(rows[2][0], "3");
}

#[test]
fn insert_next_partitioned_multi_row_same_partition() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE items (cat TEXT, pos INTEGER DEFAULT NEXT(cat))")
        .unwrap();

    // Multi-row INSERT with same partition value
    engine
        .execute("INSERT INTO items (cat) VALUES ('a'), ('a'), ('a')")
        .unwrap();

    let rows = index
        .query_raw("SELECT pos FROM items ORDER BY pos")
        .unwrap();
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0][0], "1");
    assert_eq!(rows[1][0], "2");
    assert_eq!(rows[2][0], "3");
}

// ── Helpers for expression tests ────────────────────────────────

/// Parse a SQL expression string into an `Expr` via a SELECT wrapper.
fn parse_expr(sql: &str) -> Expr {
    let stmts = Parser::parse_sql(&GenericDialect, &format!("SELECT {sql}")).unwrap();
    match &stmts[0] {
        Statement::Query(q) => match q.body.as_ref() {
            SetExpr::Select(s) => match &s.projection[0] {
                sqlparser::ast::SelectItem::UnnamedExpr(expr) => expr.clone(),
                other => panic!("expected UnnamedExpr, got {other:?}"),
            },
            _ => panic!("expected Select"),
        },
        _ => panic!("expected Query"),
    }
}

// ── Part A: unit tests for formatting and evaluation ────────────

#[test]
fn value_to_sql_formats_coalesce() {
    let expr = parse_expr("COALESCE(NULL, 'fallback')");
    let sql = value_to_sql(&expr).unwrap();
    assert_eq!(sql, "COALESCE(NULL, 'fallback')");
}

#[test]
fn value_to_sql_rejects_unlisted_function() {
    let expr = parse_expr("RANDOM()");
    let err = value_to_sql(&expr).unwrap_err();
    let msg = format!("{err}");
    assert!(
        msg.contains("not allowed"),
        "expected 'not allowed' in: {msg}"
    );
}

#[test]
fn value_to_sql_formats_subquery() {
    let expr = parse_expr("(SELECT MAX(x) FROM t)");
    let sql = value_to_sql(&expr).unwrap();
    assert!(sql.contains("SELECT"), "expected SELECT in: {sql}");
    assert!(
        sql.starts_with('(') && sql.ends_with(')'),
        "expected parens in: {sql}"
    );
}

#[test]
fn value_to_sql_formats_binary_op() {
    let expr = parse_expr("1 + 2");
    let sql = value_to_sql(&expr).unwrap();
    assert_eq!(sql, "(1 + 2)");
}

#[test]
fn value_to_sql_formats_nested() {
    let expr = parse_expr("(42)");
    let sql = value_to_sql(&expr).unwrap();
    assert_eq!(sql, "(42)");
}

#[test]
fn is_literal_expr_true_for_value() {
    let expr = parse_expr("'hello'");
    assert!(is_literal_expr(&expr));
}

#[test]
fn is_literal_expr_false_for_function() {
    let expr = parse_expr("COALESCE(1, 2)");
    assert!(!is_literal_expr(&expr));
}

#[test]
fn eval_expr_coalesce_null_fallback() {
    let (_dir, _repo, index) = setup();
    let expr = parse_expr("COALESCE(NULL, 'fallback')");
    let result = eval_expr(&index.conn, &expr).unwrap();
    assert_eq!(result, "fallback");
}

#[test]
fn eval_expr_ifnull() {
    let (_dir, _repo, index) = setup();
    let expr = parse_expr("IFNULL(NULL, 'default')");
    let result = eval_expr(&index.conn, &expr).unwrap();
    assert_eq!(result, "default");
}

#[test]
fn eval_expr_nullif_returns_empty() {
    let (_dir, _repo, index) = setup();
    let expr = parse_expr("NULLIF('', '')");
    let result = eval_expr(&index.conn, &expr).unwrap();
    assert_eq!(result, "");
}

#[test]
fn eval_expr_abs() {
    let (_dir, _repo, index) = setup();
    let expr = parse_expr("ABS(-5)");
    let result = eval_expr(&index.conn, &expr).unwrap();
    assert_eq!(result, "5");
}

#[test]
fn eval_expr_binary_op() {
    let (_dir, _repo, index) = setup();
    let expr = parse_expr("2 + 3");
    let result = eval_expr(&index.conn, &expr).unwrap();
    assert_eq!(result, "5");
}

#[test]
fn eval_expr_literal_passthrough() {
    let (_dir, _repo, index) = setup();
    let expr = parse_expr("'hello'");
    let result = eval_expr(&index.conn, &expr).unwrap();
    assert_eq!(result, "hello");
}

// ── Part B: integration tests (full SQL engine flow) ────────────

#[test]
fn insert_with_coalesce_subquery() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE expr_coal (name TEXT, sort_order INTEGER)")
        .unwrap();

    // First insert: COALESCE over empty table gives 0
    engine
            .execute(
                "INSERT INTO expr_coal (name, sort_order) VALUES ('first', COALESCE((SELECT MAX(sort_order) FROM expr_coal), 0))",
            )
            .unwrap();

    let rows = index
        .query_raw("SELECT sort_order FROM expr_coal WHERE name = 'first'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0][0], "0");

    // Second insert: COALESCE picks up the existing max (0) + 1 = 1
    engine
            .execute(
                "INSERT INTO expr_coal (name, sort_order) VALUES ('second', COALESCE((SELECT MAX(sort_order) FROM expr_coal), 0) + 1)",
            )
            .unwrap();

    let rows = index
        .query_raw("SELECT sort_order FROM expr_coal WHERE name = 'second'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0][0], "1");
}

#[test]
fn update_with_ifnull() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE expr_ifn (name TEXT, status TEXT)")
        .unwrap();

    let id = match engine
        .execute("INSERT INTO expr_ifn (name, status) VALUES ('row1', 'active')")
        .unwrap()
    {
        SqlResult::Ok(id) => id,
        other => panic!("expected Ok, got {other:?}"),
    };

    engine
        .execute(&format!(
            "UPDATE expr_ifn SET status = IFNULL(NULL, 'default') WHERE id = '{id}'"
        ))
        .unwrap();

    let rows = index
        .query_raw(&format!("SELECT status FROM expr_ifn WHERE id = '{id}'"))
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0][0], "default");
}

#[test]
fn insert_rejects_unlisted_function() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE expr_rej (name TEXT, val INTEGER)")
        .unwrap();

    let err = engine
        .execute("INSERT INTO expr_rej (name, val) VALUES ('x', RANDOM())")
        .unwrap_err();
    let msg = format!("{err}");
    assert!(
        msg.contains("not allowed"),
        "expected 'not allowed' in: {msg}"
    );
}

#[test]
fn insert_with_arithmetic() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE expr_arith (name TEXT, val INTEGER)")
        .unwrap();

    engine
        .execute("INSERT INTO expr_arith (name, val) VALUES ('sum', (2 + 3))")
        .unwrap();

    let rows = index
        .query_raw("SELECT val FROM expr_arith WHERE name = 'sum'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0][0], "5");
}

#[test]
fn update_expression_rejects_invalid_allowed_value() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE expr_enum (name TEXT, status ENUM('open','closed'))")
        .unwrap();

    let id = match engine
        .execute("INSERT INTO expr_enum (name, status) VALUES ('item', 'open')")
        .unwrap()
    {
        SqlResult::Ok(id) => id,
        other => panic!("expected Ok, got {other:?}"),
    };

    // Literal update with invalid value should be rejected
    let err = engine
        .execute(&format!(
            "UPDATE expr_enum SET status = 'invalid' WHERE id = '{id}'"
        ))
        .unwrap_err();
    assert!(format!("{err}").contains("not in allowed values"));

    // Expression update with invalid value should also be rejected
    let err = engine
        .execute(&format!(
            "UPDATE expr_enum SET status = UPPER('invalid') WHERE id = '{id}'"
        ))
        .unwrap_err();
    assert!(format!("{err}").contains("not in allowed values"));
}

#[test]
fn insert_defaults_date_from_id() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE events (name TEXT, priority INTEGER)")
        .unwrap();

    let id = match engine
        .execute("INSERT INTO events (name, priority) VALUES ('Launch', 1)")
        .unwrap()
    {
        SqlResult::Ok(id) => id,
        _ => panic!("expected Ok"),
    };

    // Derive expected date from the 14-digit ID (YYYYMMDDHHmmss → YYYY-MM-DD)
    let expected_date = format!("{}-{}-{}", &id[0..4], &id[4..6], &id[6..8]);

    let path = index.resolve_path(&id).unwrap();
    let content = repo.read_file(&path).unwrap();
    assert!(
        content.contains(&format!("date: {expected_date}")),
        "expected date derived from ID in frontmatter: {content}"
    );

    let parsed = crate::parser::parse(&content, &path).unwrap();
    assert_eq!(
        parsed.meta.date.as_deref(),
        Some(expected_date.as_str()),
        "parsed date should match ID-derived date"
    );
}

#[test]
fn insert_explicit_date_preserved() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE meetings (name TEXT, priority INTEGER)")
        .unwrap();

    let id = match engine
        .execute("INSERT INTO meetings (name, date, priority) VALUES ('Standup', '2025-12-25', 2)")
        .unwrap()
    {
        SqlResult::Ok(id) => id,
        _ => panic!("expected Ok"),
    };

    let path = index.resolve_path(&id).unwrap();
    let content = repo.read_file(&path).unwrap();
    assert!(
        content.contains("date: 2025-12-25"),
        "explicit date should be preserved in frontmatter: {content}"
    );

    // Verify the explicit date is NOT overridden by the ID-derived date
    let id_derived = format!("{}-{}-{}", &id[0..4], &id[4..6], &id[6..8]);
    let parsed = crate::parser::parse(&content, &path).unwrap();
    assert_eq!(
        parsed.meta.date.as_deref(),
        Some("2025-12-25"),
        "explicit date should win over ID-derived date ({id_derived})"
    );
}

#[test]
fn insert_schema_date_column_no_duplicate() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    // Type with "date" as an explicit schema column (like meeting-minutes)
    engine
        .execute("CREATE TABLE minutes (date TEXT, attendees TEXT)")
        .unwrap();

    // ALTER to put date in frontmatter zone (default for TEXT is body)
    engine
        .execute("ALTER TABLE minutes SET ZONE frontmatter FOR date")
        .unwrap();

    let id = match engine
        .execute("INSERT INTO minutes (date, attendees) VALUES ('2026-03-15', 'Alice')")
        .unwrap()
    {
        SqlResult::Ok(id) => id,
        _ => panic!("expected Ok"),
    };

    let path = index.resolve_path(&id).unwrap();
    let content = repo.read_file(&path).unwrap();

    let date_count = content.matches("date:").count();
    assert_eq!(
        date_count, 1,
        "expected exactly one date: field, got {date_count} in: {content}"
    );

    let parsed = crate::parser::parse(&content, &path).unwrap();
    assert_eq!(
        parsed.meta.date.as_deref(),
        Some("2026-03-15"),
        "schema column date should be promoted to meta.date"
    );
}

fn make_typedef_parsed(title: &str, extra: BTreeMap<String, Value>) -> ParsedDoogat {
    ParsedDoogat {
        meta: DoogatMeta {
            id: None,
            title: Some(title.to_string()),
            date: None,
            doogat_type: Some("_typedef".into()),
            tags: vec![],
            extra,
        },
        body: String::new(),
        sections: vec![],
        reference_section: String::new(),
        inline_fields: vec![],
        links: vec![],
        body_tags: vec![],
        checkboxes: vec![],
        path: "ddb/_typedef/test.md".into(),
        updated_at: None,
    }
}

#[test]
fn schema_from_parsed_rejects_multi_hop_title_template() {
    let mut extra = BTreeMap::new();
    extra.insert("columns".to_string(), Value::List(vec![]));
    extra.insert(
        "title_template".to_string(),
        Value::String("{a.b.c}".to_string()),
    );
    let parsed = make_typedef_parsed("my_type", extra);
    let err = schema_from_parsed(&parsed).unwrap_err();
    let msg = format!("{err}");
    assert!(
        msg.contains("multi-hop") || msg.contains("one-level"),
        "expected multi-hop rejection: {msg}"
    );
}

#[test]
fn schema_from_parsed_rejects_dotted_path_on_non_ref_column() {
    let mut col = BTreeMap::new();
    col.insert("name".to_string(), Value::String("label".into()));
    col.insert("data_type".to_string(), Value::String("TEXT".into()));
    let mut extra = BTreeMap::new();
    extra.insert("columns".to_string(), Value::List(vec![Value::Map(col)]));
    extra.insert(
        "title_template".to_string(),
        Value::String("{label.title}".to_string()),
    );
    let parsed = make_typedef_parsed("my_type", extra);
    let err = schema_from_parsed(&parsed).unwrap_err();
    let msg = format!("{err}");
    assert!(
        msg.contains("not a REFERENCES column"),
        "expected non-ref rejection: {msg}"
    );
}

#[test]
fn schema_from_parsed_rejects_dotted_path_on_missing_column() {
    let mut extra = BTreeMap::new();
    extra.insert("columns".to_string(), Value::List(vec![]));
    extra.insert(
        "title_template".to_string(),
        Value::String("{ghost.title}".to_string()),
    );
    let parsed = make_typedef_parsed("my_type", extra);
    let err = schema_from_parsed(&parsed).unwrap_err();
    let msg = format!("{err}");
    assert!(
        msg.contains("not found"),
        "expected column-not-found rejection: {msg}"
    );
}

#[test]
fn schema_from_parsed_unique_together_absent() {
    let mut extra = BTreeMap::new();
    extra.insert("columns".to_string(), Value::List(vec![]));
    let parsed = make_typedef_parsed("my_type", extra);
    let schema = schema_from_parsed(&parsed).unwrap();
    assert_eq!(schema.unique_together, None);
}

#[test]
fn schema_from_parsed_unique_together_flat() {
    let mut extra = BTreeMap::new();
    extra.insert("columns".to_string(), Value::List(vec![]));
    extra.insert(
        "unique_together".to_string(),
        Value::List(vec![
            Value::String("link_id".to_string()),
            Value::String("category_fqn".to_string()),
        ]),
    );
    let parsed = make_typedef_parsed("my_type", extra);
    let schema = schema_from_parsed(&parsed).unwrap();
    assert_eq!(
        schema.unique_together,
        Some(vec![vec![
            "link_id".to_string(),
            "category_fqn".to_string()
        ]])
    );
}

#[test]
fn schema_from_parsed_unique_together_nested() {
    let mut extra = BTreeMap::new();
    extra.insert("columns".to_string(), Value::List(vec![]));
    extra.insert(
        "unique_together".to_string(),
        Value::List(vec![
            Value::List(vec![
                Value::String("a".to_string()),
                Value::String("b".to_string()),
            ]),
            Value::List(vec![
                Value::String("c".to_string()),
                Value::String("d".to_string()),
            ]),
        ]),
    );
    let parsed = make_typedef_parsed("my_type", extra);
    let schema = schema_from_parsed(&parsed).unwrap();
    assert_eq!(
        schema.unique_together,
        Some(vec![
            vec!["a".to_string(), "b".to_string()],
            vec!["c".to_string(), "d".to_string()],
        ])
    );
}

#[test]
fn schema_from_parsed_unique_together_empty_list() {
    let mut extra = BTreeMap::new();
    extra.insert("columns".to_string(), Value::List(vec![]));
    extra.insert("unique_together".to_string(), Value::List(vec![]));
    let parsed = make_typedef_parsed("my_type", extra);
    let schema = schema_from_parsed(&parsed).unwrap();
    assert_eq!(schema.unique_together, None);
}

// Helper: set up a table with a unique_together constraint on (code) by manually
// building the typedef and indexing it, then returning the engine ready to use.
fn setup_unique_table(repo: &crate::git_ops::GitRepo, index: &crate::indexer::Index) {
    let typedef = "\
---
id: 20260501000000
title: uqtest
type: _typedef
columns:
  - name: code
    data_type: TEXT
    zone: frontmatter
  - name: label
    data_type: TEXT
    zone: frontmatter
unique_together:
  - - code
---
";
    let typedef_path = "ddb/_typedef/20260501000000.md";
    repo.commit_file(typedef_path, typedef, "add uqtest typedef")
        .unwrap();
    let parsed = crate::parser::parse(typedef, typedef_path).unwrap();
    index.index_doogat(&parsed).unwrap();
    index.materialize_all_types(repo).unwrap();
}

#[test]
fn on_conflict_do_nothing_returns_existing_id() {
    let (_dir, repo, index) = setup();
    setup_unique_table(&repo, &index);
    let mut engine = SqlEngine::new(&index, &repo);

    // First insert succeeds normally
    let first_result = engine
        .execute("INSERT INTO uqtest (code, label) VALUES ('ABC', 'first')")
        .unwrap();
    let first_id = match &first_result {
        SqlResult::Ok(id) => id.clone(),
        other => panic!("expected Ok(id) for first insert, got {other:?}"),
    };

    // Second insert with the same unique key and ON CONFLICT DO NOTHING returns existing ID
    let result = engine
        .execute("INSERT INTO uqtest (code, label) VALUES ('ABC', 'second') ON CONFLICT DO NOTHING")
        .unwrap();

    match result {
        SqlResult::Ok(ref id) => assert_eq!(id, &first_id, "should return existing row ID"),
        other => panic!("expected Ok(existing_id) for skipped duplicate, got {other:?}"),
    }

    // Only one row in the table - the original value is unchanged
    let rows = index.query_raw("SELECT label FROM uqtest").unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0][0], "first");
}

#[test]
fn on_conflict_do_nothing_returns_new_id() {
    let (_dir, repo, index) = setup();
    setup_unique_table(&repo, &index);
    let mut engine = SqlEngine::new(&index, &repo);

    // No existing row - ON CONFLICT DO NOTHING returns new ID like a normal insert
    let result = engine
        .execute("INSERT INTO uqtest (code, label) VALUES ('XYZ', 'new') ON CONFLICT DO NOTHING")
        .unwrap();

    match &result {
        SqlResult::Ok(id) => assert!(
            id.chars().all(|c| c.is_ascii_digit()) && id.len() == 14,
            "expected 14-digit ID, got: {id}"
        ),
        other => panic!("expected Ok with new id, got {other:?}"),
    }

    let rows = index.query_raw("SELECT code, label FROM uqtest").unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0][0], "XYZ");
    assert_eq!(rows[0][1], "new");
}

#[test]
fn on_conflict_do_update_rejected() {
    let (_dir, repo, index) = setup();
    setup_unique_table(&repo, &index);
    let mut engine = SqlEngine::new(&index, &repo);

    // ON CONFLICT DO UPDATE SET is not supported
    let err = engine
            .execute(
                "INSERT INTO uqtest (code, label) VALUES ('ABC', 'x') ON CONFLICT (code) DO UPDATE SET label = 'x'",
            )
            .unwrap_err();
    assert!(
        format!("{err}").contains("not supported"),
        "expected 'not supported' error, got: {err}"
    );
}

#[test]
fn plain_insert_duplicate_still_errors() {
    let (_dir, repo, index) = setup();
    setup_unique_table(&repo, &index);
    let mut engine = SqlEngine::new(&index, &repo);

    // First insert succeeds
    engine
        .execute("INSERT INTO uqtest (code, label) VALUES ('DUP', 'original')")
        .unwrap();

    // Second plain insert (no ON CONFLICT) with same unique key must still error
    let result = engine.execute("INSERT INTO uqtest (code, label) VALUES ('DUP', 'conflict')");
    assert!(
        result.is_err(),
        "plain INSERT with duplicate unique key should error, got: {result:?}"
    );
}

#[test]
fn duplicate_insert_does_not_leave_ghost_doogats_row() {
    // Regression for https://github.com/doogat/ddb/issues/4
    // A failing typed-table INSERT (UNIQUE violation) must NOT leave a ghost
    // row behind in the internal `doogats` index table. Otherwise every
    // subsequent mutation that touches the index via the GraphQL write path
    // fails on the dangling reference.
    let (_dir, repo, index) = setup();
    setup_unique_table(&repo, &index);
    let mut engine = SqlEngine::new(&index, &repo);

    // First insert succeeds
    let first_id = match engine
        .execute("INSERT INTO uqtest (code, label) VALUES ('DUP', 'original')")
        .unwrap()
    {
        SqlResult::Ok(id) => id,
        other => panic!("expected Ok(id), got {other:?}"),
    };

    let before = index
        .query_raw("SELECT id FROM doogats WHERE type = 'uqtest'")
        .unwrap();
    assert_eq!(before.len(), 1, "baseline: one uqtest row in doogats index");

    // Duplicate insert fails on UNIQUE constraint
    let result = engine.execute("INSERT INTO uqtest (code, label) VALUES ('DUP', 'conflict')");
    assert!(result.is_err(), "duplicate insert should fail, got: {result:?}");

    // doogats index must still have exactly the original row — no ghost entry
    let after = index
        .query_raw("SELECT id FROM doogats WHERE type = 'uqtest'")
        .unwrap();
    assert_eq!(
        after.len(),
        1,
        "failed INSERT left a ghost doogats index row; rows = {after:?}"
    );
    assert_eq!(
        after[0][0], first_id,
        "surviving row must be the first insert"
    );

    // The materialized typed table also has exactly the original row
    let typed = index.query_raw("SELECT label FROM uqtest").unwrap();
    assert_eq!(typed.len(), 1);
    assert_eq!(typed[0][0], "original");
}

// --- Issue #4 group A1: cross-mutation parity after a failed UNIQUE INSERT.
//
// `duplicate_insert_does_not_leave_ghost_doogats_row` above pins the index
// invariant. Issue #4 explicitly named all three GraphQL write paths
// (updateDoogat / createDoogat / deleteDoogat) as broken on the regression,
// so each one needs its own pin at the SQL engine level. The integration
// suite layers the GraphQL-surface checks on top in section 45.A1.

#[test]
fn update_after_unique_failure_succeeds_issue_4_a1() {
    let (_dir, repo, index) = setup();
    setup_unique_table(&repo, &index);
    let mut engine = SqlEngine::new(&index, &repo);

    // Seed a valid row.
    let valid_id = match engine
        .execute("INSERT INTO uqtest (code, label) VALUES ('A1U', 'original')")
        .unwrap()
    {
        SqlResult::Ok(id) => id,
        other => panic!("expected Ok(id), got {other:?}"),
    };

    // Failing duplicate must produce an error and leave no ghost row.
    let dup = engine.execute("INSERT INTO uqtest (code, label) VALUES ('A1U', 'dup')");
    assert!(dup.is_err(), "duplicate insert should error, got: {dup:?}");

    // updateDoogat analogue: update the existing row's label via SQL UPDATE.
    let upd = engine
        .execute(&format!("UPDATE uqtest SET label = 'updated' WHERE id = '{valid_id}'"))
        .unwrap();
    match upd {
        SqlResult::Affected(n) => assert_eq!(n, 1, "UPDATE should affect 1 row, got {n}"),
        other => panic!("expected Affected(1), got {other:?}"),
    }

    // Index and materialized table must agree: one row with the new label.
    let typed = index.query_raw("SELECT label FROM uqtest").unwrap();
    assert_eq!(typed.len(), 1);
    assert_eq!(typed[0][0], "updated");
    let idx = index
        .query_raw("SELECT id FROM doogats WHERE type = 'uqtest'")
        .unwrap();
    assert_eq!(idx.len(), 1);
    assert_eq!(idx[0][0], valid_id);
}

#[test]
fn insert_after_unique_failure_succeeds_issue_4_a1() {
    let (_dir, repo, index) = setup();
    setup_unique_table(&repo, &index);
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("INSERT INTO uqtest (code, label) VALUES ('A1I', 'original')")
        .unwrap();

    let dup = engine.execute("INSERT INTO uqtest (code, label) VALUES ('A1I', 'dup')");
    assert!(dup.is_err(), "duplicate insert should error, got: {dup:?}");

    // createDoogat analogue: a fresh INSERT with a different unique key must
    // still succeed after the rollback.
    let fresh = engine
        .execute("INSERT INTO uqtest (code, label) VALUES ('A1I_NEW', 'fresh')")
        .unwrap();
    let fresh_id = match fresh {
        SqlResult::Ok(id) => id,
        other => panic!("expected Ok(id), got {other:?}"),
    };

    let typed = index
        .query_raw("SELECT code, label FROM uqtest ORDER BY code")
        .unwrap();
    assert_eq!(typed.len(), 2);
    assert_eq!(typed[0][0], "A1I");
    assert_eq!(typed[1][0], "A1I_NEW");
    let idx = index
        .query_raw("SELECT id FROM doogats WHERE type = 'uqtest' ORDER BY id")
        .unwrap();
    assert_eq!(idx.len(), 2, "doogats index should have both rows");
    assert!(
        idx.iter().any(|row| row[0] == fresh_id),
        "fresh insert id {fresh_id} should be in the doogats index"
    );
}

#[test]
fn delete_after_unique_failure_succeeds_issue_4_a1() {
    let (_dir, repo, index) = setup();
    setup_unique_table(&repo, &index);
    let mut engine = SqlEngine::new(&index, &repo);

    let valid_id = match engine
        .execute("INSERT INTO uqtest (code, label) VALUES ('A1D', 'original')")
        .unwrap()
    {
        SqlResult::Ok(id) => id,
        other => panic!("expected Ok(id), got {other:?}"),
    };

    let dup = engine.execute("INSERT INTO uqtest (code, label) VALUES ('A1D', 'dup')");
    assert!(dup.is_err(), "duplicate insert should error, got: {dup:?}");

    // deleteDoogat analogue: DELETE the row that DID commit. Must succeed
    // and remove both the typed-table row and the doogats index row.
    let del = engine
        .execute(&format!("DELETE FROM uqtest WHERE id = '{valid_id}'"))
        .unwrap();
    match del {
        SqlResult::Affected(n) => assert_eq!(n, 1, "DELETE should affect 1 row, got {n}"),
        other => panic!("expected Affected(1), got {other:?}"),
    }

    let typed = index.query_raw("SELECT label FROM uqtest").unwrap();
    assert_eq!(typed.len(), 0, "typed table should be empty after delete");
    let idx = index
        .query_raw("SELECT id FROM doogats WHERE type = 'uqtest'")
        .unwrap();
    assert_eq!(
        idx.len(),
        0,
        "doogats index should have no uqtest rows after delete"
    );
}

#[test]
fn failed_insert_on_table_a_does_not_corrupt_table_b_issue_4_a3() {
    // Issue #4 group A3: a failed UNIQUE INSERT on table A must leave table B
    // fully writable. Proves the rollback is scoped to the failing table and
    // doesn't poison sibling materialized tables sharing the index.
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    // Table thing: plain NOT NULL column, no UNIQUE. Used to prove B remains
    // writable after the failure on table item.
    engine
        .execute("CREATE TABLE thing (title VARCHAR(255) NOT NULL)")
        .unwrap();
    // Table item: carries the UNIQUE constraint whose violation triggers the
    // rollback we're testing.
    engine
        .execute("CREATE TABLE item (title VARCHAR(255) NOT NULL, name VARCHAR(255) NOT NULL, UNIQUE(name))")
        .unwrap();

    // Seed table thing with one row we'll try to update after the failure.
    let thing_id = match engine
        .execute("INSERT INTO thing (title) VALUES ('t1')")
        .unwrap()
    {
        SqlResult::Ok(id) => id,
        other => panic!("expected Ok(id), got {other:?}"),
    };

    // Seed item with one row so the duplicate INSERT below has something to
    // collide with.
    engine
        .execute("INSERT INTO item (title, name) VALUES ('a', 'u1')")
        .unwrap();

    // Failing UNIQUE INSERT on item.
    let dup = engine.execute("INSERT INTO item (title, name) VALUES ('b', 'u1')");
    assert!(dup.is_err(), "duplicate insert should fail, got {dup:?}");

    // Table thing must still be writable via UPDATE.
    let upd = engine
        .execute(&format!(
            "UPDATE thing SET title = 't2' WHERE id = '{thing_id}'"
        ))
        .unwrap();
    match upd {
        SqlResult::Affected(n) => assert_eq!(n, 1, "UPDATE should affect 1 row, got {n}"),
        other => panic!("expected Affected(1), got {other:?}"),
    }

    // Verify the UPDATE landed on thing and didn't leak into item.
    let thing_rows = index.query_raw("SELECT title FROM thing").unwrap();
    assert_eq!(thing_rows.len(), 1);
    assert_eq!(thing_rows[0][0], "t2");
    let item_rows = index
        .query_raw("SELECT name FROM item ORDER BY name")
        .unwrap();
    assert_eq!(item_rows.len(), 1);
    assert_eq!(item_rows[0][0], "u1");

    // Table thing must still accept fresh INSERTs.
    engine
        .execute("INSERT INTO thing (title) VALUES ('t3')")
        .unwrap();
    let after = index
        .query_raw("SELECT COUNT(*) FROM thing")
        .unwrap();
    assert_eq!(after[0][0], "2");
}

#[test]
fn create_table_with_unique_constraint_enforced() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute(
            "CREATE TABLE membership (title TEXT, link_id VARCHAR(255) NOT NULL, cat VARCHAR(255) NOT NULL, UNIQUE(link_id, cat))",
        )
        .unwrap();

    // First insert succeeds
    engine
        .execute("INSERT INTO membership (title, link_id, cat) VALUES ('a', 'link1', 'cat1')")
        .unwrap();

    // Duplicate insert must fail due to UNIQUE constraint
    let result =
        engine.execute("INSERT INTO membership (title, link_id, cat) VALUES ('b', 'link1', 'cat1')");
    assert!(
        result.is_err(),
        "duplicate insert should fail with UNIQUE constraint, got: {result:?}"
    );
}

#[test]
fn composite_unique_duplicate_rejected_with_clear_error_issue_9_f1() {
    // Issue #9 group F1: composite UNIQUE duplicate rejection must produce a
    // clear error message that identifies the table and at least one of the
    // offending columns. The existing create_table_with_unique_constraint_enforced
    // pins the rejection at is_err() level; this test pins the MESSAGE format
    // so a regression that falls back to a generic "constraint failed" string
    // (without the table/column context) still gets caught.
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute(
            "CREATE TABLE f1membership (title TEXT, link_id VARCHAR(255) NOT NULL, category VARCHAR(255) NOT NULL, UNIQUE(link_id, category))",
        )
        .unwrap();

    engine
        .execute("INSERT INTO f1membership (title, link_id, category) VALUES ('a', 'link1', 'cat1')")
        .unwrap();

    let err = engine
        .execute("INSERT INTO f1membership (title, link_id, category) VALUES ('b', 'link1', 'cat1')")
        .expect_err("composite UNIQUE duplicate must error");
    let msg = format!("{err}");
    assert!(
        msg.contains("UNIQUE"),
        "error message should contain 'UNIQUE', got: {msg}"
    );
    // At least one of the offending columns or the table name must appear.
    // This keeps the test resilient if sqlite's error format changes the exact
    // phrasing but still catches a regression that strips all context.
    assert!(
        msg.contains("f1membership") || msg.contains("link_id") || msg.contains("category"),
        "error message should identify the table or a UNIQUE column, got: {msg}"
    );
}

#[test]
fn single_column_unique_duplicate_rejected_with_clear_error_issue_9_f2() {
    // Issue #9 group F2: single-column UNIQUE duplicate produces a clear error.
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE f2item (title TEXT, code VARCHAR(255) NOT NULL, UNIQUE(code))")
        .unwrap();
    engine
        .execute("INSERT INTO f2item (title, code) VALUES ('a', 'X1')")
        .unwrap();

    let err = engine
        .execute("INSERT INTO f2item (title, code) VALUES ('b', 'X1')")
        .expect_err("single-column UNIQUE duplicate must error");
    let msg = format!("{err}");
    assert!(
        msg.contains("UNIQUE"),
        "error message should contain 'UNIQUE', got: {msg}"
    );
    assert!(
        msg.contains("f2item") || msg.contains("code"),
        "error message should identify the table or column, got: {msg}"
    );
}

#[test]
fn concurrent_inserts_produce_unique_ids_issue_9_f8() {
    // Issue #9 group F8: rapid sequential INSERTs all get distinct IDs.
    // (True concurrency requires threads; this test exercises the fast-path
    // sequential case which is what the SQL engine sees from a single actor.)
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE f8rapid (label TEXT)")
        .unwrap();

    let mut ids = Vec::new();
    for i in 0..10 {
        let result = engine
            .execute(&format!("INSERT INTO f8rapid (title, label) VALUES ('row{i}', 'l{i}')"))
            .unwrap();
        match result {
            SqlResult::Ok(id) => ids.push(id),
            other => panic!("expected Ok(id) for row {i}, got {other:?}"),
        }
    }

    // All IDs must be distinct.
    let unique: std::collections::HashSet<&String> = ids.iter().collect();
    assert_eq!(
        unique.len(),
        ids.len(),
        "expected {} unique IDs, got {} — duplicates in: {ids:?}",
        ids.len(),
        unique.len()
    );
}

#[test]
fn create_table_unique_constraint_persisted_in_typedef() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute(
            "CREATE TABLE orders (title TEXT, customer VARCHAR(255), product VARCHAR(255), UNIQUE(customer, product))",
        )
        .unwrap();

    // Verify the typedef stored the unique_together constraint
    let schema = engine.load_schema("orders").unwrap();
    let constraints = schema.unique_together.expect("unique_together should be set");
    assert_eq!(constraints, vec![vec!["customer".to_string(), "product".to_string()]]);
}

#[test]
fn create_table_multiple_unique_constraints() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute(
            "CREATE TABLE multi (title TEXT, a VARCHAR(255), b VARCHAR(255), c VARCHAR(255), UNIQUE(a, b), UNIQUE(c))",
        )
        .unwrap();

    let schema = engine.load_schema("multi").unwrap();
    let constraints = schema.unique_together.expect("unique_together should be set");
    assert_eq!(constraints.len(), 2);
    assert_eq!(constraints[0], vec!["a".to_string(), "b".to_string()]);
    assert_eq!(constraints[1], vec!["c".to_string()]);
}

#[test]
fn create_table_unique_survives_rematerialization() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute(
            "CREATE TABLE remat (title TEXT, code VARCHAR(255), UNIQUE(code))",
        )
        .unwrap();

    // Insert first row
    engine
        .execute("INSERT INTO remat (title, code) VALUES ('first', 'X')")
        .unwrap();

    // Rematerialize (simulates reindex)
    index.rematerialize_type("remat", &repo).unwrap();

    // Duplicate should still fail after rematerialization
    let result = engine.execute("INSERT INTO remat (title, code) VALUES ('second', 'X')");
    assert!(
        result.is_err(),
        "duplicate should fail after rematerialization, got: {result:?}"
    );
}

// Regression tests for issue #5: UPDATE/DELETE fast-path returning Affected(0)
// instead of erroring when the WHERE id = 'X' target does not exist.

#[test]
fn update_with_missing_id_returns_affected_zero() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE projects (name TEXT, priority INTEGER)")
        .unwrap();
    let real_id = engine_exec_id(
        &repo,
        &index,
        "INSERT INTO projects (name, priority) VALUES ('Alpha', 1)",
    );

    let result = engine
        .execute("UPDATE projects SET priority = 5 WHERE id = 'nonexistent_id_99999999999999'")
        .expect("missing id should not error");
    match result {
        SqlResult::Affected(n) => assert_eq!(n, 0, "expected 0 rows affected"),
        other => panic!("expected Affected(0), got {other:?}"),
    }

    // Existing row must be unchanged.
    let rows = index
        .query_raw(&format!(
            "SELECT priority FROM projects WHERE id = '{real_id}'"
        ))
        .unwrap();
    assert_eq!(rows[0][0], "1", "existing row should not be touched");
}

#[test]
fn delete_with_missing_id_returns_affected_zero() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine.execute("CREATE TABLE projects (name TEXT)").unwrap();
    let real_id = engine_exec_id(
        &repo,
        &index,
        "INSERT INTO projects (name) VALUES ('Alpha')",
    );

    let result = engine
        .execute("DELETE FROM projects WHERE id = 'nonexistent_id_99999999999999'")
        .expect("missing id should not error");
    match result {
        SqlResult::Affected(n) => assert_eq!(n, 0, "expected 0 rows affected"),
        other => panic!("expected Affected(0), got {other:?}"),
    }

    // Existing row must still be present.
    let rows = index
        .query_raw(&format!(
            "SELECT COUNT(*) FROM projects WHERE id = '{real_id}'"
        ))
        .unwrap();
    assert_eq!(rows[0][0], "1", "existing row should still be present");
}

#[test]
fn update_with_in_clause_mixing_missing_and_valid_returns_affected_one() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE projects (name TEXT, priority INTEGER)")
        .unwrap();
    let real_id = engine_exec_id(
        &repo,
        &index,
        "INSERT INTO projects (name, priority) VALUES ('Alpha', 1)",
    );

    let result = engine
        .execute(&format!(
            "UPDATE projects SET priority = 9 WHERE id IN ('nonexistent', '{real_id}')"
        ))
        .expect("IN clause with mixed ids should not error");
    match result {
        SqlResult::Affected(n) => assert_eq!(n, 1, "expected 1 row affected"),
        other => panic!("expected Affected(1), got {other:?}"),
    }

    let rows = index
        .query_raw(&format!(
            "SELECT priority FROM projects WHERE id = '{real_id}'"
        ))
        .unwrap();
    assert_eq!(rows[0][0], "9");
}

#[test]
fn update_with_compound_where_nonmatching_predicate_returns_affected_zero() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE projects (name TEXT, priority INTEGER)")
        .unwrap();
    let real_id = engine_exec_id(
        &repo,
        &index,
        "INSERT INTO projects (name, priority) VALUES ('Alpha', 1)",
    );

    let result = engine
        .execute(&format!(
            "UPDATE projects SET priority = 9 WHERE id = '{real_id}' AND name = 'wrongname'"
        ))
        .expect("compound predicate non-match should not error");
    match result {
        SqlResult::Affected(n) => assert_eq!(n, 0, "expected 0 rows affected"),
        other => panic!("expected Affected(0), got {other:?}"),
    }

    let rows = index
        .query_raw(&format!(
            "SELECT priority FROM projects WHERE id = '{real_id}'"
        ))
        .unwrap();
    assert_eq!(rows[0][0], "1", "existing row should not be touched");
}

#[test]
fn update_with_valid_id_still_affects_one_row() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE projects (name TEXT, priority INTEGER)")
        .unwrap();
    let real_id = engine_exec_id(
        &repo,
        &index,
        "INSERT INTO projects (name, priority) VALUES ('Alpha', 1)",
    );

    let result = engine
        .execute(&format!(
            "UPDATE projects SET priority = 7 WHERE id = '{real_id}'"
        ))
        .expect("valid id should succeed");
    match result {
        SqlResult::Affected(n) => assert_eq!(n, 1, "expected 1 row affected"),
        other => panic!("expected Affected(1), got {other:?}"),
    }

    let rows = index
        .query_raw(&format!(
            "SELECT priority FROM projects WHERE id = '{real_id}'"
        ))
        .unwrap();
    assert_eq!(rows[0][0], "7");
}

#[test]
fn update_with_id_from_different_table_returns_affected_zero() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE projects (name TEXT, priority INTEGER)")
        .unwrap();
    engine.execute("CREATE TABLE contacts (email TEXT)").unwrap();

    let _project_id = engine_exec_id(
        &repo,
        &index,
        "INSERT INTO projects (name, priority) VALUES ('Alpha', 1)",
    );
    let contact_id = engine_exec_id(
        &repo,
        &index,
        "INSERT INTO contacts (email) VALUES ('alice@example.com')",
    );

    // UPDATE against `projects` using a contact id must NOT mutate the
    // contact — the fast path should fall through to Affected(0) because
    // no row with that id exists in the `projects` table.
    let result = engine
        .execute(&format!(
            "UPDATE projects SET priority = 99 WHERE id = '{contact_id}'"
        ))
        .expect("cross-table id should not error");
    match result {
        SqlResult::Affected(n) => assert_eq!(n, 0, "expected 0 rows affected"),
        other => panic!("expected Affected(0), got {other:?}"),
    }

    // Contact row is untouched.
    let rows = index
        .query_raw(&format!(
            "SELECT email FROM contacts WHERE id = '{contact_id}'"
        ))
        .unwrap();
    assert_eq!(
        rows[0][0], "alice@example.com",
        "contact row must not be mutated"
    );
}

#[test]
fn delete_with_id_from_different_table_returns_affected_zero() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine.execute("CREATE TABLE projects (name TEXT)").unwrap();
    engine.execute("CREATE TABLE contacts (email TEXT)").unwrap();

    let _project_id = engine_exec_id(
        &repo,
        &index,
        "INSERT INTO projects (name) VALUES ('Alpha')",
    );
    let contact_id = engine_exec_id(
        &repo,
        &index,
        "INSERT INTO contacts (email) VALUES ('alice@example.com')",
    );

    // DELETE against `projects` using a contact id must NOT delete the
    // contact — the fast path should fall through to Affected(0).
    let result = engine
        .execute(&format!(
            "DELETE FROM projects WHERE id = '{contact_id}'"
        ))
        .expect("cross-table id should not error");
    match result {
        SqlResult::Affected(n) => assert_eq!(n, 0, "expected 0 rows affected"),
        other => panic!("expected Affected(0), got {other:?}"),
    }

    // Contact row is still present.
    let rows = index
        .query_raw(&format!(
            "SELECT COUNT(*) FROM contacts WHERE id = '{contact_id}'"
        ))
        .unwrap();
    assert_eq!(rows[0][0], "1", "contact row should still exist");
}

// ── validate_row_against_schema unit tests ──────────────────────────

fn col(name: &str, data_type: &str, required: bool) -> ColumnDef {
    ColumnDef {
        name: name.into(),
        data_type: data_type.into(),
        references: None,
        zone: None,
        required,
        search_boost: None,
        allowed_values: None,
        default_value: None,
        on_delete: crate::types::OnDeleteAction::Restrict,
    }
}

fn schema_with(cols: Vec<ColumnDef>) -> TableSchema {
    TableSchema {
        table_name: "t".into(),
        columns: cols,
        crdt_strategy: None,
        template_sections: vec![],
        folder: false,
        stale_after_days: None,
        title_template: None,
        origin: None,
        unique_together: None,
        search_key: None,
    }
}

fn vals(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

fn nulls(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|s| s.to_string()).collect()
}

fn names(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

#[test]
fn validate_rejects_unknown_column_on_insert() {
    let schema = schema_with(vec![col("a", "TEXT", false)]);
    let err = SqlEngine::validate_row_against_schema(
        &schema,
        "t",
        &names(&["a", "bogus"]),
        &vals(&[("a", "x"), ("bogus", "y")]),
        &nulls(&[]),
        true,
    )
    .unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("unknown column: t.bogus"), "got: {msg}");
}

#[test]
fn validate_accepts_reserved_columns() {
    let schema = schema_with(vec![col("a", "TEXT", false)]);
    SqlEngine::validate_row_against_schema(
        &schema,
        "t",
        &names(&["a", "title", "date", "id", "type", "created_at", "updated_at", "tags"]),
        &vals(&[("a", "x")]),
        &nulls(&[]),
        true,
    )
    .unwrap();
}

#[test]
fn validate_rejects_not_null_absent_on_insert() {
    let schema = schema_with(vec![col("a", "TEXT", true), col("b", "TEXT", false)]);
    let err = SqlEngine::validate_row_against_schema(
        &schema,
        "t",
        &names(&["b"]),
        &vals(&[("b", "x")]),
        &nulls(&[]),
        true,
    )
    .unwrap_err();
    assert!(format!("{err}").contains("NOT NULL constraint violated: t.a"));
}

#[test]
fn validate_rejects_not_null_explicit_null_on_insert() {
    let schema = schema_with(vec![col("a", "TEXT", true)]);
    let err = SqlEngine::validate_row_against_schema(
        &schema,
        "t",
        &names(&["a"]),
        &vals(&[("a", "")]),
        &nulls(&["a"]),
        true,
    )
    .unwrap_err();
    assert!(format!("{err}").contains("NOT NULL constraint violated: t.a"));
}

#[test]
fn validate_accepts_not_null_with_default_already_filled() {
    let schema = schema_with(vec![col("a", "TEXT", true)]);
    // Caller filled default before this helper runs.
    SqlEngine::validate_row_against_schema(
        &schema,
        "t",
        &names(&["a"]),
        &vals(&[("a", "default-value")]),
        &nulls(&[]),
        true,
    )
    .unwrap();
}

#[test]
fn validate_update_only_rejects_explicit_null_on_required() {
    let schema = schema_with(vec![col("a", "TEXT", true)]);
    // UPDATE that doesn't touch column 'a' is fine.
    SqlEngine::validate_row_against_schema(
        &schema,
        "t",
        &names(&[]),
        &vals(&[]),
        &nulls(&[]),
        false,
    )
    .unwrap();
    // UPDATE SET a = NULL must fail.
    let err = SqlEngine::validate_row_against_schema(
        &schema,
        "t",
        &names(&["a"]),
        &vals(&[("a", "")]),
        &nulls(&["a"]),
        false,
    )
    .unwrap_err();
    assert!(format!("{err}").contains("NOT NULL constraint violated: t.a"));
}

#[test]
fn validate_rejects_integer_with_string_value() {
    let schema = schema_with(vec![col("count", "INTEGER", false)]);
    let err = SqlEngine::validate_row_against_schema(
        &schema,
        "t",
        &names(&["count"]),
        &vals(&[("count", "not_a_number")]),
        &nulls(&[]),
        true,
    )
    .unwrap_err();
    assert!(format!("{err}")
        .contains("type mismatch for t.count: expected INTEGER, got 'not_a_number'"));
}

#[test]
fn validate_accepts_integer_with_numeric_value() {
    let schema = schema_with(vec![col("count", "INTEGER", false)]);
    SqlEngine::validate_row_against_schema(
        &schema,
        "t",
        &names(&["count"]),
        &vals(&[("count", "42")]),
        &nulls(&[]),
        true,
    )
    .unwrap();
}

#[test]
fn validate_rejects_real_with_garbage() {
    let schema = schema_with(vec![col("ratio", "REAL", false)]);
    let err = SqlEngine::validate_row_against_schema(
        &schema,
        "t",
        &names(&["ratio"]),
        &vals(&[("ratio", "abc")]),
        &nulls(&[]),
        true,
    )
    .unwrap_err();
    assert!(format!("{err}").contains("type mismatch for t.ratio: expected REAL, got 'abc'"));
}

#[test]
fn validate_rejects_boolean_with_garbage() {
    let schema = schema_with(vec![col("flag", "BOOLEAN", false)]);
    let err = SqlEngine::validate_row_against_schema(
        &schema,
        "t",
        &names(&["flag"]),
        &vals(&[("flag", "maybe")]),
        &nulls(&[]),
        true,
    )
    .unwrap_err();
    assert!(format!("{err}").contains("type mismatch for t.flag: expected BOOLEAN, got 'maybe'"));
}

#[test]
fn validate_accepts_boolean_variants() {
    let schema = schema_with(vec![col("flag", "BOOLEAN", false)]);
    for variant in ["0", "1", "true", "false", "TRUE", "FALSE"] {
        SqlEngine::validate_row_against_schema(
            &schema,
            "t",
            &names(&["flag"]),
            &vals(&[("flag", variant)]),
            &nulls(&[]),
            true,
        )
        .unwrap_or_else(|e| panic!("variant {variant} should be accepted, got: {e}"));
    }
}

#[test]
fn validate_rejects_varchar_overflow() {
    let schema = schema_with(vec![col("name", "VARCHAR(10)", false)]);
    let long_value = "x".repeat(11);
    let err = SqlEngine::validate_row_against_schema(
        &schema,
        "t",
        &names(&["name"]),
        &vals(&[("name", &long_value)]),
        &nulls(&[]),
        true,
    )
    .unwrap_err();
    assert!(format!("{err}")
        .contains("value too long for t.name: 11 chars exceeds limit 10"));
}

#[test]
fn validate_accepts_varchar_within_limit() {
    let schema = schema_with(vec![col("name", "VARCHAR(10)", false)]);
    SqlEngine::validate_row_against_schema(
        &schema,
        "t",
        &names(&["name"]),
        &vals(&[("name", "0123456789")]),
        &nulls(&[]),
        true,
    )
    .unwrap();
}

#[test]
fn validate_accepts_varchar_no_length() {
    let schema = schema_with(vec![col("name", "VARCHAR", false)]);
    let big = "z".repeat(10_000);
    SqlEngine::validate_row_against_schema(
        &schema,
        "t",
        &names(&["name"]),
        &vals(&[("name", &big)]),
        &nulls(&[]),
        true,
    )
    .unwrap();
}

// ── INSERT-path enforcement (issue #7 reproducers) ────────────────────

fn count_rows(index: &Index, table: &str) -> i64 {
    let rows = index
        .query_raw(&format!("SELECT COUNT(*) FROM \"{table}\""))
        .unwrap();
    rows[0][0].parse().unwrap()
}

fn count_index_rows(index: &Index, doogat_type: &str) -> i64 {
    let rows = index
        .query_raw(&format!(
            "SELECT COUNT(*) FROM doogats WHERE type = '{doogat_type}'"
        ))
        .unwrap();
    rows[0][0].parse().unwrap()
}

#[test]
fn executesql_insert_rejects_not_null_violation() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE link (title VARCHAR(255) NOT NULL, url VARCHAR(255) NOT NULL)")
        .unwrap();

    let err = engine
        .execute("INSERT INTO link (title, url) VALUES (NULL, 'https://n.com')")
        .unwrap_err();
    assert!(
        format!("{err}").contains("NOT NULL constraint violated: link.title"),
        "got: {err}"
    );

    assert_eq!(count_rows(&index, "link"), 0, "no row should be materialized");
    assert_eq!(count_index_rows(&index, "link"), 0, "no ghost index row");
}

#[test]
fn executesql_insert_rejects_unknown_column() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE link (title VARCHAR(255) NOT NULL, url VARCHAR(255) NOT NULL)")
        .unwrap();

    let err = engine
        .execute("INSERT INTO link (title, url, unknown_col) VALUES ('t', 'https://u.com', 'dropped')")
        .unwrap_err();
    assert!(
        format!("{err}").contains("unknown column: link.unknown_col"),
        "got: {err}"
    );

    assert_eq!(count_rows(&index, "link"), 0);
    assert_eq!(count_index_rows(&index, "link"), 0);
}

#[test]
fn executesql_insert_rejects_integer_type_mismatch() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE numeric (title VARCHAR(255) NOT NULL, count INTEGER)")
        .unwrap();

    let err = engine
        .execute("INSERT INTO numeric (title, count) VALUES ('a', 'not_a_number')")
        .unwrap_err();
    assert!(
        format!("{err}").contains("type mismatch for numeric.count: expected INTEGER"),
        "got: {err}"
    );

    assert_eq!(count_rows(&index, "numeric"), 0);
    assert_eq!(count_index_rows(&index, "numeric"), 0);
}

// ── Silent title fallback removal (issue #7 sub-bullet 5) ─────────────

#[test]
fn executesql_insert_rejects_missing_title_when_required() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute(
            "CREATE TABLE link (title VARCHAR(255) NOT NULL, url VARCHAR(255), description TEXT)",
        )
        .unwrap();

    let err = engine
        .execute("INSERT INTO link (url) VALUES ('https://notitle.com')")
        .unwrap_err();
    assert!(
        format!("{err}").contains("NOT NULL constraint violated: link.title"),
        "got: {err}"
    );

    assert_eq!(count_rows(&index, "link"), 0);
    assert_eq!(count_index_rows(&index, "link"), 0);
}

#[test]
fn executesql_insert_uses_title_template_when_title_required() {
    // PRD 00122 cycle 2 (D1): a table with `title NOT NULL` AND a declared
    // `title_template` must accept INSERTs that omit the title — the
    // template fills it. Without the C2-1 fix, the validator's NOT NULL
    // check rejects before resolve_insert_title gets to run the template.
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE person (title VARCHAR(255) NOT NULL, name VARCHAR(100))")
        .unwrap();
    engine
        .execute("ALTER TABLE person SET TITLE TEMPLATE 'rendered-from-template'")
        .unwrap();

    let id = match engine
        .execute("INSERT INTO person (name) VALUES ('Alice')")
        .expect("INSERT should succeed when title_template supplies the title")
    {
        SqlResult::Ok(id) => id,
        _ => panic!("expected Ok"),
    };

    let path = index.resolve_path(&id).unwrap();
    let content = repo.read_file(&path).unwrap();
    assert!(
        content.contains("title: rendered-from-template"),
        "template should have produced title: {content}"
    );

    // Sanity: an explicit `INSERT (title) VALUES (NULL)` is still rejected,
    // even with a template — explicit NULL is a deliberate user choice.
    let err = engine
        .execute("INSERT INTO person (title, name) VALUES (NULL, 'Bob')")
        .unwrap_err();
    assert!(
        format!("{err}").contains("NOT NULL constraint violated: person.title"),
        "explicit NULL should still be rejected: {err}"
    );
}

#[test]
fn executesql_insert_uses_explicit_title() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE link (title VARCHAR(255) NOT NULL, url VARCHAR(255))")
        .unwrap();
    engine
        .execute("INSERT INTO link (title, url) VALUES ('My Bookmark', 'https://x.com')")
        .unwrap();

    let id = fetch_first_id(&index, "link");
    let rows = index
        .query_raw(&format!("SELECT title FROM link WHERE id = '{id}'"))
        .unwrap();
    assert_eq!(rows[0][0], "My Bookmark");
}

// ── Doubt-review: explicit empty string is rejected for strict types ──

#[test]
fn executesql_insert_rejects_empty_string_on_integer() {
    // Doubt review D2: with the C1 fix, the blanket empty-string skip in
    // check_column_types was tightened to numeric/bool types only. An
    // explicit `''` literal in an INTEGER column should now be rejected as
    // a type mismatch (it's not a valid i64). Empty strings on TEXT
    // columns are still accepted.
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE numeric (title VARCHAR(255) NOT NULL, count INTEGER)")
        .unwrap();

    let err = engine
        .execute("INSERT INTO numeric (title, count) VALUES ('a', '')")
        .unwrap_err();
    assert!(
        format!("{err}").contains("type mismatch for numeric.count: expected INTEGER, got ''"),
        "got: {err}"
    );
    assert_eq!(count_rows(&index, "numeric"), 0);
    assert_eq!(count_index_rows(&index, "numeric"), 0);
}

#[test]
fn executesql_insert_accepts_empty_string_on_text() {
    // Sanity: TEXT columns still accept empty strings.
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE note (title VARCHAR(255) NOT NULL, body TEXT)")
        .unwrap();
    engine
        .execute("INSERT INTO note (title, body) VALUES ('a', '')")
        .expect("empty string on TEXT must succeed");
    assert_eq!(count_rows(&index, "note"), 1);
}

// ── Multi-row INSERT atomicity (PRD 00122 blind review C2) ────────────

#[test]
fn executesql_multi_row_insert_validation_failure_writes_no_rows() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE link (title VARCHAR(255) NOT NULL, url VARCHAR(255) NOT NULL)")
        .unwrap();

    let err = engine
        .execute(
            "INSERT INTO link (title, url) VALUES \
             ('first', 'https://1.com'), \
             (NULL, 'https://2.com')",
        )
        .unwrap_err();
    assert!(
        format!("{err}").contains("NOT NULL constraint violated: link.title"),
        "got: {err}"
    );

    // Neither row should have been committed.
    assert_eq!(count_rows(&index, "link"), 0, "no row should be materialized");
    assert_eq!(count_index_rows(&index, "link"), 0, "no doogats index entry");
}

#[test]
fn executesql_multi_row_insert_validation_failure_on_third_row() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE numeric (title VARCHAR(255) NOT NULL, count INTEGER)")
        .unwrap();

    let err = engine
        .execute(
            "INSERT INTO numeric (title, count) VALUES \
             ('a', 1), \
             ('b', 2), \
             ('c', 'not_a_number')",
        )
        .unwrap_err();
    assert!(
        format!("{err}").contains("type mismatch for numeric.count: expected INTEGER"),
        "got: {err}"
    );

    assert_eq!(count_rows(&index, "numeric"), 0);
    assert_eq!(count_index_rows(&index, "numeric"), 0);
}

#[test]
fn executesql_multi_row_insert_all_valid_succeeds() {
    // Sanity: the C2 fix must not break the happy path.
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE link (title VARCHAR(255) NOT NULL, url VARCHAR(255) NOT NULL)")
        .unwrap();
    engine
        .execute(
            "INSERT INTO link (title, url) VALUES \
             ('first', 'https://1.com'), \
             ('second', 'https://2.com'), \
             ('third', 'https://3.com')",
        )
        .unwrap();

    assert_eq!(count_rows(&index, "link"), 3);
    assert_eq!(count_index_rows(&index, "link"), 3);
}

// ── Expression-synthesized NULL detection (PRD 00122 blind review C1) ─

#[test]
fn executesql_insert_rejects_coalesce_null_on_not_null() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE link (title VARCHAR(255) NOT NULL, url VARCHAR(255) NOT NULL)")
        .unwrap();

    let err = engine
        .execute(
            "INSERT INTO link (title, url) VALUES (COALESCE(NULL, NULL), 'https://x.com')",
        )
        .unwrap_err();
    assert!(
        format!("{err}").contains("NOT NULL constraint violated: link.title"),
        "got: {err}"
    );
    assert_eq!(count_rows(&index, "link"), 0);
    assert_eq!(count_index_rows(&index, "link"), 0);
}

#[test]
fn executesql_insert_rejects_ifnull_null_on_not_null_integer() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE numeric (title VARCHAR(255) NOT NULL, count INTEGER NOT NULL)")
        .unwrap();

    let err = engine
        .execute("INSERT INTO numeric (title, count) VALUES ('a', IFNULL(NULL, NULL))")
        .unwrap_err();
    assert!(
        format!("{err}").contains("NOT NULL constraint violated: numeric.count"),
        "got: {err}"
    );
    assert_eq!(count_rows(&index, "numeric"), 0);
    assert_eq!(count_index_rows(&index, "numeric"), 0);
}

#[test]
fn executesql_insert_accepts_ifnull_with_value_on_nullable() {
    // Sanity: IFNULL(NULL, 42) on a nullable INTEGER must still succeed.
    // Regression guard against the C1 fix breaking the legitimate
    // "default to a value" pattern that smoke section 23 also exercises.
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE counter (title VARCHAR(255) NOT NULL, count INTEGER)")
        .unwrap();
    engine
        .execute("INSERT INTO counter (title, count) VALUES ('a', IFNULL(NULL, 42))")
        .expect("non-NULL IFNULL result must succeed");

    let rows = index
        .query_raw("SELECT count FROM counter")
        .unwrap();
    assert_eq!(rows[0][0], "42");
}

#[test]
fn executesql_insert_accepts_nullif_on_nullable_integer() {
    // Smoke section 23 inserts NULLIF(0, 0) into a nullable INTEGER and
    // expects success. Pin that behavior here too.
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE exprtbl (title VARCHAR(255) NOT NULL, sort_order INTEGER)")
        .unwrap();
    engine
        .execute("INSERT INTO exprtbl (title, sort_order) VALUES ('a', NULLIF(0, 0))")
        .expect("NULLIF on nullable column must succeed");
}

#[test]
fn executesql_update_rejects_coalesce_null_on_not_null() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE link (title VARCHAR(255) NOT NULL, url VARCHAR(255))")
        .unwrap();
    engine
        .execute("INSERT INTO link (title, url) VALUES ('keep', 'https://x.com')")
        .unwrap();
    let id = fetch_first_id(&index, "link");

    let err = engine
        .execute(&format!(
            "UPDATE link SET title = COALESCE(NULL, NULL) WHERE id = '{id}'"
        ))
        .unwrap_err();
    assert!(
        format!("{err}").contains("NOT NULL constraint violated: link.title"),
        "got: {err}"
    );

    let rows = index
        .query_raw(&format!("SELECT title FROM link WHERE id = '{id}'"))
        .unwrap();
    assert_eq!(rows[0][0], "keep");
}

// ── UPDATE-path enforcement (issue #7 reproducers) ────────────────────

fn fetch_first_id(index: &Index, table: &str) -> String {
    let rows = index
        .query_raw(&format!("SELECT id FROM \"{table}\" LIMIT 1"))
        .unwrap();
    rows[0][0].clone()
}

#[test]
fn executesql_update_rejects_unknown_column() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE link (title VARCHAR(255) NOT NULL, url VARCHAR(255))")
        .unwrap();
    engine
        .execute("INSERT INTO link (title, url) VALUES ('hello', 'https://x.com')")
        .unwrap();
    let id = fetch_first_id(&index, "link");

    let err = engine
        .execute(&format!(
            "UPDATE link SET unknown_col = 'x' WHERE id = '{id}'"
        ))
        .unwrap_err();
    assert!(
        format!("{err}").contains("unknown column: link.unknown_col"),
        "got: {err}"
    );

    // Original row untouched.
    let rows = index
        .query_raw(&format!("SELECT title FROM link WHERE id = '{id}'"))
        .unwrap();
    assert_eq!(rows[0][0], "hello");
}

#[test]
fn executesql_update_rejects_integer_type_mismatch() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE numeric (title VARCHAR(255) NOT NULL, count INTEGER)")
        .unwrap();
    engine
        .execute("INSERT INTO numeric (title, count) VALUES ('a', 1)")
        .unwrap();
    let id = fetch_first_id(&index, "numeric");

    let err = engine
        .execute(&format!(
            "UPDATE numeric SET count = 'not_a_number' WHERE id = '{id}'"
        ))
        .unwrap_err();
    assert!(
        format!("{err}").contains("type mismatch for numeric.count: expected INTEGER"),
        "got: {err}"
    );

    let rows = index
        .query_raw(&format!("SELECT count FROM numeric WHERE id = '{id}'"))
        .unwrap();
    assert_eq!(rows[0][0], "1", "count should be unchanged after rejection");
}

#[test]
fn executesql_update_rejects_set_null_on_not_null() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE link (title VARCHAR(255) NOT NULL, url VARCHAR(255))")
        .unwrap();
    engine
        .execute("INSERT INTO link (title, url) VALUES ('hello', 'https://x.com')")
        .unwrap();
    let id = fetch_first_id(&index, "link");

    let err = engine
        .execute(&format!("UPDATE link SET title = NULL WHERE id = '{id}'"))
        .unwrap_err();
    assert!(
        format!("{err}").contains("NOT NULL constraint violated: link.title"),
        "got: {err}"
    );

    let rows = index
        .query_raw(&format!("SELECT title FROM link WHERE id = '{id}'"))
        .unwrap();
    assert_eq!(rows[0][0], "hello");
}

#[test]
fn executesql_update_rejects_varchar_overflow() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE shortname (title VARCHAR(10) NOT NULL)")
        .unwrap();
    engine
        .execute("INSERT INTO shortname (title) VALUES ('hi')")
        .unwrap();
    let id = fetch_first_id(&index, "shortname");

    let long = "x".repeat(11);
    let err = engine
        .execute(&format!(
            "UPDATE shortname SET title = '{long}' WHERE id = '{id}'"
        ))
        .unwrap_err();
    assert!(
        format!("{err}").contains("value too long for shortname.title: 11 chars exceeds limit 10"),
        "got: {err}"
    );

    let rows = index
        .query_raw(&format!("SELECT title FROM shortname WHERE id = '{id}'"))
        .unwrap();
    assert_eq!(rows[0][0], "hi");
}

#[test]
fn executesql_insert_rejects_varchar_overflow() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE shortname (title VARCHAR(10) NOT NULL)")
        .unwrap();

    let long = "x".repeat(11);
    let err = engine
        .execute(&format!(
            "INSERT INTO shortname (title) VALUES ('{long}')"
        ))
        .unwrap_err();
    assert!(
        format!("{err}").contains("value too long for shortname.title: 11 chars exceeds limit 10"),
        "got: {err}"
    );

    assert_eq!(count_rows(&index, "shortname"), 0);
    assert_eq!(count_index_rows(&index, "shortname"), 0);
}

// --- Issue #10: RESTRICT semantics for NOT NULL REFERENCES columns.
//
// Deleting a parent doogat that is referenced by a typed-table row via a
// NOT NULL REFERENCES column used to silently strip the wikilink, leaving
// the row with NULL in a NOT NULL column. We now reject the delete.

fn setup_restrict_schema(repo: &GitRepo, index: &Index) {
    let mut engine = SqlEngine::new(index, repo);
    engine
        .execute("CREATE TABLE link (url VARCHAR(255) NOT NULL)")
        .unwrap();
    engine
        .execute("CREATE TABLE category (name VARCHAR(255) NOT NULL)")
        .unwrap();
    engine
        .execute(
            "CREATE TABLE \"category-membership\" (\
                 link_id VARCHAR(255) NOT NULL REFERENCES link(id),\
                 category_id VARCHAR(255) NOT NULL REFERENCES category(id),\
                 UNIQUE(link_id, category_id)\
             )",
        )
        .unwrap();
}

fn seed_membership(repo: &GitRepo, index: &Index) -> (String, String, String) {
    let link_id = engine_exec_id(
        repo,
        index,
        "INSERT INTO link (title, url) VALUES ('L', 'https://a.com')",
    );
    let cat_id = engine_exec_id(
        repo,
        index,
        "INSERT INTO category (title, name) VALUES ('C', 'c')",
    );
    let mem_id = engine_exec_id(
        repo,
        index,
        &format!(
            "INSERT INTO \"category-membership\" (title, link_id, category_id) \
             VALUES ('M', '{link_id}', '{cat_id}')"
        ),
    );
    (link_id, cat_id, mem_id)
}

#[test]
fn delete_rejected_by_not_null_references_issue_10() {
    let (_dir, repo, index) = setup();
    setup_restrict_schema(&repo, &index);
    let (link_id, _cat_id, mem_id) = seed_membership(&repo, &index);
    let mut engine = SqlEngine::new(&index, &repo);

    let err = engine
        .execute(&format!("DELETE FROM link WHERE id = '{link_id}'"))
        .unwrap_err();
    let msg = format!("{err}");
    assert!(
        msg.contains("cannot delete")
            && msg.contains(&link_id)
            && msg.contains("category-membership")
            && msg.contains("link_id")
            && msg.contains(&mem_id),
        "error should name the deleted id, blocking table, column, and row; got: {msg}"
    );

    // Parent row still present in its typed table and in the index.
    assert_eq!(count_rows(&index, "link"), 1);
    assert_eq!(count_index_rows(&index, "link"), 1);
    // Child row still present with the FK intact.
    let rows = index
        .query_raw("SELECT link_id FROM \"category-membership\"")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0][0], link_id);
}

#[test]
fn delete_service_path_rejected_by_not_null_references_issue_10() {
    use crate::service::DoogatService;

    let (dir, repo, index) = setup();
    setup_restrict_schema(&repo, &index);
    let (link_id, _cat_id, _mem_id) = seed_membership(&repo, &index);
    // Drop local handles so DoogatService can open the repo.
    drop(repo);
    drop(index);

    let svc = DoogatService::open(dir.path()).unwrap();
    let err = svc
        .delete_doogat(&link_id, &format!("delete link {link_id}"))
        .unwrap_err();
    assert!(
        format!("{err}").contains("NOT NULL REFERENCES"),
        "got: {err}"
    );
}

#[test]
fn delete_succeeds_after_child_removed_issue_10() {
    let (_dir, repo, index) = setup();
    setup_restrict_schema(&repo, &index);
    let (link_id, _cat_id, mem_id) = seed_membership(&repo, &index);
    let mut engine = SqlEngine::new(&index, &repo);

    // Remove the child row first.
    let affected = engine
        .execute(&format!(
            "DELETE FROM \"category-membership\" WHERE id = '{mem_id}'"
        ))
        .unwrap();
    match affected {
        SqlResult::Affected(n) => assert_eq!(n, 1),
        other => panic!("expected Affected(1), got {other:?}"),
    }

    // Parent delete now succeeds.
    let affected = engine
        .execute(&format!("DELETE FROM link WHERE id = '{link_id}'"))
        .unwrap();
    match affected {
        SqlResult::Affected(n) => assert_eq!(n, 1),
        other => panic!("expected Affected(1), got {other:?}"),
    }
    assert_eq!(count_rows(&index, "link"), 0);
}

#[test]
fn delete_allowed_when_reference_is_nullable_issue_10() {
    // Without NOT NULL, the existing wikilink-strip cascade runs and the
    // parent delete proceeds. This test pins that RESTRICT only fires on
    // required FKs and does not regress nullable-reference behavior.
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);
    engine
        .execute("CREATE TABLE link (url VARCHAR(255) NOT NULL)")
        .unwrap();
    engine
        .execute(
            "CREATE TABLE bookmark (\
                 note VARCHAR(255),\
                 link_id VARCHAR(255) REFERENCES link(id)\
             )",
        )
        .unwrap();
    let link_id = engine_exec_id(
        &repo,
        &index,
        "INSERT INTO link (title, url) VALUES ('L', 'https://a.com')",
    );
    engine_exec_ok(
        &repo,
        &index,
        &format!(
            "INSERT INTO bookmark (title, note, link_id) VALUES ('B', 'n', '{link_id}')"
        ),
    );

    let affected = engine
        .execute(&format!("DELETE FROM link WHERE id = '{link_id}'"))
        .unwrap();
    match affected {
        SqlResult::Affected(n) => assert_eq!(n, 1),
        other => panic!("expected Affected(1), got {other:?}"),
    }
    assert_eq!(count_rows(&index, "link"), 0);
}

#[test]
fn bulk_delete_atomically_rejected_by_restrict_issue_10() {
    // A bulk DELETE that touches any parent with a NOT NULL REFERENCES
    // dependent must reject the whole statement without deleting any rows.
    let (_dir, repo, index) = setup();
    setup_restrict_schema(&repo, &index);
    let (link_id_blocked, _, _) = seed_membership(&repo, &index);
    let link_id_free = engine_exec_id(
        &repo,
        &index,
        "INSERT INTO link (title, url) VALUES ('L2', 'https://b.com')",
    );
    let mut engine = SqlEngine::new(&index, &repo);

    let err = engine
        .execute("DELETE FROM link WHERE url LIKE 'https://%'")
        .unwrap_err();
    assert!(
        format!("{err}").contains("NOT NULL REFERENCES"),
        "got: {err}"
    );

    // Both rows must still be there — bulk delete is atomic.
    assert_eq!(count_rows(&index, "link"), 2);
    let remaining = index
        .query_raw("SELECT id FROM link ORDER BY id")
        .unwrap();
    let mut ids: Vec<&str> = remaining.iter().map(|r| r[0].as_str()).collect();
    ids.sort();
    let mut want = vec![link_id_blocked.as_str(), link_id_free.as_str()];
    want.sort();
    assert_eq!(ids, want);
}

// --- title_template placeholder parser tests ---

#[test]
fn parse_title_template_empty_returns_no_placeholders() {
    let placeholders = parse_title_template("static text").unwrap();
    assert!(placeholders.is_empty());
}

#[test]
fn parse_title_template_bare_placeholder() {
    let placeholders = parse_title_template("{link}").unwrap();
    assert_eq!(
        placeholders,
        vec![TemplatePlaceholder {
            raw: "{link}".into(),
            col: "link".into(),
            field: None,
        }]
    );
}

#[test]
fn parse_title_template_dotted_placeholder() {
    let placeholders = parse_title_template("{link.title}").unwrap();
    assert_eq!(
        placeholders,
        vec![TemplatePlaceholder {
            raw: "{link.title}".into(),
            col: "link".into(),
            field: Some("title".into()),
        }]
    );
}

#[test]
fn parse_title_template_mixed_placeholders() {
    let placeholders = parse_title_template("{link.title} in {category.fqn}").unwrap();
    assert_eq!(placeholders.len(), 2);
    assert_eq!(placeholders[0].col, "link");
    assert_eq!(placeholders[0].field.as_deref(), Some("title"));
    assert_eq!(placeholders[1].col, "category");
    assert_eq!(placeholders[1].field.as_deref(), Some("fqn"));
}

#[test]
fn parse_title_template_rejects_multi_hop() {
    let err = parse_title_template("{a.b.c}").unwrap_err();
    let msg = format!("{err}");
    assert!(
        msg.contains("multi-hop") || msg.contains("one-level"),
        "unexpected message: {msg}"
    );
}

#[test]
fn parse_title_template_rejects_empty_segment() {
    assert!(parse_title_template("{.x}").is_err());
    assert!(parse_title_template("{x.}").is_err());
    assert!(parse_title_template("{}").is_err());
}

#[test]
fn parse_title_template_accepts_hyphen_in_identifier() {
    let placeholders = parse_title_template("{my-col}").unwrap();
    assert_eq!(placeholders[0].col, "my-col");
    let placeholders = parse_title_template("{my-col.my-field}").unwrap();
    assert_eq!(placeholders[0].col, "my-col");
    assert_eq!(placeholders[0].field.as_deref(), Some("my-field"));
}

#[test]
fn parse_title_template_rejects_identifier_starting_with_digit() {
    assert!(parse_title_template("{1col}").is_err());
    assert!(parse_title_template("{col.1field}").is_err());
}

// --- REFERENCES-aware title_template INSERT resolution ---

#[test]
fn insert_title_template_dotted_ref_resolves_target_title() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine.execute("CREATE TABLE link (url TEXT)").unwrap();
    engine
        .execute("CREATE TABLE category (fqn TEXT)")
        .unwrap();
    engine
        .execute("CREATE TABLE membership (link TEXT REFERENCES link, category TEXT REFERENCES category)")
        .unwrap();
    engine
        .execute("ALTER TABLE membership SET TITLE TEMPLATE '{link.title} in {category.fqn}'")
        .unwrap();

    let link_id = engine_exec_id(
        &repo,
        &index,
        "INSERT INTO link (title, url) VALUES ('My Link', 'https://x')",
    );
    let cat_id = engine_exec_id(
        &repo,
        &index,
        "INSERT INTO category (title, fqn) VALUES ('Cat', 'A/B')",
    );
    let mem_id = engine_exec_id(
        &repo,
        &index,
        &format!(
            "INSERT INTO membership (link, category) VALUES ('{link_id}', '{cat_id}')"
        ),
    );

    let path = index.resolve_path(&mem_id).unwrap();
    let content = repo.read_file(&path).unwrap();
    assert!(
        content.contains("My Link in A/B"),
        "dotted template should resolve: {content}"
    );
}

#[test]
fn insert_title_template_bare_ref_still_returns_id() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine.execute("CREATE TABLE link (url TEXT)").unwrap();
    engine
        .execute("CREATE TABLE membership (link TEXT REFERENCES link)")
        .unwrap();
    engine
        .execute("ALTER TABLE membership SET TITLE TEMPLATE 'raw: {link}'")
        .unwrap();

    let link_id = engine_exec_id(
        &repo,
        &index,
        "INSERT INTO link (title, url) VALUES ('My Link', 'https://x')",
    );
    let mem_id = engine_exec_id(
        &repo,
        &index,
        &format!("INSERT INTO membership (link) VALUES ('{link_id}')"),
    );

    let path = index.resolve_path(&mem_id).unwrap();
    let content = repo.read_file(&path).unwrap();
    let expected = format!("raw: {link_id}");
    assert!(
        content.contains(&expected),
        "bare ref should substitute id: {content}"
    );
}

#[test]
fn insert_title_template_dotted_ref_null_target_field_renders_empty() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE link (url TEXT, subtitle TEXT)")
        .unwrap();
    engine
        .execute("CREATE TABLE membership (link TEXT REFERENCES link)")
        .unwrap();
    engine
        .execute("ALTER TABLE membership SET TITLE TEMPLATE 'prefix {link.subtitle} suffix'")
        .unwrap();

    // Insert a link without providing subtitle — subtitle column is NULL on the target row.
    let link_id = engine_exec_id(
        &repo,
        &index,
        "INSERT INTO link (title, url) VALUES ('My Link', 'https://x')",
    );
    let mem_id = engine_exec_id(
        &repo,
        &index,
        &format!("INSERT INTO membership (link) VALUES ('{link_id}')"),
    );

    let path = index.resolve_path(&mem_id).unwrap();
    let content = repo.read_file(&path).unwrap();
    // subtitle is NULL on target → empty substitution → "prefix  suffix"
    assert!(
        content.contains("prefix  suffix") || content.contains("'prefix  suffix'"),
        "null target field should render empty: {content}"
    );
    assert!(
        !content.contains("prefix subtitle suffix"),
        "column identifier should not leak as literal: {content}"
    );
}

#[test]
fn set_title_template_rejects_dotted_on_non_ref_column() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE thing (label TEXT)")
        .unwrap();
    let err = engine
        .execute("ALTER TABLE thing SET TITLE TEMPLATE '{label.title}'")
        .unwrap_err();
    let msg = format!("{err}");
    assert!(
        msg.contains("not a REFERENCES column"),
        "expected non-ref rejection: {msg}"
    );
}

#[test]
fn set_title_template_rejects_missing_column() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine.execute("CREATE TABLE thing (label TEXT)").unwrap();
    let err = engine
        .execute("ALTER TABLE thing SET TITLE TEMPLATE '{ghost.title}'")
        .unwrap_err();
    let msg = format!("{err}");
    assert!(
        msg.contains("not found"),
        "expected column-not-found rejection: {msg}"
    );
}

#[test]
fn set_title_template_rejects_bad_field_on_target_type() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine.execute("CREATE TABLE link (url TEXT)").unwrap();
    engine
        .execute("CREATE TABLE membership (link TEXT REFERENCES link)")
        .unwrap();
    let err = engine
        .execute("ALTER TABLE membership SET TITLE TEMPLATE '{link.bogus}'")
        .unwrap_err();
    let msg = format!("{err}");
    assert!(
        msg.contains("does not exist on link"),
        "expected field-not-found rejection: {msg}"
    );
}

#[test]
fn set_title_template_accepts_title_when_target_type_not_yet_materialized() {
    // Forward reference: target type isn't materialized yet. Validation
    // should still accept `{ref.title}` because `title` is always available
    // on any typed doogat, and field-existence checks on typed columns are
    // skipped when the target schema can't be loaded (runtime falls back
    // to empty string).
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    // Note: `ghost_type` does not exist — no CREATE TABLE for it.
    engine
        .execute("CREATE TABLE stub (target TEXT REFERENCES ghost_type)")
        .unwrap();
    engine
        .execute("ALTER TABLE stub SET TITLE TEMPLATE '{target.title}'")
        .unwrap();
}

#[test]
fn set_title_template_accepts_title_on_any_ref() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine.execute("CREATE TABLE link (url TEXT)").unwrap();
    engine
        .execute("CREATE TABLE membership (link TEXT REFERENCES link)")
        .unwrap();
    engine
        .execute("ALTER TABLE membership SET TITLE TEMPLATE '{link.title}'")
        .unwrap();
}

#[test]
fn set_title_template_rejects_multi_hop() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine.execute("CREATE TABLE link (url TEXT)").unwrap();
    engine
        .execute("CREATE TABLE membership (link TEXT REFERENCES link)")
        .unwrap();
    let err = engine
        .execute("ALTER TABLE membership SET TITLE TEMPLATE '{link.other.title}'")
        .unwrap_err();
    let msg = format!("{err}");
    assert!(
        msg.contains("multi-hop") || msg.contains("one-level"),
        "expected multi-hop rejection: {msg}"
    );
}

#[test]
fn update_recomputes_title_when_ref_column_changes() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine.execute("CREATE TABLE link (url TEXT)").unwrap();
    engine
        .execute("CREATE TABLE membership (link TEXT REFERENCES link)")
        .unwrap();
    engine
        .execute("ALTER TABLE membership SET TITLE TEMPLATE '{link.title}'")
        .unwrap();

    let link_a = engine_exec_id(
        &repo,
        &index,
        "INSERT INTO link (title, url) VALUES ('Link A', 'https://a')",
    );
    let link_b = engine_exec_id(
        &repo,
        &index,
        "INSERT INTO link (title, url) VALUES ('Link B', 'https://b')",
    );
    let mem_id = engine_exec_id(
        &repo,
        &index,
        &format!("INSERT INTO membership (link) VALUES ('{link_a}')"),
    );

    engine
        .execute(&format!(
            "UPDATE membership SET link = '{link_b}' WHERE id = '{mem_id}'"
        ))
        .unwrap();

    let path = index.resolve_path(&mem_id).unwrap();
    let content = repo.read_file(&path).unwrap();
    assert!(
        content.contains("Link B"),
        "title should recompute to Link B: {content}"
    );
    assert!(
        !content.contains("Link A"),
        "old title should be gone: {content}"
    );
}

#[test]
fn update_does_not_recompute_title_when_unrelated_column_changes() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE link (url TEXT)")
        .unwrap();
    engine
        .execute("CREATE TABLE membership (link TEXT REFERENCES link, note TEXT)")
        .unwrap();
    engine
        .execute("ALTER TABLE membership SET TITLE TEMPLATE '{link.title}'")
        .unwrap();

    let link_a = engine_exec_id(
        &repo,
        &index,
        "INSERT INTO link (title, url) VALUES ('Link A', 'https://a')",
    );
    let mem_id = engine_exec_id(
        &repo,
        &index,
        &format!("INSERT INTO membership (link) VALUES ('{link_a}')"),
    );

    engine
        .execute(&format!(
            "UPDATE membership SET note = 'hello' WHERE id = '{mem_id}'"
        ))
        .unwrap();

    let path = index.resolve_path(&mem_id).unwrap();
    let content = repo.read_file(&path).unwrap();
    assert!(
        content.contains("Link A"),
        "title should stay Link A: {content}"
    );
}

#[test]
fn update_with_explicit_title_takes_priority_over_template() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine.execute("CREATE TABLE link (url TEXT)").unwrap();
    engine
        .execute("CREATE TABLE membership (link TEXT REFERENCES link)")
        .unwrap();
    engine
        .execute("ALTER TABLE membership SET TITLE TEMPLATE '{link.title}'")
        .unwrap();

    let link_a = engine_exec_id(
        &repo,
        &index,
        "INSERT INTO link (title, url) VALUES ('Link A', 'https://a')",
    );
    let link_b = engine_exec_id(
        &repo,
        &index,
        "INSERT INTO link (title, url) VALUES ('Link B', 'https://b')",
    );
    let mem_id = engine_exec_id(
        &repo,
        &index,
        &format!("INSERT INTO membership (link) VALUES ('{link_a}')"),
    );

    engine
        .execute(&format!(
            "UPDATE membership SET link = '{link_b}', title = 'Manual' WHERE id = '{mem_id}'"
        ))
        .unwrap();

    let path = index.resolve_path(&mem_id).unwrap();
    let content = repo.read_file(&path).unwrap();
    assert!(
        content.contains("Manual"),
        "explicit title should win over template: {content}"
    );
}

// --- ALTER TABLE ALTER COLUMN TYPE ---

#[test]
fn alter_column_type_widens_varchar_metadata_only() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE widen (name VARCHAR(10))")
        .unwrap();
    engine
        .execute("INSERT INTO widen (title, name) VALUES ('t1', '1234567890')")
        .unwrap();
    engine
        .execute("ALTER TABLE widen ALTER COLUMN name TYPE VARCHAR(100)")
        .unwrap();

    let schema = engine.load_schema("widen").unwrap();
    let col = schema.columns.iter().find(|c| c.name == "name").unwrap();
    assert_eq!(col.data_type, "VARCHAR(100)");

    engine
        .execute(&format!(
            "INSERT INTO widen (title, name) VALUES ('t2', '{}')",
            "x".repeat(50)
        ))
        .unwrap();
}

#[test]
fn alter_column_type_varchar_to_text_metadata_only() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE totext (url VARCHAR(255))")
        .unwrap();
    engine
        .execute(&format!(
            "INSERT INTO totext (title, url) VALUES ('t', '{}')",
            "a".repeat(255)
        ))
        .unwrap();
    engine
        .execute("ALTER TABLE totext ALTER COLUMN url TYPE TEXT")
        .unwrap();

    let schema = engine.load_schema("totext").unwrap();
    let col = schema.columns.iter().find(|c| c.name == "url").unwrap();
    assert_eq!(col.data_type, "TEXT");

    engine
        .execute(&format!(
            "INSERT INTO totext (title, url) VALUES ('t2', '{}')",
            "b".repeat(2000)
        ))
        .unwrap();
}

#[test]
fn alter_column_type_narrow_varchar_rejects_when_overflow() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE narrow_bad (name VARCHAR(100))")
        .unwrap();
    engine
        .execute(&format!(
            "INSERT INTO narrow_bad (title, name) VALUES ('t', '{}')",
            "x".repeat(50)
        ))
        .unwrap();

    let err = engine
        .execute("ALTER TABLE narrow_bad ALTER COLUMN name TYPE VARCHAR(20)")
        .unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("cannot narrow"), "{msg}");
    assert!(msg.contains("VARCHAR(20)"), "{msg}");
    assert!(msg.contains("1 existing rows"), "{msg}");
}

#[test]
fn alter_column_type_narrow_varchar_allows_when_clean() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE narrow_ok (name VARCHAR(100))")
        .unwrap();
    engine
        .execute("INSERT INTO narrow_ok (title, name) VALUES ('t', 'short')")
        .unwrap();
    engine
        .execute("ALTER TABLE narrow_ok ALTER COLUMN name TYPE VARCHAR(20)")
        .unwrap();

    let schema = engine.load_schema("narrow_ok").unwrap();
    let col = schema.columns.iter().find(|c| c.name == "name").unwrap();
    assert_eq!(col.data_type, "VARCHAR(20)");
}

#[test]
fn alter_column_type_integer_to_real_allows_when_clean() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE int_to_real (score INTEGER)")
        .unwrap();
    engine
        .execute("INSERT INTO int_to_real (title, score) VALUES ('t', 42)")
        .unwrap();
    engine
        .execute("ALTER TABLE int_to_real ALTER COLUMN score TYPE REAL")
        .unwrap();

    let schema = engine.load_schema("int_to_real").unwrap();
    let col = schema.columns.iter().find(|c| c.name == "score").unwrap();
    assert_eq!(col.data_type, "REAL");
}

#[test]
fn alter_column_type_real_to_integer_rejects_when_fractional() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE real_to_int (score REAL)")
        .unwrap();
    engine
        .execute("INSERT INTO real_to_int (title, score) VALUES ('t', 3.14)")
        .unwrap();

    let err = engine
        .execute("ALTER TABLE real_to_int ALTER COLUMN score TYPE INTEGER")
        .unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("cannot convert"), "{msg}");
    assert!(msg.contains("INTEGER"), "{msg}");
}

#[test]
fn alter_column_type_boolean_rejected() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE bool_t (flag BOOLEAN)")
        .unwrap();
    let err = engine
        .execute("ALTER TABLE bool_t ALTER COLUMN flag TYPE INTEGER")
        .unwrap_err();
    assert!(format!("{err}").contains("not supported"));
}

#[test]
fn alter_column_type_same_type_idempotent() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE idem (name VARCHAR(100))")
        .unwrap();
    let head_before = repo.head_oid().unwrap();
    engine
        .execute("ALTER TABLE idem ALTER COLUMN name TYPE VARCHAR(100)")
        .unwrap();
    let head_after = repo.head_oid().unwrap();
    assert_eq!(
        head_before.0, head_after.0,
        "idempotent ALTER must not commit a new typedef"
    );
}

#[test]
fn alter_column_type_unknown_column_errors() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE alter_miss (name TEXT)")
        .unwrap();
    let err = engine
        .execute("ALTER TABLE alter_miss ALTER COLUMN nonexistent TYPE TEXT")
        .unwrap_err();
    assert!(format!("{err}").contains("column not found"));
}

#[test]
fn alter_column_type_text_to_varchar_rejects_when_overflow() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE text_narrow (body TEXT)")
        .unwrap();
    engine
        .execute(&format!(
            "INSERT INTO text_narrow (title, body) VALUES ('t', '{}')",
            "x".repeat(500)
        ))
        .unwrap();
    let err = engine
        .execute("ALTER TABLE text_narrow ALTER COLUMN body TYPE VARCHAR(100)")
        .unwrap_err();
    assert!(format!("{err}").contains("cannot narrow"));
}

#[test]
fn alter_column_type_persists_in_typedef() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE persist (url VARCHAR(255))")
        .unwrap();
    engine
        .execute("ALTER TABLE persist ALTER COLUMN url TYPE TEXT")
        .unwrap();

    // Verify the typedef doogat on disk reflects the new type by reloading.
    let schema_after = engine.load_schema("persist").unwrap();
    let col = schema_after
        .columns
        .iter()
        .find(|c| c.name == "url")
        .unwrap();
    assert_eq!(col.data_type, "TEXT");

    // Rebuild by reading the typedef file back from git.
    let rows = index
        .query_raw("SELECT id FROM doogats WHERE type = '_typedef' AND title = 'persist'")
        .unwrap();
    let typedef_id = &rows[0][0];
    let path = format!("ddb/_typedef/{typedef_id}.md");
    let content = repo.read_file(&path).unwrap();
    assert!(
        content.contains("TEXT"),
        "typedef should contain TEXT: {content}"
    );
    assert!(
        !content.contains("VARCHAR(255)"),
        "typedef should no longer contain VARCHAR(255): {content}"
    );
}

#[test]
fn alter_column_type_references_column_rejects_non_widening() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE ref_parent (name TEXT)")
        .unwrap();
    engine
        .execute("CREATE TABLE ref_child (parent VARCHAR(32) REFERENCES ref_parent)")
        .unwrap();

    let err = engine
        .execute("ALTER TABLE ref_child ALTER COLUMN parent TYPE INTEGER")
        .unwrap_err();
    assert!(format!("{err}").contains("REFERENCES column"));
}

#[test]
fn alter_column_type_set_data_type_form_also_accepted() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE stdform (name VARCHAR(10))")
        .unwrap();
    engine
        .execute("ALTER TABLE stdform ALTER COLUMN name SET DATA TYPE VARCHAR(100)")
        .unwrap();

    let schema = engine.load_schema("stdform").unwrap();
    let col = schema.columns.iter().find(|c| c.name == "name").unwrap();
    assert_eq!(col.data_type, "VARCHAR(100)");
}


#[test]
fn alter_column_type_in_string_literal_is_not_rewritten() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine.execute("CREATE TABLE notes (body TEXT)").unwrap();
    let id = engine_exec_id(
        &repo,
        &index,
        "INSERT INTO notes (title, body) VALUES ('alter quote', 'ALTER COLUMN foo TYPE bar')",
    );

    let result = engine
        .execute(&format!("SELECT body FROM notes WHERE id = '{id}'"))
        .unwrap();
    match result {
        SqlResult::Rows { rows, .. } => {
            assert_eq!(rows[0][0], "ALTER COLUMN foo TYPE bar");
        }
        _ => panic!("expected Rows"),
    }
}

#[test]
fn alter_column_type_char_to_varchar_rejected() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE charcross (code CHAR(10))")
        .unwrap();
    let err = engine
        .execute("ALTER TABLE charcross ALTER COLUMN code TYPE VARCHAR(20)")
        .unwrap_err();
    assert!(format!("{err}").contains("not supported"));
}

#[test]
fn alter_column_type_char_widens_metadata_only() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE charwide (code CHAR(5))")
        .unwrap();
    engine
        .execute("INSERT INTO charwide (title, code) VALUES ('t', 'abcde')")
        .unwrap();
    engine
        .execute("ALTER TABLE charwide ALTER COLUMN code TYPE CHAR(20)")
        .unwrap();

    let schema = engine.load_schema("charwide").unwrap();
    let col = schema.columns.iter().find(|c| c.name == "code").unwrap();
    assert_eq!(col.data_type, "CHAR(20)");
}

#[test]
fn alter_column_type_char_narrowing_uses_char_in_error() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE charnarrow (code CHAR(20))")
        .unwrap();
    engine
        .execute("INSERT INTO charnarrow (title, code) VALUES ('t', '12345678901234567890')")
        .unwrap();
    let err = engine
        .execute("ALTER TABLE charnarrow ALTER COLUMN code TYPE CHAR(5)")
        .unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("CHAR(5)"), "expected CHAR(5) in error, got: {msg}");
    assert!(!msg.contains("VARCHAR"), "error should not mention VARCHAR: {msg}");
}

#[test]
fn alter_column_type_references_column_widens_to_text() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE ref_parent_w (name TEXT)")
        .unwrap();
    engine
        .execute("CREATE TABLE ref_child_w (parent VARCHAR(32) REFERENCES ref_parent_w)")
        .unwrap();
    engine
        .execute("ALTER TABLE ref_child_w ALTER COLUMN parent TYPE TEXT")
        .unwrap();

    let schema = engine.load_schema("ref_child_w").unwrap();
    let col = schema.columns.iter().find(|c| c.name == "parent").unwrap();
    assert_eq!(col.data_type, "TEXT");
    assert_eq!(col.references.as_deref(), Some("ref_parent_w"));
}

#[test]
fn alter_column_type_in_multi_statement_batch_does_not_corrupt_following_insert() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE batch_notes (body TEXT)")
        .unwrap();
    engine
        .execute("CREATE TABLE batch_alter (val VARCHAR(50))")
        .unwrap();

    // Single batch: ALTER first, then INSERT with the literal text in a string.
    engine
        .execute_batch(
            "ALTER TABLE batch_alter ALTER COLUMN val TYPE TEXT; \
             INSERT INTO batch_notes (title, body) VALUES ('quoted', 'ALTER COLUMN foo TYPE bar')",
        )
        .unwrap();

    let result = engine
        .execute("SELECT body FROM batch_notes WHERE title = 'quoted'")
        .unwrap();
    match result {
        SqlResult::Rows { rows, .. } => {
            assert_eq!(rows[0][0], "ALTER COLUMN foo TYPE bar");
        }
        _ => panic!("expected Rows"),
    }
}

#[test]
fn alter_column_type_shorthand_works_inside_transactional_batch() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE txn_alter (val VARCHAR(10))")
        .unwrap();

    engine
        .execute_batch(
            "BEGIN; ALTER TABLE txn_alter ALTER COLUMN val TYPE TEXT; COMMIT",
        )
        .unwrap();

    let schema = engine.load_schema("txn_alter").unwrap();
    let col = schema.columns.iter().find(|c| c.name == "val").unwrap();
    assert_eq!(col.data_type, "TEXT");
}

#[test]
fn normalize_alter_column_type_only_rewrites_alter_form() {
    use crate::sql_engine::helpers::normalize_alter_column_type;

    // Rewrite happens for the shorthand form.
    let rewritten = normalize_alter_column_type("ALTER TABLE t ALTER COLUMN c TYPE TEXT");
    assert!(rewritten.contains("SET DATA TYPE"));

    // Idempotent for the canonical form.
    let canonical = "ALTER TABLE t ALTER COLUMN c SET DATA TYPE TEXT";
    let after = normalize_alter_column_type(canonical);
    assert_eq!(after.as_ref(), canonical);

    // Rewrite is local to ALTER COLUMN context — does not touch surrounding text.
    let mixed = normalize_alter_column_type(
        "ALTER TABLE t ALTER COLUMN c TYPE VARCHAR(5); -- followed by text",
    );
    assert!(mixed.contains("SET DATA TYPE"));
}

#[test]
fn alter_column_type_core_columns_rejected() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine.execute("CREATE TABLE corecol (url TEXT)").unwrap();

    let err = engine
        .execute("ALTER TABLE corecol ALTER COLUMN title TYPE INTEGER")
        .unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("title"), "{msg}");
    assert!(msg.contains("core column"), "{msg}");

    let err = engine
        .execute("ALTER TABLE corecol ALTER COLUMN date TYPE INTEGER")
        .unwrap_err();
    assert!(format!("{err}").contains("core column"));
}

// --- SET SEARCH KEY (ddb#15 follow-up #2) ---

#[test]
fn set_search_key_persists_to_typedef() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE category (fqn VARCHAR(255), space VARCHAR(255))")
        .unwrap();
    engine
        .execute("ALTER TABLE category SET SEARCH KEY fqn")
        .unwrap();

    // _ddb_meta should now record the search key for "category".
    let v: Option<String> = index
        .conn
        .query_row(
            "SELECT value FROM _ddb_meta WHERE key = 'search_key:category'",
            [],
            |row| row.get(0),
        )
        .ok();
    assert_eq!(v.as_deref(), Some("fqn"));
}

#[test]
fn set_search_key_round_trips_through_typedef_yaml() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine.execute("CREATE TABLE thing (label TEXT)").unwrap();
    engine
        .execute("ALTER TABLE thing SET SEARCH KEY label")
        .unwrap();

    let typedef_path: String = index
        .conn
        .query_row(
            "SELECT path FROM doogats WHERE type = '_typedef' AND title = 'thing'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let content = repo.read_file(&typedef_path).unwrap();
    assert!(content.contains("search_key: label"), "{content}");
    let parsed = crate::parser::parse(&content, &typedef_path).unwrap();
    let schema = crate::sql_engine::schema_from_parsed(&parsed).unwrap();
    assert_eq!(schema.search_key.as_deref(), Some("label"));
}

#[test]
fn drop_search_key_clears_typedef() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE category (fqn VARCHAR(255))")
        .unwrap();
    engine
        .execute("ALTER TABLE category SET SEARCH KEY fqn")
        .unwrap();
    engine
        .execute("ALTER TABLE category DROP SEARCH KEY")
        .unwrap();

    let v: Option<String> = index
        .conn
        .query_row(
            "SELECT value FROM _ddb_meta WHERE key = 'search_key:category'",
            [],
            |row| row.get(0),
        )
        .ok();
    assert_eq!(v, None);
}

#[test]
fn set_search_key_rejects_missing_column() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE category (fqn TEXT)")
        .unwrap();
    let err = engine
        .execute("ALTER TABLE category SET SEARCH KEY ghost")
        .unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("not found"), "{msg}");
}

#[test]
fn set_search_key_rejects_references_column() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine.execute("CREATE TABLE link (url TEXT)").unwrap();
    engine
        .execute("CREATE TABLE membership (link TEXT REFERENCES link)")
        .unwrap();
    let err = engine
        .execute("ALTER TABLE membership SET SEARCH KEY link")
        .unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("REFERENCES"), "{msg}");
}

#[test]
fn sql_insert_populates_auto_junction_for_references_column() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    // Create category typedef first so the REFERENCES target exists.
    engine
        .execute("CREATE TABLE category (label VARCHAR(100))")
        .unwrap();
    engine
        .execute("CREATE TABLE bookmark (url TEXT, category TEXT REFERENCES category)")
        .unwrap();

    // Create a category row and capture its id.
    let cat_id = match engine
        .execute("INSERT INTO category (label) VALUES ('tech')")
        .unwrap()
    {
        SqlResult::Ok(id) => id,
        other => panic!("expected Ok, got {other:?}"),
    };

    // Insert a bookmark with the category column populated in the same statement.
    let bm_id = match engine
        .execute(&format!(
            "INSERT INTO bookmark (url, category) VALUES ('https://example.com', '{cat_id}')"
        ))
        .unwrap()
    {
        SqlResult::Ok(id) => id,
        other => panic!("expected Ok, got {other:?}"),
    };

    // The auto-junction table should be populated atomically with the INSERT.
    let rows = index
        .query_raw(&format!(
            "SELECT category_id FROM bookmark_category WHERE bookmark_id = '{bm_id}'"
        ))
        .unwrap();
    assert_eq!(
        rows.len(),
        1,
        "auto-junction bookmark_category should have exactly one row after INSERT"
    );
    assert_eq!(rows[0][0], cat_id);
}

#[test]
fn sql_update_syncs_auto_junction_when_references_changes() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    // Create category typedef with a label column.
    engine
        .execute("CREATE TABLE category (label VARCHAR(100))")
        .unwrap();
    // Create bookmark typedef referencing category.
    engine
        .execute("CREATE TABLE bookmark (url TEXT, category TEXT REFERENCES category)")
        .unwrap();

    // Insert two distinct category rows.
    let cat_a_id = match engine
        .execute("INSERT INTO category (label) VALUES ('alpha')")
        .unwrap()
    {
        SqlResult::Ok(id) => id,
        other => panic!("expected Ok, got {other:?}"),
    };
    let cat_b_id = match engine
        .execute("INSERT INTO category (label) VALUES ('beta')")
        .unwrap()
    {
        SqlResult::Ok(id) => id,
        other => panic!("expected Ok, got {other:?}"),
    };
    assert_ne!(cat_a_id, cat_b_id, "categories must have distinct ids");

    // Insert a bookmark pointing at category A.
    let bm_id = match engine
        .execute(&format!(
            "INSERT INTO bookmark (url, category) VALUES ('https://example.com', '{cat_a_id}')"
        ))
        .unwrap()
    {
        SqlResult::Ok(id) => id,
        other => panic!("expected Ok, got {other:?}"),
    };

    // Sanity-check post-INSERT junction state.
    let initial = index
        .query_raw(&format!(
            "SELECT category_id FROM bookmark_category WHERE bookmark_id = '{bm_id}'"
        ))
        .unwrap();
    assert_eq!(
        initial.len(),
        1,
        "auto-junction should have exactly one row after INSERT"
    );
    assert_eq!(initial[0][0], cat_a_id);

    // UPDATE the bookmark's category to point at B.
    match engine
        .execute(&format!(
            "UPDATE bookmark SET category = '{cat_b_id}' WHERE id = '{bm_id}'"
        ))
        .unwrap()
    {
        SqlResult::Affected(n) => assert_eq!(n, 1, "expected one row affected"),
        other => panic!("expected Affected, got {other:?}"),
    }

    // Old junction row pointing at cat_a_id must be gone.
    let old_count = index
        .query_raw(&format!(
            "SELECT COUNT(*) FROM bookmark_category WHERE bookmark_id = '{bm_id}' AND category_id = '{cat_a_id}'"
        ))
        .unwrap();
    assert_eq!(
        old_count[0][0], "0",
        "stale junction row pointing at previous category must be removed"
    );

    // New junction row pointing at cat_b_id must be present.
    let new_count = index
        .query_raw(&format!(
            "SELECT COUNT(*) FROM bookmark_category WHERE bookmark_id = '{bm_id}' AND category_id = '{cat_b_id}'"
        ))
        .unwrap();
    assert_eq!(
        new_count[0][0], "1",
        "new junction row pointing at updated category must be inserted"
    );

    // Total junction rows for this bookmark must be exactly 1.
    let total = index
        .query_raw(&format!(
            "SELECT COUNT(*) FROM bookmark_category WHERE bookmark_id = '{bm_id}'"
        ))
        .unwrap();
    assert_eq!(
        total[0][0], "1",
        "auto-junction must hold exactly one row for the bookmark after UPDATE"
    );
}

/// PRD 00134 cycle-1 review C1 task #3: `UPDATE … SET col = NULL` on a
/// REFERENCES column must clear the junction (no row), not leave an
/// empty-string `<col>_id` ghost. Today `expr_to_string` collapses NULL
/// to "", `update_reference_line` writes `- col:: [[]]`,
/// `extract_multi_reference_values` returns `[""]`, and the helper would
/// `INSERT (parent_id, '')` into the junction. Filtering empty/whitespace
/// in `extract_multi_reference_values` gives a single uniform fix for
/// every caller (insert + sync + extract_column_values).
#[test]
fn sql_update_set_null_on_references_clears_auto_junction() {
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE category (label VARCHAR(100))")
        .unwrap();
    engine
        .execute("CREATE TABLE bookmark (url TEXT, category TEXT REFERENCES category)")
        .unwrap();

    let cat_id = match engine
        .execute("INSERT INTO category (label) VALUES ('alpha')")
        .unwrap()
    {
        SqlResult::Ok(id) => id,
        other => panic!("expected Ok, got {other:?}"),
    };
    let bm_id = match engine
        .execute(&format!(
            "INSERT INTO bookmark (url, category) VALUES ('https://example.com', '{cat_id}')"
        ))
        .unwrap()
    {
        SqlResult::Ok(id) => id,
        other => panic!("expected Ok, got {other:?}"),
    };

    // UPDATE … SET category = NULL.
    match engine
        .execute(&format!(
            "UPDATE bookmark SET category = NULL WHERE id = '{bm_id}'"
        ))
        .unwrap()
    {
        SqlResult::Affected(n) => assert_eq!(n, 1, "expected one row affected"),
        other => panic!("expected Affected, got {other:?}"),
    }

    // Junction must be empty for this bookmark — no rows at all, and
    // certainly no empty-string category_id ghost.
    let total = index
        .query_raw(&format!(
            "SELECT COUNT(*) FROM bookmark_category WHERE bookmark_id = '{bm_id}'"
        ))
        .unwrap();
    assert_eq!(
        total[0][0], "0",
        "SET col = NULL on a REFERENCES column must clear the junction (no rows, no empty-string ghost)"
    );

    // Materialized row should now read NULL/empty for category. (It
    // existed pre-update; only the FK changed.)
    let cat_after = index
        .query_raw(&format!(
            "SELECT category FROM bookmark WHERE id = '{bm_id}'"
        ))
        .unwrap();
    assert_eq!(
        cat_after.len(),
        1,
        "bookmark row must still exist after SET NULL"
    );
    assert!(
        cat_after[0][0].is_empty() || cat_after[0][0] == "NULL",
        "bookmark.category must be NULL/empty after SET NULL, got: {:?}",
        cat_after[0][0]
    );
}

#[test]
fn sql_delete_referenced_target_clears_auto_junction_rows() {
    // Regression coverage for T3 / PRD 00134 §"Non-goals" verification:
    // when the *referenced target* of a REFERENCES column is deleted,
    // `cascade_junction_cleanup` must remove auto-junction rows that
    // mention it. The owning-parent direction is handled separately
    // (it's not the same code path) and is tracked outside this PRD.
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE category (label VARCHAR(100))")
        .unwrap();
    engine
        .execute("CREATE TABLE bookmark (url TEXT, category TEXT REFERENCES category)")
        .unwrap();

    let cat_id = match engine
        .execute("INSERT INTO category (label) VALUES ('tech')")
        .unwrap()
    {
        SqlResult::Ok(id) => id,
        other => panic!("expected Ok, got {other:?}"),
    };
    let bm_id = match engine
        .execute(&format!(
            "INSERT INTO bookmark (url, category) VALUES ('https://example.com', '{cat_id}')"
        ))
        .unwrap()
    {
        SqlResult::Ok(id) => id,
        other => panic!("expected Ok, got {other:?}"),
    };

    // Sanity: junction populated by T1's INSERT path.
    let initial = index
        .query_raw(&format!(
            "SELECT category_id FROM bookmark_category WHERE bookmark_id = '{bm_id}'"
        ))
        .unwrap();
    assert_eq!(initial.len(), 1, "junction should be populated post-INSERT");

    // Delete the *category* (referenced target). cascade_junction_cleanup
    // sweeps every junction column whose `references` equals the deleted
    // type, so the bookmark_category row mentioning it must disappear.
    engine
        .execute(&format!("DELETE FROM category WHERE id = '{cat_id}'"))
        .unwrap();

    let after_target_delete = index
        .query_raw(&format!(
            "SELECT COUNT(*) FROM bookmark_category WHERE category_id = '{cat_id}'"
        ))
        .unwrap();
    assert_eq!(
        after_target_delete[0][0], "0",
        "junction rows must be removed when the referenced typed row is deleted (cascade)"
    );
}

#[test]
fn sql_delete_parent_clears_owned_auto_junction_rows() {
    // PRD 00137: when the *owning parent* of an auto-junction row is deleted
    // (`DELETE FROM bookmark WHERE id = '<bm>'`), the junction rows owned by
    // that id (`bookmark_category WHERE bookmark_id = '<bm>'`) must be
    // removed. Complements the reverse-direction sweep covered by
    // `sql_delete_referenced_target_clears_auto_junction_rows`. Trimmed out
    // of PRD 00134 (which scoped INSERT/UPDATE atomicity, not DELETE).
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE category (label VARCHAR(100))")
        .unwrap();
    engine
        .execute("CREATE TABLE bookmark (url TEXT, category TEXT REFERENCES category)")
        .unwrap();

    let cat_id = match engine
        .execute("INSERT INTO category (label) VALUES ('alpha')")
        .unwrap()
    {
        SqlResult::Ok(id) => id,
        other => panic!("expected Ok, got {other:?}"),
    };
    let bm_id = match engine
        .execute(&format!(
            "INSERT INTO bookmark (url, category) VALUES ('https://example.com', '{cat_id}')"
        ))
        .unwrap()
    {
        SqlResult::Ok(id) => id,
        other => panic!("expected Ok, got {other:?}"),
    };

    // Sanity: the owner-side junction row is in place post-INSERT.
    let initial = index
        .query_raw(&format!(
            "SELECT COUNT(*) FROM bookmark_category WHERE bookmark_id = '{bm_id}'"
        ))
        .unwrap();
    assert_eq!(
        initial[0][0], "1",
        "junction must hold the owner-side row before parent delete"
    );

    // Delete the *bookmark* (parent of the junction row).
    engine
        .execute(&format!("DELETE FROM bookmark WHERE id = '{bm_id}'"))
        .unwrap();

    let after_parent_delete = index
        .query_raw(&format!(
            "SELECT COUNT(*) FROM bookmark_category WHERE bookmark_id = '{bm_id}'"
        ))
        .unwrap();
    assert_eq!(
        after_parent_delete[0][0], "0",
        "junction rows must be removed when their owning parent row is deleted (cascade)"
    );
}

#[test]
fn sql_update_single_row_rolls_back_materialized_when_junction_sync_fails() {
    // PRD 00134 cycle-1 review (C1): the WHERE-id fast path in
    // `apply_single_row_update` calls `update_materialized_row` followed by
    // `sync_junction_tables_for_columns` without a SAVEPOINT. If the junction
    // sync fails, the materialized-row update is left half-applied: SQLite
    // reflects the new value while the operation as a whole errored. The fix
    // wraps both calls in a SAVEPOINT so a sync failure rolls back the
    // materialized update.
    //
    // We force sync failure by dropping the auto-junction table after the row
    // exists. The subsequent UPDATE on a row that touches the REFERENCES
    // column will:
    //   1. Update the materialized `bookmark` row (succeeds).
    //   2. Try to sync `bookmark_category` (fails: table dropped).
    // With the bug: error propagates, but `url` shows the new value.
    // With the fix: SAVEPOINT rollback restores `url` to the old value.
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE category (label VARCHAR(100))")
        .unwrap();
    engine
        .execute("CREATE TABLE bookmark (url TEXT, category TEXT REFERENCES category)")
        .unwrap();

    let cat_a_id = match engine
        .execute("INSERT INTO category (label) VALUES ('alpha')")
        .unwrap()
    {
        SqlResult::Ok(id) => id,
        other => panic!("expected Ok, got {other:?}"),
    };
    let cat_b_id = match engine
        .execute("INSERT INTO category (label) VALUES ('beta')")
        .unwrap()
    {
        SqlResult::Ok(id) => id,
        other => panic!("expected Ok, got {other:?}"),
    };

    let bm_id = match engine
        .execute(&format!(
            "INSERT INTO bookmark (url, category) VALUES ('https://old.example.com', '{cat_a_id}')"
        ))
        .unwrap()
    {
        SqlResult::Ok(id) => id,
        other => panic!("expected Ok, got {other:?}"),
    };

    // Force `sync_junction_tables_for_columns` to fail by dropping the
    // auto-junction table. The materialized `bookmark` UPDATE will still
    // succeed (it touches `bookmark`, not `bookmark_category`), so the
    // failure window is exactly the one the SAVEPOINT must cover.
    engine
        .index
        .sql_conn()
        .execute("DROP TABLE \"bookmark_category\"", [])
        .unwrap();

    // WHERE-id fast path (single-row): UPDATE both a regular column AND the
    // REFERENCES column.
    let result = engine.execute(&format!(
        "UPDATE bookmark SET url = 'https://new.example.com', category = '{cat_b_id}' WHERE id = '{bm_id}'"
    ));
    assert!(
        result.is_err(),
        "UPDATE must error when junction sync fails, got: {result:?}"
    );

    // Materialized row's regular column must be the OLD value: the SAVEPOINT
    // rollback should have undone the SQLite UPDATE on `bookmark` when the
    // subsequent junction-sync DELETE on the dropped `bookmark_category`
    // failed.
    let url: String = engine
        .index
        .sql_conn()
        .query_row(
            "SELECT url FROM bookmark WHERE id = ?1",
            params![bm_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        url, "https://old.example.com",
        "materialized url must roll back to OLD value when junction sync fails (single-row path)"
    );
}

#[test]
fn sql_update_bulk_rolls_back_materialized_when_junction_sync_fails() {
    // Same atomicity invariant as the single-row test, but exercising
    // `update_bulk_rows` (the WHERE clause does not match the
    // `WHERE id = '<literal>'` fast path, so `extract_where_id` fails and
    // execution falls through to `resolve_matching_ids`). The bulk-loop
    // body in `update_bulk_rows` likewise calls `update_materialized_row`
    // and then `sync_junction_tables_for_columns` without a SAVEPOINT.
    let (_dir, repo, index) = setup();
    let mut engine = SqlEngine::new(&index, &repo);

    engine
        .execute("CREATE TABLE category (label VARCHAR(100))")
        .unwrap();
    engine
        .execute("CREATE TABLE bookmark (url TEXT, category TEXT REFERENCES category)")
        .unwrap();

    let cat_a_id = match engine
        .execute("INSERT INTO category (label) VALUES ('alpha')")
        .unwrap()
    {
        SqlResult::Ok(id) => id,
        other => panic!("expected Ok, got {other:?}"),
    };
    let cat_b_id = match engine
        .execute("INSERT INTO category (label) VALUES ('beta')")
        .unwrap()
    {
        SqlResult::Ok(id) => id,
        other => panic!("expected Ok, got {other:?}"),
    };

    let bm_id = match engine
        .execute(&format!(
            "INSERT INTO bookmark (url, category) VALUES ('https://old.example.com', '{cat_a_id}')"
        ))
        .unwrap()
    {
        SqlResult::Ok(id) => id,
        other => panic!("expected Ok, got {other:?}"),
    };

    engine
        .index
        .sql_conn()
        .execute("DROP TABLE \"bookmark_category\"", [])
        .unwrap();

    // Bulk path: `WHERE id IN (...)` is not the `id = '<literal>'` fast path,
    // so `extract_where_id` returns Err and `update_bulk_rows` runs.
    let result = engine.execute(&format!(
        "UPDATE bookmark SET url = 'https://new.example.com', category = '{cat_b_id}' WHERE id IN ('{bm_id}')"
    ));
    assert!(
        result.is_err(),
        "bulk UPDATE must error when junction sync fails, got: {result:?}"
    );

    let url: String = engine
        .index
        .sql_conn()
        .query_row(
            "SELECT url FROM bookmark WHERE id = ?1",
            params![bm_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        url, "https://old.example.com",
        "materialized url must roll back to OLD value when junction sync fails (bulk path)"
    );
}
