//! Structured `fields` input across GraphQL, REST, and FFI (PRD 00178).
//!
//! `fields` is a native JSON object on every transport. A boolean stays a
//! boolean and a number stays a number. A top-level `null` omits the key on
//! create and unsets it on update, and no literal `null` text reaches the
//! document. The old JSON-encoded string shape is rejected, never reparsed.
//!
//! FFI has no encoded-string or `null` form to reject: `FieldValue` is a typed
//! enum, and an update unsets a key through `unset_fields`. So the FFI legs
//! cover the native-type happy path and the unset path only.

use crate::common::{DdbTestRepo, ServerGuard};
use ddb_core::ffi::{DoogatDriver, FieldEntryValue, FieldValue, UpdateStructuredInput};
use ddb_core::types::ConflictAction;
use serde_json::{json, Value};

const CREATE_TASK_TABLE: &str = "CREATE TABLE sfi_task (done BOOLEAN, priority INTEGER, note VARCHAR(100))";

/// A fresh repo carrying the `sfi_task` typedef.
fn repo_with_task_type() -> DdbTestRepo {
    let repo = DdbTestRepo::init();
    repo.ddb()
        .args(["query", CREATE_TASK_TABLE])
        .assert()
        .success();
    repo
}

/// Raw Markdown of doogat `id`, straight off the working tree.
fn doogat_file(repo: &DdbTestRepo, id: &str) -> String {
    let path = repo.path().join("ddb").join(format!("{id}.md"));
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("doogat file {} must be readable: {e}", path.display()))
}

/// The frontmatter lines of a doogat file, trimmed.
fn frontmatter_lines(doc: &str) -> Vec<String> {
    doc.split("---")
        .nth(1)
        .unwrap_or_else(|| panic!("doogat must carry frontmatter, got: {doc}"))
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect()
}

/// `done: true` and `priority: 3` as native YAML scalars, not quoted strings.
fn assert_native_types(doc: &str, label: &str) {
    let lines = frontmatter_lines(doc);
    assert!(
        lines.iter().any(|l| l == "done: true"),
        "{label}: done must be stored as a native boolean, got: {doc}"
    );
    // Core numbers are f64, so an integral value serializes as `3.0`; what
    // matters is that it is an unquoted YAML number, not the string "3".
    assert!(
        lines.iter().any(|l| l == "priority: 3" || l == "priority: 3.0"),
        "{label}: priority must be stored as a native number, got: {doc}"
    );
}

/// No `note` key and no literal `null` text anywhere in the frontmatter.
fn assert_note_absent(doc: &str, label: &str) {
    let lines = frontmatter_lines(doc);
    assert!(
        !lines.iter().any(|l| l.starts_with("note:")),
        "{label}: note must be absent from the document, got: {doc}"
    );
    assert!(
        !lines.iter().any(|l| l.contains("null")),
        "{label}: no literal null text may be written, got: {doc}"
    );
}

/// The seed's `note: draft` is stored, so a later unset has something to remove.
fn assert_note_present(doc: &str) {
    assert!(
        frontmatter_lines(doc)
            .iter()
            .any(|l| l.starts_with("note:") && l.contains("draft")),
        "seed must store note, got: {doc}"
    );
}

fn gql_id(response: &Value, mutation: &str) -> String {
    assert!(
        response.get("errors").is_none(),
        "GraphQL {mutation} must succeed: {response}"
    );
    response["data"][mutation]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("GraphQL {mutation} must return an id: {response}"))
        .to_string()
}

fn rest_create_id(server: &ServerGuard, body: Value) -> String {
    let resp = server.rest_post("/doogats", body);
    assert_eq!(resp.status(), 201, "REST create must return 201 Created");
    let body: Value = resp.json().expect("REST create must return JSON");
    body["data"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("REST create must return an id: {body}"))
        .to_string()
}

