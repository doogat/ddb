use ddb_server::actor::ActorHandle;
use ddb_server::events::EventBus;
use ddb_server::read_pool::ReadPool;

async fn setup(dir: &std::path::Path) -> (ActorHandle, ReadPool) {
    ddb_core::service::DoogatService::init(dir).expect("init repo");
    let event_bus = EventBus::new();
    let actor = ActorHandle::spawn(
        dir.to_path_buf(),
        event_bus,
        ddb_core::git_ops::DEFAULT_REMOTE_TIMEOUT,
    )
    .expect("spawn actor");
    let pool = ReadPool::new(dir.to_path_buf(), 1).expect("read pool");
    (actor, pool)
}

/// Run `query` against a freshly built schema and return the JSON data,
/// failing on any GraphQL error.
async fn run_query(query: &str) -> serde_json::Value {
    let tmp = tempfile::tempdir().unwrap();
    let (actor, pool) = setup(tmp.path()).await;
    let schema =
        ddb_server::schema::build_schema(actor, pool, vec![], None).expect("schema must build");
    let response = schema.execute(query).await;
    assert!(
        response.errors.is_empty(),
        "expected no GraphQL errors, got: {:?}",
        response.errors
    );
    response.data.clone().into_json().unwrap()
}

#[tokio::test]
async fn capabilities_query_returns_server_version_and_flags_in_order() {
    let data = run_query("{ capabilities { version flags } }").await;
    let caps = &data["capabilities"];

    assert_eq!(
        caps["version"].as_str(),
        Some(ddb_server::capabilities::version()),
        "version must equal capabilities::version(); got: {caps}"
    );
    let flags: Vec<String> = caps["flags"]
        .as_array()
        .unwrap_or_else(|| panic!("flags must be a JSON array; got: {caps}"))
        .iter()
        .map(|v| {
            v.as_str()
                .unwrap_or_else(|| panic!("each flag must be a string; got: {v}"))
                .to_string()
        })
        .collect();
    assert_eq!(
        flags,
        ddb_server::capabilities::capabilities(),
        "flags must equal capabilities::capabilities() in the same order"
    );
    assert!(flags.len() > 1, "expected several flags; got: {flags:?}");
}

#[tokio::test]
async fn capabilities_flags_are_flat_strings_not_boolean_object() {
    let data = run_query("{ capabilities { flags } }").await;
    let flags = &data["capabilities"]["flags"];
    assert!(
        !flags.is_object(),
        "flags must be a list of names, not an object of booleans; got: {flags}"
    );
    let arr = flags.as_array().expect("flags must be an array");
    assert!(arr.iter().all(|v| v.is_string()), "got: {flags}");
    let names: Vec<&str> = arr.iter().filter_map(|v| v.as_str()).collect();
    // Exact-name membership: near-miss names sharing a prefix must not appear.
    assert!(names.contains(&"schema_apply"), "got: {names:?}");
    assert!(!names.contains(&"schema"), "got: {names:?}");
    assert!(!names.contains(&"schema_apply_v2"), "got: {names:?}");
}

#[tokio::test]
async fn capabilities_query_rejects_unknown_subfield() {
    let tmp = tempfile::tempdir().unwrap();
    let (actor, pool) = setup(tmp.path()).await;
    let schema =
        ddb_server::schema::build_schema(actor, pool, vec![], None).expect("schema must build");
    let response = schema.execute("{ capabilities { version enabled } }").await;
    assert!(
        !response.errors.is_empty(),
        "querying a non-existent `enabled` subfield must surface a GraphQL error"
    );
}
