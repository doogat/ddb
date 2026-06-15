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

/// PRD 00149 T1: updateDoogat must carry `extensions.warnings` as an always-present
/// empty array when no warnings are collected (mirrors createDoogat behaviour).
#[tokio::test]
async fn graphql_update_doogat_emits_empty_warnings_extension() {
    let tmp = tempfile::tempdir().unwrap();
    let (actor, pool) = setup(tmp.path()).await;
    let schema =
        ddb_server::schema::build_schema(actor, pool, vec![], None).expect("schema must build");

    // First create a doogat so we have a valid id to update.
    let create_query = r#"mutation { createDoogat(input: { title: "Original Title" }) { id } }"#;
    let create_response = schema.execute(create_query).await;
    assert!(
        create_response.errors.is_empty(),
        "expected no GraphQL errors from createDoogat, got: {:?}",
        create_response.errors
    );
    let create_data = create_response.data.clone().into_json().unwrap();
    let id = create_data["createDoogat"]["id"]
        .as_str()
        .expect("createDoogat must return an id");

    // Now update the doogat.
    let update_query = format!(
        r#"mutation {{ updateDoogat(input: {{ id: "{id}", title: "New Title" }}) {{ id }} }}"#
    );
    let response = schema.execute(update_query).await;

    assert!(
        response.errors.is_empty(),
        "expected no GraphQL errors from updateDoogat, got: {:?}",
        response.errors
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

/// PRD 00149 T2: updateDoogat against a non-existent id must return a GraphQL
/// error with `extensions.code == "NOT_FOUND"`.
#[tokio::test]
async fn graphql_update_doogat_not_found_maps_to_not_found_code() {
    let tmp = tempfile::tempdir().unwrap();
    let (actor, pool) = setup(tmp.path()).await;
    let schema =
        ddb_server::schema::build_schema(actor, pool, vec![], None).expect("schema must build");

    // Use a well-formed 14-digit id that was never created.
    let query = r#"mutation { updateDoogat(input: { id: "00000000000000", title: "Ghost" }) { id } }"#;
    let response = schema.execute(query).await;

    assert!(
        !response.errors.is_empty(),
        "expected a GraphQL error for a non-existent id, got none"
    );

    let resp_json = serde_json::to_value(&response).unwrap();
    let code = resp_json["errors"][0]["extensions"]["code"].as_str();
    assert_eq!(
        code,
        Some("NOT_FOUND"),
        "error extensions.code must be NOT_FOUND for a missing id; got: {:?}",
        code
    );
}