fn ffi_driver(repo: &DdbTestRepo) -> DoogatDriver {
    DoogatDriver::new(repo.path().display().to_string()).expect("FFI driver must open the repo")
}

fn ffi_entry(key: &str, value: FieldValue) -> FieldEntryValue {
    FieldEntryValue {
        key: key.to_string(),
        value,
    }
}

fn ffi_create(repo: &DdbTestRepo, fields: Vec<FieldEntryValue>) -> String {
    ffi_driver(repo)
        .create_structured(
            Some("FFI task".to_string()),
            vec![],
            Some("sfi_task".to_string()),
            None,
            fields,
            ConflictAction::Error.into(),
        )
        .expect("FFI create_structured must succeed")
        .id
}

// ── Happy path: native boolean and number ────────────────────────────

#[test]
fn graphql_create_stores_native_boolean_and_number() {
    let repo = repo_with_task_type();
    let server = ServerGuard::start(&repo);
    let response = server.graphql_with_vars(
        r#"mutation($input: CreateDoogatInput!) { createDoogat(input: $input) { id } }"#,
        json!({ "input": { "title": "GQL task", "type": "sfi_task",
                           "fields": { "done": true, "priority": 3 } } }),
    );
    let id = gql_id(&response, "createDoogat");
    assert_native_types(&doogat_file(&repo, &id), "GraphQL");
}

#[test]
fn rest_create_stores_native_boolean_and_number() {
    let repo = repo_with_task_type();
    let server = ServerGuard::start(&repo);
    let id = rest_create_id(
        &server,
        json!({ "title": "REST task", "type": "sfi_task",
                "fields": { "done": true, "priority": 3 } }),
    );
    assert_native_types(&doogat_file(&repo, &id), "REST");
}

#[test]
fn ffi_create_stores_native_boolean_and_number() {
    let repo = repo_with_task_type();
    let id = ffi_create(
        &repo,
        vec![
            ffi_entry("done", FieldValue::Bool(true)),
            ffi_entry("priority", FieldValue::Num(3.0)),
        ],
    );
    assert_native_types(&doogat_file(&repo, &id), "FFI");
}

// ── Edge case: null omits on create, unsets on update ────────────────

#[test]
fn graphql_null_field_omits_on_create_and_unsets_on_update() {
    let repo = repo_with_task_type();
    let server = ServerGuard::start(&repo);

    let created = server.graphql_with_vars(
        r#"mutation($input: CreateDoogatInput!) { createDoogat(input: $input) { id } }"#,
        json!({ "input": { "title": "GQL omit", "type": "sfi_task",
                           "fields": { "done": false, "note": null } } }),
    );
    let omitted_id = gql_id(&created, "createDoogat");
    assert_note_absent(&doogat_file(&repo, &omitted_id), "GraphQL create");

    let seeded = server.graphql(
        r#"mutation { createDoogat(input: { title: "GQL seed", type: "sfi_task", fields: { note: "draft" } }) { id } }"#,
    );
    let id = gql_id(&seeded, "createDoogat");
    assert_note_present(&doogat_file(&repo, &id));

    let updated = server.graphql_with_vars(
        r#"mutation($input: UpdateDoogatInput!) { updateDoogat(input: $input) { id } }"#,
        json!({ "input": { "id": id, "fields": { "note": null } } }),
    );
    gql_id(&updated, "updateDoogat");
    assert_note_absent(&doogat_file(&repo, &id), "GraphQL update");
}

#[test]
fn rest_null_field_omits_on_create_and_unsets_on_update() {
    let repo = repo_with_task_type();
    let server = ServerGuard::start(&repo);

    let omitted_id = rest_create_id(
        &server,
        json!({ "title": "REST omit", "type": "sfi_task",
                "fields": { "done": false, "note": null } }),
    );
    assert_note_absent(&doogat_file(&repo, &omitted_id), "REST create");

    let id = rest_create_id(
        &server,
        json!({ "title": "REST seed", "type": "sfi_task", "fields": { "note": "draft" } }),
    );
    assert_note_present(&doogat_file(&repo, &id));

    let resp = server.rest_put(&format!("/doogats/{id}"), json!({ "fields": { "note": null } }));
    assert_eq!(resp.status(), 200, "REST update with a null field must succeed");
    assert_note_absent(&doogat_file(&repo, &id), "REST update");
}

