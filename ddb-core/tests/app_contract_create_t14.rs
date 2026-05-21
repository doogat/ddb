// T14: Compile-time proof that CreateCommand has the correct field shape.
//
// These tests will NOT compile until Ivan updates CreateCommand to:
//   - title: Option<String>   (currently String)
//   - body: Option<String>    (currently String)
//   - on_conflict: ConflictAction (currently missing)
//
// The tests are intentionally failing specs — do not implement the fix here.

use ddb_core::app_contract::CreateCommand;
use ddb_core::types::ConflictAction;
use std::collections::BTreeMap;

/// Proves that CreateCommand accepts title: None, body: None, and
/// on_conflict: ConflictAction::Ignore. Fails to compile until the struct
/// is updated.
#[test]
fn create_command_accepts_optional_title_and_body_with_on_conflict() {
    let _cmd = CreateCommand {
        title: None,
        body: None,
        tags: vec![],
        doogat_type: None,
        fields: BTreeMap::new(),
        on_conflict: ConflictAction::Ignore,
    };
    // Compile-time proof only; no runtime assertion needed.
}

/// Proves that CreateCommand accepts title: Some(_) and body: Some(_)
/// alongside on_conflict (regression guard — existing callers must still work).
#[test]
fn create_command_accepts_some_title_and_body_with_on_conflict() {
    let _cmd = CreateCommand {
        title: Some("Hello".to_string()),
        body: Some("world".to_string()),
        tags: vec!["tag".to_string()],
        doogat_type: None,
        fields: BTreeMap::new(),
        on_conflict: ConflictAction::Error,
    };
}
