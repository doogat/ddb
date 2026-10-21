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
    // Bind to intent: the type is named `Capabilities` and exposes exactly
    // `version` and `flags`, nothing more.
    let data = run_query(r#"{ __type(name: "Capabilities") { fields { name } } }"#).await;
    let mut names: Vec<&str> = data["__type"]["fields"]
        .as_array()
        .unwrap_or_else(|| panic!("type `Capabilities` must exist; got: {data}"))
        .iter()
        .filter_map(|f| f["name"].as_str())
        .collect();
    names.sort_unstable();
    assert_eq!(names, vec!["flags", "version"], "got: {data}");
}

/// Unwrap a `__Type` JSON node, asserting it is NON_NULL, and return its inner type.
fn expect_non_null<'a>(ty: &'a serde_json::Value, what: &str) -> &'a serde_json::Value {
    assert_eq!(ty["kind"].as_str(), Some("NON_NULL"), "{what} must be non-null; got: {ty}");
    &ty["ofType"]
}

#[tokio::test]
async fn capabilities_types_are_non_null_string_and_string_list() {
    let data = run_query(
        r#"{
            query: __type(name: "Query") { fields { name type { ...T } } }
            caps: __type(name: "Capabilities") { fields { name type { ...T } } }
        }
        fragment T on __Type {
            kind name ofType { kind name ofType { kind name ofType { kind name } } }
        }"#,
    )
    .await;
    let field_type = |obj: &str, field: &str| -> serde_json::Value {
        data[obj]["fields"]
            .as_array()
            .unwrap_or_else(|| panic!("{obj} fields missing; got: {data}"))
            .iter()
            .find(|f| f["name"] == field)
            .unwrap_or_else(|| panic!("{obj}.{field} missing; got: {data}"))["type"]
            .clone()
    };

    let cap_field = field_type("query", "capabilities");
    let inner = expect_non_null(&cap_field, "Query.capabilities");
    assert_eq!(inner["kind"], "OBJECT", "got: {cap_field}");
    assert_eq!(inner["name"], "Capabilities", "got: {cap_field}");

    let version = field_type("caps", "version");
    let inner = expect_non_null(&version, "Capabilities.version");
    assert_eq!(inner["name"], "String", "got: {version}");

    let flags = field_type("caps", "flags");
    let list = expect_non_null(&flags, "Capabilities.flags");
    assert_eq!(list["kind"], "LIST", "got: {flags}");
    let item = expect_non_null(&list["ofType"], "Capabilities.flags items");
    assert_eq!(item["name"], "String", "got: {flags}");
}

/// Concatenated source of every `.rs` file under `src/schema/`.
fn schema_sources() -> String {
    fn walk(dir: &std::path::Path, out: &mut String) {
        for entry in std::fs::read_dir(dir).expect("read schema dir") {
            let path = entry.expect("dir entry").path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push_str(&std::fs::read_to_string(&path).expect("read source"));
            }
        }
    }
    let mut out = String::new();
    walk(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/schema"),
        &mut out,
    );
    out
}

#[test]
fn capabilities_resolver_derives_from_capabilities_module() {
    // The version and flag list must come from the single source of truth,
    // not literals that happen to match today's Cargo version and FLAGS.
    let src = schema_sources();
    assert!(
        src.contains("capabilities::version()"),
        "the GraphQL resolver must call capabilities::version()"
    );
    assert!(
        src.contains("capabilities::capabilities()"),
        "the GraphQL resolver must call capabilities::capabilities()"
    );
    let version_literal = format!("\"{}\"", ddb_server::capabilities::version());
    assert!(
        !src.contains(&version_literal),
        "schema code must not hardcode the version literal {version_literal}"
    );
    for flag in ddb_server::capabilities::FLAGS {
        let literal = format!("\"{flag}\"");
        assert!(
            !src.contains(&literal),
            "schema code must not hand-copy flag literal {literal}"
        );
    }
}