#[test]
fn ffi_unset_field_removes_key_on_update() {
    let repo = repo_with_task_type();
    let id = ffi_create(&repo, vec![ffi_entry("note", FieldValue::Str("draft".to_string()))]);
    assert_note_present(&doogat_file(&repo, &id));

    ffi_driver(&repo)
        .update_structured(
            id.clone(),
            UpdateStructuredInput {
                title: None,
                tags: None,
                doogat_type: None,
                body: None,
                fields: vec![],
                unset_fields: vec!["note".to_string()],
            },
        )
        .expect("FFI update_structured must succeed");
    assert_note_absent(&doogat_file(&repo, &id), "FFI update");
}

// ── Error case: the old encoded-string shape is rejected ─────────────

#[test]
fn rest_rejects_encoded_string_fields_as_invalid_input() {
    let repo = repo_with_task_type();
    let server = ServerGuard::start(&repo);
    let resp = server.rest_post(
        "/doogats",
        json!({ "title": "REST legacy", "type": "sfi_task",
                "fields": "{\"done\":true,\"priority\":3}" }),
    );
    assert_eq!(resp.status(), 400, "encoded-string fields must return 400");
    let body: Value = resp.json().expect("REST error must return JSON");
    assert_eq!(
        body["error"], "VALIDATION_ERROR",
        "encoded-string fields must map to the invalid-input code, got: {body}"
    );

    let list: Value = server
        .rest_get("/doogats?type=sfi_task")
        .json()
        .expect("REST list must return JSON");
    assert_eq!(
        list["data"].as_array().map(Vec::len),
        Some(0),
        "a rejected create must write nothing, got: {list}"
    );
}

/// A legacy client keeps `$f: String` and sends the encoded string. The
/// operation must fail with the migration hint and write nothing.
/// (The dynamic schema accepts a `String` variable in a `JSON` position, so
/// the rejection comes from the fields decoder, not from variable validation.)
#[test]
fn graphql_string_variable_for_fields_is_rejected() {
    let repo = repo_with_task_type();
    let server = ServerGuard::start(&repo);
    let response = server.graphql_with_vars(
        r#"mutation($f: String) { createDoogat(input: { title: "GQL legacy var", type: "sfi_task", fields: $f }) { id } }"#,
        json!({ "f": "{\"done\":true}" }),
    );
    assert!(
        response["data"].is_null(),
        "a String-typed fields variable must not execute: {response}"
    );
    let message = response["errors"][0]["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("migrate the GraphQL variable type to JSON"),
        "a String-typed fields variable must be rejected with the migration hint, got: {response}"
    );
    let count = server.graphql(r#"{ search(query: "legacy") { totalCount } }"#);
    assert_eq!(
        count["data"]["search"]["totalCount"], 0,
        "a rejected create must write nothing, got: {count}"
    );
}

#[test]
fn graphql_encoded_string_literal_for_fields_is_rejected() {
    let repo = repo_with_task_type();
    let server = ServerGuard::start(&repo);
    let response = server.graphql(
        r#"mutation { createDoogat(input: { title: "GQL legacy", type: "sfi_task", fields: "{\"done\":true}" }) { id } }"#,
    );
    let message = response["errors"][0]["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("migrate the GraphQL variable type to JSON"),
        "an encoded-string literal must be rejected with the migration hint, got: {response}"
    );
    let count = server.graphql(r#"{ search(query: "GQL legacy") { totalCount } }"#);
    assert_eq!(
        count["data"]["search"]["totalCount"], 0,
        "a rejected create must write nothing, got: {count}"
    );
}
