
use super::helpers::{data_type_to_string, eval_expr, is_literal_expr, value_to_sql};
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

    // Insert with non-existent reference should fail
    let err = engine
        .execute("INSERT INTO tasks (name, assignee) VALUES ('Fix bug', '99999999999999')")
        .unwrap_err();
    assert!(format!("{err}").contains("referenced doogat not found"));
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
    let (_dir, repo, _index) = setup();
    // Simple CTE that references no real table so we don't depend on schema.
    let mut engine = SqlEngine::new(&_index, &repo);
    engine
        .execute("CREATE TABLE cte_probe_t (label VARCHAR(255))")
        .unwrap();
    engine
        .execute("INSERT INTO cte_probe_t (title, label) VALUES ('row', 'x')")
        .unwrap();
    let result = _index
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

    // Delete the bookmark (no REFERENCES point TO bookmark, only FROM it)
    engine
        .execute(&format!("DELETE FROM bookmark WHERE id = '{bm_id}'"))
        .unwrap();

    // Junction row should NOT be cascade-deleted by the bookmark delete,
    // because the cascade targets the referenced type (category), not the
    // referencing type (bookmark). The junction row cleanup for the
    // "owner" side is a separate concern (write-through).
    // However, the category_id junction entry should remain intact.
    let rows = index
        .query_raw(&format!(
            "SELECT COUNT(*) FROM bookmark_category WHERE category_id = '{cat_id}'"
        ))
        .unwrap();
    assert_eq!(
            rows[0][0], "1",
            "junction row should not be affected when deleting a doogat of a type that is not referenced"
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
