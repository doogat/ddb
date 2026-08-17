use crate::common::{DdbTestRepo, ServerGuard};
use serde_json::{json, Value};
use std::time::Duration;
use tokio_postgres::SimpleQueryMessage;

fn assert_graphql_ok(result: &Value) {
    assert!(
        result.get("errors").is_none(),
        "GraphQL request failed: {result}"
    );
    assert!(
        result.get("data").is_some(),
        "missing GraphQL data: {result}"
    );
}

fn execute_sql(server: &ServerGuard, sql: &str) -> Value {
    let result = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message affected } }"#,
        json!({ "sql": sql }),
    );
    assert_graphql_ok(&result);
    result
}

fn rest_create(server: &ServerGuard, title: &str) -> String {
    let response = server.rest_post("/doogats", json!({ "title": title, "tags": ["sorttest"] }));
    assert_eq!(response.status(), 201, "REST create failed for {title}");
    let body: Value = response.json().expect("REST create returned invalid JSON");
    body["data"]["id"]
        .as_str()
        .expect("REST create response missing id")
        .to_string()
}

#[test]
fn integration_19_rest_sort_parameter() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    let _charlie_id = rest_create(&server, "Charlie Sort");
    std::thread::sleep(Duration::from_millis(1100));
    let _alpha_id = rest_create(&server, "Alpha Sort");
    std::thread::sleep(Duration::from_millis(1100));
    let bravo_id = rest_create(&server, "Bravo Sort");

    let response = server.rest_get("/doogats?tag=sorttest&sort=title");
    assert_eq!(response.status(), 200);
    let body: Value = response
        .json()
        .expect("ascending sort returned invalid JSON");
    assert_eq!(body["data"][0]["title"], "Alpha Sort");

    let response = server.rest_get("/doogats?tag=sorttest&sort=-title");
    assert_eq!(response.status(), 200);
    let body: Value = response
        .json()
        .expect("descending sort returned invalid JSON");
    assert_eq!(body["data"][0]["title"], "Charlie Sort");

    let response = server.rest_get("/doogats?tag=sorttest&sort=date");
    assert_eq!(response.status(), 200);
    let body: Value = response.json().expect("date sort returned invalid JSON");
    assert_eq!(body["data"][0]["id"], bravo_id);

    let response = server.rest_get("/doogats?sort=invalid");
    assert_eq!(response.status(), 400);
}

#[test]
fn integration_20_pgwire_boolean_type() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    execute_sql(
        &server,
        "CREATE TABLE pgbooltest (label TEXT, active BOOLEAN)",
    );
    execute_sql(
        &server,
        "INSERT INTO pgbooltest (label, active) VALUES ('yes', true)",
    );

    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let (client, connection) = tokio_postgres::Config::new()
            .host("127.0.0.1")
            .port(server.pg_port)
            .user("ddb")
            .password(&server.token)
            .dbname("ddb")
            .connect(tokio_postgres::NoTls)
            .await
            .expect("PgWire connection failed");
        tokio::spawn(async move {
            connection.await.ok();
        });

        let messages = client
            .simple_query("SELECT active FROM pgbooltest WHERE label = 'yes'")
            .await
            .expect("PgWire BOOLEAN query failed");
        let row = messages
            .iter()
            .find_map(|message| match message {
                SimpleQueryMessage::Row(row) => Some(row),
                _ => None,
            })
            .expect("PgWire BOOLEAN query returned no row");
        assert_eq!(row.get(0), Some("t"));
    });
}

#[test]
fn integration_20_nosql_endpoints() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    let created = server.graphql_with_vars(
        r#"mutation($input: CreateDoogatInput!) { createDoogat(input: $input) { id title tags } }"#,
        json!({
            "input": {
                "title": "First note",
                "content": "NoSQL endpoint fixture",
                "tags": ["smoke"]
            }
        }),
    );
    assert_graphql_ok(&created);
    let id = created["data"]["createDoogat"]["id"]
        .as_str()
        .expect("createDoogat missing id");
    let client = reqwest::blocking::Client::new();

    let response = client
        .get(format!("http://127.0.0.1:{}/nosql/{id}", server.port))
        .header("Authorization", format!("Bearer {}", server.token))
        .send()
        .expect("NoSQL get request failed");
    assert_eq!(response.status(), 200);
    let body = response.text().expect("NoSQL get body failed");
    assert!(body.contains("First note"), "NoSQL get response: {body}");

    let response = client
        .get(format!("http://127.0.0.1:{}/nosql?tag=smoke", server.port))
        .header("Authorization", format!("Bearer {}", server.token))
        .send()
        .expect("NoSQL scan request failed");
    assert_eq!(response.status(), 200);
    let body = response.text().expect("NoSQL scan body failed");
    assert!(body.contains(id), "NoSQL tag scan missing {id}: {body}");

    let response = client
        .get(format!(
            "http://127.0.0.1:{}/nosql?type=project&tag=test",
            server.port
        ))
        .header("Authorization", format!("Bearer {}", server.token))
        .send()
        .expect("NoSQL invalid scan request failed");
    assert_eq!(response.status(), 400);

    let response = client
        .get(format!("http://127.0.0.1:{}/nosql/{id}", server.port))
        .send()
        .expect("NoSQL unauthenticated request failed");
    assert_eq!(response.status(), 401);
}

#[test]
fn integration_20_sql_engine_error_descriptive() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    let result = server.graphql(r#"mutation { executeSql(sql: "SELCT * FORM oops") { message } }"#);
    let errors = result["errors"]
        .as_array()
        .unwrap_or_else(|| panic!("malformed SQL should return GraphQL errors: {result}"));
    let message = errors[0]["message"]
        .as_str()
        .expect("GraphQL SQL error missing message");
    assert!(
        message.to_ascii_lowercase().contains("parse:"),
        "SQL error should be descriptive: {result}"
    );
}

#[test]
fn integration_20_compact_custom_backup_path() {
    let repo = DdbTestRepo::init();
    repo.ddb()
        .args(["register-node", "IntegrationNode"])
        .assert()
        .success();
    let backup_path = repo.path().join("gql-backup.bundle.tar");
    let server = ServerGuard::start(&repo);

    let result = server.graphql_with_vars(
        r#"mutation($path: String!) { compact(force: true, backupPath: $path) { gcSuccess backupPath } }"#,
        json!({ "path": backup_path.to_string_lossy() }),
    );
    assert_graphql_ok(&result);
    assert!(result["data"]["compact"]["gcSuccess"].is_boolean());
    assert_eq!(
        result["data"]["compact"]["backupPath"],
        backup_path.to_string_lossy().as_ref()
    );
    assert!(backup_path.is_file(), "custom backup file was not created");
}

#[test]
fn integration_20_maintenance_mutation() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    let result =
        server.graphql(r#"mutation { maintenance { success durationMs fallbackUsed tasksRun } }"#);
    assert_graphql_ok(&result);
    let maintenance = &result["data"]["maintenance"];
    assert!(maintenance["success"].is_boolean(), "{result}");
    assert!(maintenance["durationMs"].is_i64(), "{result}");
    assert!(maintenance["fallbackUsed"].is_boolean(), "{result}");
    assert!(maintenance["tasksRun"].is_array(), "{result}");
}
