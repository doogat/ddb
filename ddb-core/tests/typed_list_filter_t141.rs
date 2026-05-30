use ddb_core::app_contract::CreateCommand;
use ddb_core::git_ops::GitRepo;
use ddb_core::indexer::Index;
use ddb_core::service::DoogatService;
use ddb_core::types::{ConflictAction, QueryValue, TypedListQuery, Value};
use std::collections::BTreeMap;
use tempfile::TempDir;

// ── Index fixture ─────────────────────────────────────────────────────────────

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

// ── DoogatService fixture ─────────────────────────────────────────────────────

fn init_service(dir: &std::path::Path) -> DoogatService {
    DoogatService::init(dir).expect("init repo")
}

// ── Index::query_raw_with_params tests ────────────────────────────────────────

#[test]
fn query_raw_with_params_text_param_filters_by_type() {
    let (_dir, repo, index) = setup();
    seed_doogat(&repo, &index, "20260101000001", "note", "Note One");
    seed_doogat(&repo, &index, "20260101000002", "task", "Task One");

    let rows = index
        .query_raw_with_params(
            "SELECT id FROM doogats WHERE type = ?1",
            &[rusqlite::types::Value::Text("note".to_string())],
        )
        .expect("query should succeed");

    assert_eq!(rows.len(), 1, "expected exactly 1 row for type=note");
    assert_eq!(
        rows[0][0], "20260101000001",
        "the returned id should belong to the note doogat"
    );
}

#[test]
fn query_raw_with_params_integer_param_limits_result_count() {
    let (_dir, repo, index) = setup();
    seed_doogat(&repo, &index, "20260101000001", "a", "Alpha");
    seed_doogat(&repo, &index, "20260101000002", "b", "Beta");
    seed_doogat(&repo, &index, "20260101000003", "c", "Gamma");

    let rows = index
        .query_raw_with_params(
            "SELECT id FROM doogats ORDER BY id DESC LIMIT ?1",
            &[rusqlite::types::Value::Integer(2)],
        )
        .expect("query should succeed");

    assert_eq!(rows.len(), 2, "LIMIT ?1=2 should return exactly 2 rows");
}

#[test]
fn query_raw_with_params_empty_params_returns_all_rows() {
    let (_dir, repo, index) = setup();
    seed_doogat(&repo, &index, "20260101000001", "note", "First");
    seed_doogat(&repo, &index, "20260101000002", "note", "Second");

    let rows = index
        .query_raw_with_params("SELECT id FROM doogats", &[])
        .expect("query should succeed");

    assert_eq!(rows.len(), 2, "no filter should return all 2 seeded rows");
}

// ── DoogatService::aggregate_query tests ─────────────────────────────────────

#[test]
fn aggregate_query_count_with_text_param_returns_count_as_string() {
    let tmp = tempfile::tempdir().unwrap();
    let mut svc = init_service(tmp.path());

    svc.execute_sql("CREATE TABLE note (summary TEXT)")
        .expect("create table");

    svc.create(CreateCommand {
        title: Some("T".into()),
        body: None,
        tags: vec![],
        doogat_type: Some("note".into()),
        fields: BTreeMap::new(),
        on_conflict: ConflictAction::Error,
    })
    .expect("create should succeed");

    let result = svc
        .aggregate_query(
            "SELECT COUNT(*) FROM doogats WHERE type = ?1",
            &[QueryValue::Text("note".to_string())],
        )
        .expect("aggregate_query should succeed");

    assert_eq!(
        result,
        vec!["1".to_string()],
        "COUNT(*) for type=note should be 1"
    );
}

// ── DoogatService::typed_filtered_list tests ─────────────────────────────────

#[test]
fn typed_filtered_list_text_param_in_where_sql_matches_typed_row() {
    let tmp = tempfile::tempdir().unwrap();
    let mut svc = init_service(tmp.path());

    svc.execute_sql("CREATE TABLE bookmark (url TEXT NOT NULL)")
        .expect("create table");

    let mut fields = BTreeMap::new();
    fields.insert(
        "url".to_string(),
        Value::String("https://example.com".to_string()),
    );
    svc.create(CreateCommand {
        title: Some("Example".into()),
        body: None,
        tags: vec![],
        doogat_type: Some("bookmark".into()),
        fields,
        on_conflict: ConflictAction::Error,
    })
    .expect("create should succeed");

    let query = TypedListQuery {
        table_name: "bookmark".to_string(),
        where_sql: "url = ?1".to_string(),
        params: vec![QueryValue::Text("https://example.com".to_string())],
        order_sql: None,
        tag: None,
        limit: None,
        offset: None,
        distinct: None,
    };

    let doogats = svc
        .typed_filtered_list(&query)
        .expect("typed_filtered_list should succeed");

    assert_eq!(
        doogats.len(),
        1,
        "exactly 1 bookmark with the given url should be returned"
    );
}
