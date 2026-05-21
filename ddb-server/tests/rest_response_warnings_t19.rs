// SingleResponse must be pub for this test; Ivan adds the pub keyword as part of T19.
// The warnings field (Vec<WarningJson> or equivalent) must also be pub and always serialized
// (no skip_serializing_if), so REST clients can rely on its presence. Ivan chooses the exact
// struct/field names; this test only binds to the JSON shape.

use ddb_server::rest::SingleResponse;

#[test]
fn rest_single_response_serializes_warnings_field_when_empty() {
    let response = SingleResponse::new_empty_warnings();
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
    let response = SingleResponse::new_with_warning(
        "TITLE_FROM_TEMPLATE",
        "title was rendered from typedef title_template",
    );
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
