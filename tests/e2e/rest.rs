//! REST CRUD/search conformance test (PRD 00143 Phase 0, golden workflow GW-7).
//!
//! Backs the REST base-doogat-CRUD `Guaranteed` label in the downstream
//! capability matrix. Exercises the full REST surface a downstream HTTP client
//! depends on: create, read (including 404), update, delete, and search.
//!
//! Scope matches GW-7: base-doogat CRUD only. Typed create/update and
//! structured error codes are `Specialized` on REST (deferred to PRD 00147);
//! consumers needing them use GraphQL. This test does not assert typed-field
//! behavior on the REST path.

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
    let resp = server.rest_put(&format!("/doogats/{id}"), json!({ "title": "REST Updated" }));
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
    let hits = body["data"].as_array().expect("search body missing data array");
    assert!(
        hits.iter()
            .any(|h| h["title"].as_str().unwrap_or("").contains("zqxfindme")),
        "search for unique token should surface the matching doogat, got: {body}"
    );
}
