//! Cross-transport structured-error conformance (PRD 00143 Phase 2).
//!
//! REST and GraphQL must report the SAME machine-readable error code for the
//! same failure, drawn from the one shared vocabulary (`ddb-server::error::
//! classify`): REST carries it in the body's `error` field, GraphQL in
//! `extensions.code`. These tests pin that parity so a downstream client can
//! branch on a single code set regardless of which interface it speaks.
//!
//! Scope = the CRUD-baseline errors both interfaces label `Guaranteed`
//! (not-found here). Typed-field structured codes (UNIQUE_VIOLATION etc.) are
//! `Specialized` on REST: REST base CRUD has no typed-field input (D-REST-1,
//! deferred to PRD 00147), so they cannot be triggered on the REST path and are
//! out of scope for a REST+GraphQL parity test.

use crate::common::{DdbTestRepo, ServerGuard};
use serde_json::Value;

/// A 14-digit id that was never created, so every lookup misses.
const MISSING_ID: &str = "20260101120000";

/// `(http_status, error_code)` from a REST error response body.
fn rest_status_and_code(resp: reqwest::blocking::Response) -> (u16, String) {
    let status = resp.status().as_u16();
    let body: Value = resp.json().expect("REST error body must be JSON");
    let code = body["error"]
        .as_str()
        .unwrap_or_else(|| panic!("REST error body missing `error` field: {body}"))
        .to_string();
    (status, code)
}

/// The machine-readable code from the first GraphQL error's `extensions.code`.
fn graphql_error_code(result: &Value) -> String {
    result["errors"][0]["extensions"]["code"]
        .as_str()
        .unwrap_or_else(|| panic!("GraphQL response missing errors[0].extensions.code: {result}"))
        .to_string()
}

/// Read path: REST `GET /doogats/:id` and GraphQL `doogat(id:)` on an unknown id
/// both report `NOT_FOUND` from the shared vocabulary.
#[test]
fn read_unknown_id_reports_not_found_on_both_transports() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    let (rest_status, rest_code) =
        rest_status_and_code(server.rest_get(&format!("/doogats/{MISSING_ID}")));
    assert_eq!(rest_status, 404, "REST read of unknown id should be 404");

    let gql_code = graphql_error_code(
        &server.graphql(&format!("{{ doogat(id: \"{MISSING_ID}\") {{ id }} }}")),
    );

    assert_eq!(rest_code, "NOT_FOUND", "REST should report NOT_FOUND");
    assert_eq!(gql_code, "NOT_FOUND", "GraphQL should report NOT_FOUND");
    assert_eq!(
        rest_code, gql_code,
        "REST `error` and GraphQL `extensions.code` must be the same code"
    );
}

/// Mutation path: REST `DELETE /doogats/:id` and GraphQL `deleteDoogat(id:)` on
/// an unknown id both report `NOT_FOUND`, proving parity holds on writes too.
#[test]
fn delete_unknown_id_reports_not_found_on_both_transports() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    let (rest_status, rest_code) =
        rest_status_and_code(server.rest_delete(&format!("/doogats/{MISSING_ID}")));
    assert_eq!(rest_status, 404, "REST delete of unknown id should be 404");

    let gql_code = graphql_error_code(
        &server.graphql(&format!("mutation {{ deleteDoogat(id: \"{MISSING_ID}\") }}")),
    );

    assert_eq!(rest_code, "NOT_FOUND", "REST should report NOT_FOUND");
    assert_eq!(gql_code, "NOT_FOUND", "GraphQL should report NOT_FOUND");
    assert_eq!(
        rest_code, gql_code,
        "REST `error` and GraphQL `extensions.code` must be the same code"
    );
}
