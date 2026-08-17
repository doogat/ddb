use crate::common::{DdbTestRepo, ServerGuard};

/// Run a `SELECT ...` via `executeSql` and return the first row's first
/// column as a string. `executeSql`'s `rows` field for a SELECT is a list
/// of JSON-stringified row arrays (e.g. `rows: ["[\"0\"]"]`), so this
/// parses the first element as JSON and indexes into it.
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

#[test]
fn integration_43_sql_insert_defaults_date_graphql() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    let create = server.graphql(
        r#"mutation { executeSql(sql: "CREATE TABLE datecheck (name TEXT)") { message } }"#,
    );
    assert!(
        create.get("errors").is_none(),
        "CREATE TABLE failed: {create}"
    );

    let insert = server.graphql(
        r#"mutation { executeSql(sql: "INSERT INTO datecheck (name) VALUES (\"DateTest\")") { message } }"#,
    );
    assert!(insert.get("errors").is_none(), "INSERT failed: {insert}");
    let id = insert["data"]["executeSql"]["message"]
        .as_str()
        .expect("message should hold the new row id")
        .to_string();
    let expected_date = format!("{}-{}-{}", &id[0..4], &id[4..6], &id[6..8]);

    let query = server.graphql("{ datechecks { items { id created_at } } }");
    assert!(query.get("errors").is_none(), "query failed: {query}");
    let items = query["data"]["datechecks"]["items"].as_array().unwrap();
    let row = items
        .iter()
        .find(|i| i["id"].as_str() == Some(id.as_str()))
        .unwrap_or_else(|| panic!("row {id} not found in {query}"));
    assert_eq!(
        row["created_at"].as_str(),
        Some(expected_date.as_str()),
        "created_at should default to the date derived from id: {query}"
    );

    let batch = server.graphql(
        r#"mutation { executeBatch(statements: ["INSERT INTO datecheck (name) VALUES (\"BatchTest\")"]) { message } }"#,
    );
    assert!(
        batch.get("errors").is_none(),
        "executeBatch failed: {batch}"
    );
    let batch_id = batch["data"]["executeBatch"][0]["message"]
        .as_str()
        .expect("message should hold the new row id")
        .to_string();
    let batch_expected_date = format!(
        "{}-{}-{}",
        &batch_id[0..4],
        &batch_id[4..6],
        &batch_id[6..8]
    );

    let doogat = server.graphql(&format!(r#"{{ doogat(id: "{batch_id}") {{ created_at }} }}"#));
    assert!(
        doogat.get("errors").is_none(),
        "doogat query failed: {doogat}"
    );
    assert_eq!(
        doogat["data"]["doogat"]["created_at"].as_str(),
        Some(batch_expected_date.as_str()),
        "created_at should default to the date derived from id: {doogat}"
    );
}

#[test]
fn integration_43_d_sql_constraint_enforcement() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    let setup1 = server.graphql(
        r#"mutation { executeSql(sql: "CREATE TABLE link_d1 (title VARCHAR(255) NOT NULL, url VARCHAR(255) NOT NULL)") { message } }"#,
    );
    assert!(
        setup1.get("errors").is_none(),
        "CREATE link_d1 failed: {setup1}"
    );
    let setup2 = server.graphql(
        r#"mutation { executeSql(sql: "CREATE TABLE numeric_d3 (title VARCHAR(255) NOT NULL, count INTEGER)") { message } }"#,
    );
    assert!(
        setup2.get("errors").is_none(),
        "CREATE numeric_d3 failed: {setup2}"
    );

    // D1: NOT NULL violation
    let d1 = server.graphql(
        r#"mutation { executeSql(sql: "INSERT INTO link_d1 (title, url) VALUES (NULL, \"https://n.com\")") { message } }"#,
    );
    assert!(
        d1.get("errors").is_some(),
        "D1 should reject a NULL title: {d1}"
    );
    assert!(
        d1["errors"][0]["message"]
            .as_str()
            .unwrap()
            .contains("NOT NULL constraint violated: link_d1.title"),
        "D1 error message mismatch: {d1}"
    );
    assert_eq!(select_scalar(&server, "SELECT COUNT(*) FROM link_d1"), "0");

    // D2: value too long for VARCHAR(255)
    let long_title = "x".repeat(300);
    let d2 = server.graphql(&format!(
        r#"mutation {{ executeSql(sql: "INSERT INTO link_d1 (title, url) VALUES (\"{long_title}\", \"https://l.com\")") {{ message }} }}"#
    ));
    assert!(
        d2.get("errors").is_some(),
        "D2 should reject an oversized title: {d2}"
    );
    assert!(
        d2["errors"][0]["message"]
            .as_str()
            .unwrap()
            .contains("value too long for link_d1.title"),
        "D2 error message mismatch: {d2}"
    );
    assert_eq!(select_scalar(&server, "SELECT COUNT(*) FROM link_d1"), "0");

    // D3: type mismatch
    let d3 = server.graphql(
        r#"mutation { executeSql(sql: "INSERT INTO numeric_d3 (title, count) VALUES (\"a\", \"not_a_number\")") { message } }"#,
    );
    assert!(
        d3.get("errors").is_some(),
        "D3 should reject a non-numeric count: {d3}"
    );
    assert!(
        d3["errors"][0]["message"]
            .as_str()
            .unwrap()
            .contains("type mismatch for numeric_d3.count: expected INTEGER"),
        "D3 error message mismatch: {d3}"
    );
    assert_eq!(
        select_scalar(&server, "SELECT COUNT(*) FROM numeric_d3"),
        "0"
    );

    // D4: unknown column on INSERT
    let d4 = server.graphql(
        r#"mutation { executeSql(sql: "INSERT INTO link_d1 (title, url, unknown_col) VALUES (\"t\", \"https://u.com\", \"dropped\")") { message } }"#,
    );
    assert!(
        d4.get("errors").is_some(),
        "D4 should reject an unknown column: {d4}"
    );
    assert!(
        d4["errors"][0]["message"]
            .as_str()
            .unwrap()
            .contains("unknown column: link_d1.unknown_col"),
        "D4 error message mismatch: {d4}"
    );
    assert_eq!(select_scalar(&server, "SELECT COUNT(*) FROM link_d1"), "0");

    // D5: unknown column on UPDATE leaves the row unchanged
    let keep = server.graphql(
        r#"mutation { executeSql(sql: "INSERT INTO link_d1 (title, url) VALUES (\"keep\", \"https://keep.com\")") { message } }"#,
    );
    assert!(
        keep.get("errors").is_none(),
        "D5 setup insert failed: {keep}"
    );
    let keep_id = keep["data"]["executeSql"]["message"]
        .as_str()
        .unwrap()
        .to_string();

    let d5 = server.graphql(&format!(
        r#"mutation {{ executeSql(sql: "UPDATE link_d1 SET unknown_col = 'x' WHERE id = '{keep_id}'") {{ message }} }}"#
    ));
    assert!(
        d5.get("errors").is_some(),
        "D5 should reject an unknown column on UPDATE: {d5}"
    );
    assert!(
        d5["errors"][0]["message"]
            .as_str()
            .unwrap()
            .contains("unknown column: link_d1.unknown_col"),
        "D5 error message mismatch: {d5}"
    );
    assert_eq!(
        select_scalar(
            &server,
            &format!("SELECT title FROM link_d1 WHERE id = '{keep_id}'")
        ),
        "keep",
        "D5: row should be unchanged after a failed UPDATE"
    );

    // D6: title is not silently derived from url/description without a title_template
    let d6_create = server.graphql(
        r#"mutation { executeSql(sql: "CREATE TABLE link_d6 (title VARCHAR(255) NOT NULL, url VARCHAR(255), description TEXT)") { message } }"#,
    );
    assert!(
        d6_create.get("errors").is_none(),
        "CREATE link_d6 failed: {d6_create}"
    );
    let d6 = server.graphql(
        r#"mutation { executeSql(sql: "INSERT INTO link_d6 (url) VALUES (\"https://notitle.com\")") { message } }"#,
    );
    assert!(
        d6.get("errors").is_some(),
        "D6 should reject a missing title with no template: {d6}"
    );
    assert!(
        d6["errors"][0]["message"]
            .as_str()
            .unwrap()
            .contains("NOT NULL constraint violated: link_d6.title"),
        "D6 error message mismatch: {d6}"
    );
    assert_eq!(select_scalar(&server, "SELECT COUNT(*) FROM link_d6"), "0");
}
