//! REST CRUD/search conformance test (PRD 00143 Phase 0, golden workflow GW-7).
//!
//! Backs the REST base-doogat-CRUD `Guaranteed` label in the downstream
//! capability matrix. Exercises the full REST surface a downstream HTTP client
//! depends on: create, read (including 404), update, delete, and search.
//!
//! Scope: base-doogat CRUD (GW-7) plus REST typed create via the `fields` body
//! member (added by PRD 00149 — see `rest_typed_create_populates_typed_columns`).
//! Structured error codes surface through the shared `error` field (the same
//! SCREAMING_SNAKE_CASE vocabulary GraphQL puts in `extensions.code`).

use crate::common::{DdbTestRepo, ServerGuard};
use serde_json::{json, Value};

/// GW-7 happy path: create → read → update → delete → confirm gone (404).
#[test]
fn rest_crud_lifecycle() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    // Create — POST /rest/doogats returns 201 + the created object under `data`.
    // Base-doogat CRUD (the `Guaranteed` scope) needs no registered type;
    // typed create is `Specialized` and goes through GraphQL (PRD 00147).
    let resp = server.rest_post("/doogats", json!({ "title": "REST Test" }));
    assert_eq!(resp.status(), 201, "create should return 201 Created");
    let body: Value = resp.json().expect("invalid json");
    let id = body["data"]["id"]
        .as_str()
        .expect("created object missing id")
        .to_string();
    assert!(!id.is_empty(), "created id must not be empty");
    assert_eq!(body["data"]["title"], "REST Test");

    // Read — GET /rest/doogats/:id returns 200 + the stored object.
    let resp = server.rest_get(&format!("/doogats/{id}"));
    assert_eq!(resp.status(), 200, "read should return 200");
    let body: Value = resp.json().expect("invalid json");
    assert_eq!(body["data"]["id"], id);
    assert_eq!(body["data"]["title"], "REST Test");

    // Update — PUT /rest/doogats/:id returns 200 + the updated object.
    let resp = server.rest_put(
        &format!("/doogats/{id}"),
        json!({ "title": "REST Updated" }),
    );
    assert_eq!(resp.status(), 200, "update should return 200");
    let body: Value = resp.json().expect("invalid json");
    assert_eq!(body["data"]["title"], "REST Updated");

    // Delete — DELETE /rest/doogats/:id returns 204 No Content.
    let resp = server.rest_delete(&format!("/doogats/{id}"));
    assert_eq!(resp.status(), 204, "delete should return 204 No Content");

    // Read after delete — the doogat is gone, so 404 with the documented body.
    let resp = server.rest_get(&format!("/doogats/{id}"));
    assert_eq!(resp.status(), 404, "read after delete should return 404");
    let body: Value = resp.json().expect("invalid json");
    assert_eq!(body["error"], "NOT_FOUND");
}

/// GW-7 not-found shape: unknown id → 404 + `{ "error": "NOT_FOUND", "message": "..." }`.
/// The `error` field carries a short code string from the shared SCREAMING_SNAKE_CASE
/// vocabulary (`docs/src/technical/rest-api.md` Error Format; same codes GraphQL puts
/// in `extensions.code`). D-REST-1 / footnote †8.
#[test]
fn rest_get_unknown_id_returns_404() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    let resp = server.rest_get("/doogats/20260101120000");
    assert_eq!(resp.status(), 404);
    let body: Value = resp.json().expect("invalid json");
    assert_eq!(body["error"], "NOT_FOUND");
    assert!(
        body["message"]
            .as_str()
            .expect("error body missing message")
            .contains("20260101120000"),
        "message should name the missing id, got: {}",
        body["message"]
    );
}

/// GW-7 search: GET /rest/doogats?q=... returns matching hits under `data`.
#[test]
fn rest_search_returns_matching_results() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    let resp = server.rest_post("/doogats", json!({ "title": "alpha zqxfindme note" }));
    assert_eq!(resp.status(), 201);
    let resp = server.rest_post("/doogats", json!({ "title": "beta unrelated note" }));
    assert_eq!(resp.status(), 201);

    let resp = server.rest_get("/doogats?q=zqxfindme");
    assert_eq!(resp.status(), 200, "search should return 200");
    let body: Value = resp.json().expect("invalid json");
    let hits = body["data"]
        .as_array()
        .expect("search body missing data array");
    assert!(
        hits.iter()
            .any(|h| h["title"].as_str().unwrap_or("").contains("zqxfindme")),
        "search for unique token should surface the matching doogat, got: {body}"
    );
}

/// POST /rest/doogats with a `fields` JSON string populates typed columns on the created doogat.
/// Registers a type via `CREATE TABLE item (category TEXT, priority INTEGER)`, creates a doogat
/// of that type with typed fields, then reads the typed data back via `executeSql` SELECT to
/// confirm the field values round-trip exactly.
#[test]
fn rest_typed_create_populates_typed_columns() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    // Register the type
    let result = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({ "sql": "CREATE TABLE item (category TEXT, priority INTEGER)" }),
    );
    assert!(
        result.get("errors").is_none(),
        "CREATE TABLE item failed: {result}"
    );

    // Create a typed doogat via REST with fields
    let resp = server.rest_post(
        "/doogats",
        json!({
            "title": "REST Typed Item",
            "type": "item",
            "fields": "{\"category\":\"books\",\"priority\":\"1\"}"
        }),
    );
    assert_eq!(
        resp.status(),
        201,
        "typed create should return 201 Created"
    );
    let body: Value = resp.json().expect("invalid json");
    let id = body["data"]["id"]
        .as_str()
        .expect("created object missing id")
        .to_string();
    assert!(!id.is_empty(), "created id must not be empty");
    assert_eq!(body["data"]["title"], "REST Typed Item");
    assert_eq!(
        body["data"]["type"], "item",
        "created doogat must carry the registered type"
    );

    // Verify typed columns are populated by reading the materialized row back.
    assert_item_typed_columns_roundtrip(&server, &id);
}

