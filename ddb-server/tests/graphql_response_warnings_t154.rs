//! PRD 00154 — graphql-response-extension-warnings-v1
//!
//! Both tests are expected to be RED until the WarningExtension is wired
//! into the schema (T3). The contract: every GraphQL response carries
//! `extensions.warnings` as a JSON array (empty when no warnings). Each
//! entry has `code` (string) and `message` (string).

use ddb_server::actor::ActorHandle;
use ddb_server::events::EventBus;
use ddb_server::read_pool::ReadPool;

async fn setup(dir: &std::path::Path) -> (ActorHandle, ReadPool) {
    ddb_core::service::DoogatService::init(dir).expect("init repo");
    let event_bus = EventBus::new();
    let actor = ActorHandle::spawn(dir.to_path_buf(), event_bus).expect("spawn actor");
    let pool = ReadPool::new(dir.to_path_buf(), 1).expect("read pool");
    (actor, pool)
}

/// PRD 00154 T1: createDoogat with a typedef that has title_template and no
/// caller-supplied title must surface a TITLE_FROM_TEMPLATE warning under
/// `extensions.warnings`.
#[tokio::test]
async fn graphql_create_doogat_surfaces_title_from_template_warning() {
    let tmp = tempfile::tempdir().unwrap();
    let (actor, pool) = setup(tmp.path()).await;

    // Register bookmark typedef with title_template so the service emits
    // TITLE_FROM_TEMPLATE when title is omitted.
    actor
        .execute_sql("CREATE TABLE bookmark (url TEXT NOT NULL)".to_string())
        .await
        .unwrap();
    actor
        .execute_sql("ALTER TABLE bookmark SET TITLE TEMPLATE '{url}'".to_string())
        .await
        .unwrap();

    let schema =
        ddb_server::schema::build_schema(actor, pool, vec![], None).expect("schema must build");

    // Omit `title` to trigger TITLE_FROM_TEMPLATE; `fields` carries url.
    let query = r#"
        mutation {
          createDoogat(input: {
            type: "bookmark",
            fields: "{\"url\": \"https://example.com\"}"
          }) { id }
        }
    "#;

    let response = schema.execute(query).await;

    // The mutation itself must succeed — no GraphQL errors.
    assert!(
        response.errors.is_empty(),
        "expected no GraphQL errors, got: {:?}",
        response.errors
    );

    // `data.createDoogat.id` must be non-empty.
    let data = response.data.clone().into_json().unwrap();
    let id = data["createDoogat"]["id"].as_str().unwrap_or("");
    assert!(
        !id.is_empty(),
        "expected a non-empty doogat id in data.createDoogat.id, got: {data}"
    );

    // extensions.warnings must be present.
    let warnings_value = response.extensions.get("warnings").unwrap_or_else(|| {
        panic!(
            "extensions.warnings must be present; got extensions: {:?}",
            response.extensions
        )
    });

    // Convert to serde_json for ergonomic assertions.
    let warnings_json = warnings_value.clone().into_json().unwrap();
    let warnings = warnings_json.as_array().unwrap_or_else(|| {
        panic!("extensions.warnings must be a JSON array; got: {warnings_json}")
    });

    assert_eq!(
        warnings.len(),
        1,
        "expected exactly one warning entry, got {}: {warnings:?}",
        warnings.len()
    );

    let entry = &warnings[0];
    assert_eq!(
        entry.get("code").and_then(|v| v.as_str()),
        Some("TITLE_FROM_TEMPLATE"),
        "warning entry must have code == 'TITLE_FROM_TEMPLATE'; got: {entry}"
    );

    let message = entry.get("message").and_then(|v| v.as_str()).unwrap_or("");
    assert!(
        !message.is_empty(),
        "warning entry must have a non-empty message; got: {entry}"
    );
}

/// PRD 00154 T1: createDoogat with an explicit title and no typedef
/// title_template must still carry `extensions.warnings` as an empty array.
#[tokio::test]
async fn graphql_create_doogat_emits_empty_warnings_extension_when_none() {
    let tmp = tempfile::tempdir().unwrap();
    let (actor, pool) = setup(tmp.path()).await;

    // No typedef with title_template — no warnings should be collected.
    let schema =
        ddb_server::schema::build_schema(actor, pool, vec![], None).expect("schema must build");

    let query = r#"
        mutation {
          createDoogat(input: {
            title: "Explicit Title"
          }) { id }
        }
    "#;

    let response = schema.execute(query).await;

    assert!(
        response.errors.is_empty(),
        "expected no GraphQL errors, got: {:?}",
        response.errors
    );

    let data = response.data.clone().into_json().unwrap();
    let id = data["createDoogat"]["id"].as_str().unwrap_or("");
    assert!(
        !id.is_empty(),
        "expected a non-empty doogat id in data.createDoogat.id, got: {data}"
    );

    // extensions.warnings must be present even when empty.
    let warnings_value = response.extensions.get("warnings").unwrap_or_else(|| {
        panic!(
            "extensions.warnings must always be present; got extensions: {:?}",
            response.extensions
        )
    });

    let warnings_json = warnings_value.clone().into_json().unwrap();
    assert_eq!(
        warnings_json,
        serde_json::Value::Array(vec![]),
        "extensions.warnings must be an empty array when no warnings collected; got: {warnings_json}"
    );
}
