use ddb_core::git_ops::GitRepo;
use ddb_core::indexer::Index;
use ddb_core::types::QueryValue;
use tempfile::TempDir;

fn setup() -> (TempDir, GitRepo, Index) {
    let dir = TempDir::new().unwrap();
    let repo = GitRepo::init(dir.path()).unwrap();
    let db_path = dir.path().join(".ddb/index.db");
    std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
    let index = Index::open(&db_path).unwrap();
    (dir, repo, index)
}

fn seed_doogat(repo: &GitRepo, index: &Index, id: &str, doogat_type: &str, title: &str) {
    let content = format!("---\ntitle: {title}\ntype: {doogat_type}\n---\n\nbody\n");
    let path = format!("ddb/{id}.md");
    repo.commit_file(&path, &content, "test: add").unwrap();
    let parsed = ddb_core::parser::parse(&content, &path).unwrap();
    index.index_doogat(&parsed).unwrap();
}

#[test]
fn query_raw_with_query_values_text_param_filters_by_type() {
    let (_dir, repo, index) = setup();
    seed_doogat(&repo, &index, "20260101000001", "note", "Note One");
    seed_doogat(&repo, &index, "20260101000002", "task", "Task One");

    let rows = index
        .query_raw_with_query_values(
            "SELECT id FROM doogats WHERE type = ?1",
            &[QueryValue::Text("note".into())],
        )
        .unwrap();

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0][0], "20260101000001");
}

#[test]
fn query_raw_with_query_values_integer_param_limits_result_count() {
    let (_dir, repo, index) = setup();
    seed_doogat(&repo, &index, "20260101000001", "note", "Note One");
    seed_doogat(&repo, &index, "20260101000002", "task", "Task One");
    seed_doogat(&repo, &index, "20260101000003", "project", "Project One");

    let rows = index
        .query_raw_with_query_values(
            "SELECT id FROM doogats ORDER BY id LIMIT ?1",
            &[QueryValue::Integer(2)],
        )
        .unwrap();

    assert_eq!(rows.len(), 2);
}

#[test]
fn query_raw_with_query_values_empty_params_returns_all_rows() {
    let (_dir, repo, index) = setup();
    seed_doogat(&repo, &index, "20260101000001", "note", "Note One");
    seed_doogat(&repo, &index, "20260101000002", "task", "Task One");

    let rows = index
        .query_raw_with_query_values("SELECT id FROM doogats", &[])
        .unwrap();

    assert_eq!(rows.len(), 2);
}

#[test]
fn query_raw_with_query_values_null_param_matches_null_column() {
    let (_dir, repo, index) = setup();
    seed_doogat(&repo, &index, "20260101000001", "note", "Note Without Date");

    let rows = index
        .query_raw_with_query_values(
            "SELECT id FROM doogats WHERE date IS ?1",
            &[QueryValue::Null],
        )
        .unwrap();

    assert!(rows.len() >= 1);
}

#[test]
fn query_raw_with_query_values_produces_same_rows_as_query_raw_with_params() {
    use rusqlite;

    let (_dir, repo, index) = setup();
    seed_doogat(&repo, &index, "20260101000001", "note", "Note One");
    seed_doogat(&repo, &index, "20260101000002", "task", "Task One");

    let sql = "SELECT id FROM doogats WHERE type = ?1";

    let rows_legacy = index
        .query_raw_with_params(sql, &[rusqlite::types::Value::Text("note".into())])
        .unwrap();

    let rows_new = index
        .query_raw_with_query_values(sql, &[QueryValue::Text("note".into())])
        .unwrap();

    assert_eq!(rows_legacy, rows_new);
}
