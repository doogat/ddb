use crate::common::{DdbTestRepo, ServerGuard};

/// Run a `SELECT ...` via `executeSql` and return the first row's first
/// column as a string. See integration_sql_writes.rs for the row shape.
fn select_scalar(server: &ServerGuard, sql: &str) -> String {
    let escaped = sql.replace('"', "\\\"");
    let query = format!(r#"mutation {{ executeSql(sql: "{escaped}") {{ rows }} }}"#);
    let result = server.graphql(&query);
    assert!(result.get("errors").is_none(), "SELECT failed: {result}");
    let rows = result["data"]["executeSql"]["rows"].as_array().unwrap();
    let row_json = rows[0].as_str().unwrap();
    let row: Vec<String> = serde_json::from_str(row_json).unwrap();
    row[0].clone()
}

/// Create `g12item` typedef via SQL, patch it to add unique_together on
/// `code`, git commit, and return the repo ready for server start. Mirrors
/// upsert.rs's `setup_repo_with_unique_constraint` approach exactly.
fn setup_g12item_with_unique_code() -> DdbTestRepo {
    let repo = DdbTestRepo::init();

    repo.ddb()
        .args(["query", "CREATE TABLE g12item (code TEXT, label TEXT)"])
        .assert()
        .success();

    let typedef_dir = repo.path().join("ddb/_typedef");
    let typedef_file = std::fs::read_dir(&typedef_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .find(|e| {
            let content = std::fs::read_to_string(e.path()).unwrap_or_default();
            content.contains("title: g12item")
        })
        .expect("g12item typedef not found");

    let content = std::fs::read_to_string(typedef_file.path()).unwrap();
    let patched = content.replace(
        "type: _typedef",
        "type: _typedef\nunique_together:\n  - - code",
    );
    std::fs::write(typedef_file.path(), &patched).unwrap();

    std::process::Command::new("git")
        .current_dir(repo.path())
        .args(["add", "-A"])
        .output()
        .unwrap();
    std::process::Command::new("git")
        .current_dir(repo.path())
        .args(["commit", "-m", "add unique_together to g12item typedef"])
        .output()
        .unwrap();

    repo
}

#[test]
fn integration_44_ddl_response_consistency() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    let create = server.graphql(
        r#"mutation { executeSql(sql: "CREATE TABLE ddltest (name VARCHAR(100))") { columns rows message } }"#,
    );
    assert!(
        create.get("errors").is_none(),
        "CREATE TABLE failed: {create}"
    );
    assert_eq!(
        create["data"]["executeSql"]["columns"],
        serde_json::json!([])
    );
    assert_eq!(create["data"]["executeSql"]["rows"], serde_json::json!([]));
    assert!(
        !create["data"]["executeSql"]["message"].is_null(),
        "CREATE should return a non-null message: {create}"
    );

    let alter = server.graphql(
        r#"mutation { executeSql(sql: "ALTER TABLE ddltest ADD COLUMN age INTEGER") { columns rows message } }"#,
    );
    assert!(
        alter.get("errors").is_none(),
        "ALTER TABLE failed: {alter}"
    );
    assert_eq!(
        alter["data"]["executeSql"]["columns"],
        serde_json::json!([])
    );
    assert_eq!(alter["data"]["executeSql"]["rows"], serde_json::json!([]));

    let drop = server.graphql(
        r#"mutation { executeSql(sql: "DROP TABLE ddltest") { columns rows message } }"#,
    );
    assert!(drop.get("errors").is_none(), "DROP TABLE failed: {drop}");
    assert_eq!(
        drop["data"]["executeSql"]["columns"],
        serde_json::json!([])
    );
    assert_eq!(drop["data"]["executeSql"]["rows"], serde_json::json!([]));

    let batch = server.graphql(
        r#"mutation { executeBatch(statements: ["CREATE TABLE ddlbatch1 (name VARCHAR)", "CREATE TABLE ddlbatch2 (val INTEGER)"]) { columns rows message } }"#,
    );
    assert!(
        batch.get("errors").is_none(),
        "executeBatch DDL failed: {batch}"
    );
    let batch_results = batch["data"]["executeBatch"].as_array().unwrap();
    assert_eq!(
        batch_results.len(),
        2,
        "executeBatch should return one result per statement: {batch}"
    );
    for r in batch_results {
        assert_eq!(r["columns"], serde_json::json!([]));
        assert_eq!(r["rows"], serde_json::json!([]));
    }

    // Regression: DDL response-shape changes must not affect DML response shape
    let create_dml_table = server.graphql(
        r#"mutation { executeSql(sql: "CREATE TABLE datecheck (name TEXT)") { message } }"#,
    );
    assert!(
        create_dml_table.get("errors").is_none(),
        "CREATE datecheck failed: {create_dml_table}"
    );
    let dml = server.graphql(
        r#"mutation { executeSql(sql: "INSERT INTO datecheck (name) VALUES (\"DmlRegression\")") { affected message } }"#,
    );
    assert!(dml.get("errors").is_none(), "DML insert failed: {dml}");
    assert!(
        !dml["data"]["executeSql"]["message"].is_null(),
        "DML message should be non-null: {dml}"
    );
}

#[test]
fn integration_45_g13_title_template_omit() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    let create = server.graphql(
        r#"mutation { executeSql(sql: "CREATE TABLE g13link (title TEXT, url VARCHAR(255))") { message } }"#,
    );
    assert!(
        create.get("errors").is_none(),
        "CREATE g13link failed: {create}"
    );

    let template = server.graphql(
        r#"mutation { executeSql(sql: "ALTER TABLE g13link SET TITLE TEMPLATE 'link-{url}'") { message } }"#,
    );
    assert!(
        template.get("errors").is_none(),
        "SET TITLE TEMPLATE failed: {template}"
    );

    let created = server.graphql(
        r#"mutation { createDoogat(input: {type: "g13link", fields: "{\"url\":\"https://example.com\"}"}) { id title } }"#,
    );
    assert!(
        created.get("errors").is_none(),
        "createDoogat with template should succeed: {created}"
    );
    assert_eq!(
        created["data"]["createDoogat"]["title"].as_str(),
        Some("link-https://example.com"),
        "title should be rendered from the template: {created}"
    );

    let create_plain = server.graphql(
        r#"mutation { executeSql(sql: "CREATE TABLE g13plain (title TEXT, url VARCHAR(255))") { message } }"#,
    );
    assert!(
        create_plain.get("errors").is_none(),
        "CREATE g13plain failed: {create_plain}"
    );

    let created_plain = server.graphql(
        r#"mutation { createDoogat(input: {type: "g13plain", fields: "{\"url\":\"https://x\"}"}) { id } }"#,
    );
    assert!(
        created_plain.get("errors").is_some(),
        "createDoogat without title or template should fail: {created_plain}"
    );
    assert!(
        created_plain["errors"][0]["message"]
            .as_str()
            .unwrap()
            .contains("NOT NULL constraint violated: g13plain.title"),
        "error message mismatch: {created_plain}"
    );
}

