// `SingleResponse` and `WarningJson` are pub for serialization; their fields
// are pub for serde derive. This test binds to the JSON shape that REST
// clients consume — `warnings` must always serialize (no skip_serializing_if)
// and each entry must carry `code` + `message`. The structs are built
// directly with their public fields so no test-only constructors leak into
// the production surface.

use std::collections::BTreeMap;

use ddb_server::rest::{DoogatJson, SingleResponse, WarningJson};

fn empty_doogat() -> DoogatJson {
    DoogatJson {
        id: String::new(),
        title: String::new(),
        body: String::new(),
        tags: vec![],
        doogat_type: None,
        frontmatter: BTreeMap::new(),
        references: BTreeMap::new(),
        reference_section: String::new(),
    }
}

#[test]
fn rest_single_response_serializes_warnings_field_when_empty() {
    let response = SingleResponse {
        data: empty_doogat(),
        warnings: vec![],
    };
    let json = serde_json::to_value(&response).expect("serialization must not fail");
    let warnings = json
        .get("warnings")
        .unwrap_or_else(|| panic!("JSON object must contain a 'warnings' key; got: {json}"));
    assert_eq!(
        warnings,
        &serde_json::Value::Array(vec![]),
        "'warnings' must serialize as an empty array when there are no warnings"
    );
}

#[test]
fn rest_single_response_serializes_warnings_with_code_and_message() {
    let response = SingleResponse {
        data: empty_doogat(),
        warnings: vec![WarningJson {
            code: "TITLE_FROM_TEMPLATE".into(),
            message: "title was rendered from typedef title_template".into(),
        }],
    };
    let json = serde_json::to_value(&response).expect("serialization must not fail");
    let warnings = json
        .get("warnings")
        .and_then(|v| v.as_array())
        .unwrap_or_else(|| panic!("'warnings' must be a JSON array; got: {json}"));

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
    assert_eq!(
        entry.get("message").and_then(|v| v.as_str()),
        Some("title was rendered from typedef title_template"),
        "warning entry must have the expected message; got: {entry}"
    );
}