/// Read the materialized `item` row back via `executeSql` (format:"objects") and
/// assert the typed columns set by the REST typed-create round-trip exactly.
fn assert_item_typed_columns_roundtrip(server: &ServerGuard, id: &str) {
    // A SELECT via executeSql with format:"objects" returns parsed rows, not a
    // `message` (which carries DDL/DML status).
    let select = server.graphql_with_vars(
        r#"mutation($sql: String!, $fmt: String) { executeSql(sql: $sql, format: $fmt) { rows } }"#,
        serde_json::json!({
            "sql": format!("SELECT category, priority FROM item WHERE id = '{id}'"),
            "fmt": "objects"
        }),
    );
    assert!(
        select.get("errors").is_none(),
        "SELECT from item failed: {select}"
    );
    let rows = select["data"]["executeSql"]["rows"]
        .as_array()
        .expect("executeSql missing rows");
    assert_eq!(rows.len(), 1, "expected exactly one materialized row, got: {select}");
    let row: Value = serde_json::from_str(rows[0].as_str().expect("row not a string"))
        .expect("row not valid json");
    assert_eq!(
        row["category"].as_str(),
        Some("books"),
        "category typed column must round-trip exactly, got row: {row}"
    );
    let priority = &row["priority"];
    assert!(
        priority.as_i64() == Some(1) || priority.as_str() == Some("1"),
        "priority typed column must round-trip as 1, got row: {row}"
    );
}

/// POST /rest/doogats with a malformed `fields` value (invalid JSON string) returns 400
/// with `{ "error": "VALIDATION_ERROR", "message": "..." }`.
#[test]
fn rest_create_malformed_fields_returns_400() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    // Register a type so the request can be routed — malformed JSON in fields must be
    // caught before any type lookup would turn it into a different error code.
    let result = server.graphql_with_vars(
        r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
        serde_json::json!({ "sql": "CREATE TABLE item (category TEXT, priority INTEGER)" }),
    );
    assert!(
        result.get("errors").is_none(),
        "CREATE TABLE item failed: {result}"
    );

    let resp = server.rest_post(
        "/doogats",
        json!({
            "title": "Bad Fields Item",
            "type": "item",
            "fields": "{not valid json"
        }),
    );
    assert_eq!(
        resp.status(),
        400,
        "malformed fields must return 400 Bad Request"
    );
    let body: Value = resp.json().expect("invalid json");
    assert_eq!(
        body["error"], "VALIDATION_ERROR",
        "error code must be VALIDATION_ERROR, got: {body}"
    );
    assert!(
        body["message"].as_str().is_some(),
        "error body must include a message, got: {body}"
    );
}

/// POST /rest/doogats without a `fields` member is unchanged: returns 201 with the created
/// object under `data`. Regression guard — typed-create support must not break untyped create.
#[test]
fn rest_untyped_create_without_fields_unchanged() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    let resp = server.rest_post("/doogats", json!({ "title": "Untyped Note" }));
    assert_eq!(
        resp.status(),
        201,
        "untyped create must still return 201 Created"
    );
    let body: Value = resp.json().expect("invalid json");
    let id = body["data"]["id"].as_str().expect("missing id");
    assert!(!id.is_empty(), "created id must not be empty");
    assert_eq!(
        body["data"]["title"], "Untyped Note",
        "title must round-trip"
    );
    // Confirm the doogat is readable
    let resp = server.rest_get(&format!("/doogats/{id}"));
    assert_eq!(resp.status(), 200, "untyped doogat must be readable after create");
    let body: Value = resp.json().expect("invalid json");
    assert_eq!(body["data"]["title"], "Untyped Note");
}

/// PRD 00155: REST `POST /doogats` keeps the `Strict` unregistered-type policy.
/// Unlike the CLI (which restores the lenient base-only create), REST rejects an
/// unregistered `type` with `TYPE_NOT_REGISTERED` (422 Unprocessable Entity).
/// REST typed/unregistered create is `Specialized` (D-REST-1); strict is the
/// ratified Phase 0 decision. The server actor builds the create command with
/// `UnregisteredTypePolicy::Strict`, so this pins that the policy split holds.
#[test]
fn rest_create_unregistered_type_returns_type_not_registered() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    // No `ddb type install project` on this fresh repo — `project` is unregistered.
    let resp = server.rest_post(
        "/doogats",
        json!({ "title": "Project Alpha", "type": "project" }),
    );
    assert_eq!(
        resp.status(),
        422,
        "unregistered-type create must reject with 422 Unprocessable Entity (Strict policy)"
    );
    let body: Value = resp.json().expect("invalid json");
    assert_eq!(
        body["error"], "TYPE_NOT_REGISTERED",
        "REST must surface the TYPE_NOT_REGISTERED code (same vocabulary GraphQL puts in extensions.code), got: {body}"
    );
}