#[test]
fn integration_45_g12_intra_batch_ignore_surviving_id() {
    let repo = setup_g12item_with_unique_code();
    let server = ServerGuard::start(&repo);

    let result = server.graphql(
        r#"mutation {
            createMany(inputs: [
                {title: "A", type: "g12item", fields: "{\"code\":\"K1\",\"label\":\"first\"}"},
                {title: "A Dup", type: "g12item", fields: "{\"code\":\"K1\",\"label\":\"second\"}"}
            ], onConflict: IGNORE) { id title }
        }"#,
    );
    assert!(
        result.get("errors").is_none(),
        "createMany with an intra-batch duplicate should not error: {result}"
    );

    let results = result["data"]["createMany"].as_array().unwrap();
    assert_eq!(results.len(), 2, "should return 2 results: {result}");
    assert_eq!(
        results[0]["id"].as_str().unwrap(),
        results[1]["id"].as_str().unwrap(),
        "both entries should return the same surviving id: {result}"
    );
    assert_eq!(
        results[1]["title"].as_str().unwrap(),
        "A",
        "surviving row's title should be the first-inserted one: {result}"
    );

    let count = select_scalar(&server, "SELECT COUNT(*) FROM g12item");
    assert_eq!(count, "1", "exactly one row should exist");
}
