use ddb_core::app_contract::{CreateCommand, UnregisteredTypePolicy};
use ddb_core::service::DoogatService;
use ddb_core::types::{ConflictAction, Value};
use std::collections::BTreeMap;

fn basic_cmd(title: &str) -> CreateCommand {
    CreateCommand {
        title: Some(title.to_string()),
        tags: vec![],
        doogat_type: None,
        body: None,
        fields: BTreeMap::new(),
        on_conflict: ConflictAction::Error,
        unregistered_type_policy: UnregisteredTypePolicy::Strict,
    }
}

#[test]
fn create_returns_parsed_doogat_with_correct_title() {
    let tmp = tempfile::TempDir::new().unwrap();
    let svc = DoogatService::init(tmp.path()).unwrap();
    let output = svc.create(basic_cmd("Hello World")).unwrap();
    assert_eq!(output.value.meta.title.as_deref(), Some("Hello World"));
}

#[test]
fn create_returns_app_output_with_no_warnings() {
    let tmp = tempfile::TempDir::new().unwrap();
    let svc = DoogatService::init(tmp.path()).unwrap();
    let output = svc.create(basic_cmd("Warn Test")).unwrap();
    assert!(output.warnings.is_empty());
}

#[test]
fn create_returns_parsed_doogat_with_tags() {
    let tmp = tempfile::TempDir::new().unwrap();
    let svc = DoogatService::init(tmp.path()).unwrap();
    let cmd = CreateCommand {
        title: Some("Tagged".to_string()),
        tags: vec!["rust".to_string(), "test".to_string()],
        doogat_type: None,
        body: None,
        fields: BTreeMap::new(),
        on_conflict: ConflictAction::Error,
        unregistered_type_policy: UnregisteredTypePolicy::Strict,
    };
    let output = svc.create(cmd).unwrap();
    assert_eq!(output.value.meta.tags, vec!["rust", "test"]);
}

#[test]
fn create_returns_parsed_doogat_with_body() {
    let tmp = tempfile::TempDir::new().unwrap();
    let svc = DoogatService::init(tmp.path()).unwrap();
    let cmd = CreateCommand {
        title: Some("Body Test".to_string()),
        tags: vec![],
        doogat_type: None,
        body: Some("some body text".to_string()),
        fields: BTreeMap::new(),
        on_conflict: ConflictAction::Error,
        unregistered_type_policy: UnregisteredTypePolicy::Strict,
    };
    let output = svc.create(cmd).unwrap();
    assert_eq!(output.value.body, "some body text");
}

#[test]
fn create_with_doogat_type_succeeds() {
    let tmp = tempfile::TempDir::new().unwrap();
    let mut svc = DoogatService::init(tmp.path()).unwrap();
    svc.execute_sql("CREATE TABLE project (title TEXT)")
        .unwrap();
    let cmd = CreateCommand {
        title: Some("My Project".to_string()),
        tags: vec![],
        doogat_type: Some("project".to_string()),
        body: None,
        fields: BTreeMap::new(),
        on_conflict: ConflictAction::Error,
        unregistered_type_policy: UnregisteredTypePolicy::Strict,
    };
    let output = svc.create(cmd).unwrap();
    assert_eq!(output.value.meta.doogat_type.as_deref(), Some("project"));
}

#[test]
fn create_passes_extra_fields_through() {
    let tmp = tempfile::TempDir::new().unwrap();
    let svc = DoogatService::init(tmp.path()).unwrap();
    let mut fields = BTreeMap::new();
    fields.insert("priority".to_string(), Value::Number(3.0));
    let cmd = CreateCommand {
        title: Some("With Fields".to_string()),
        tags: vec![],
        doogat_type: None,
        body: None,
        fields,
        on_conflict: ConflictAction::Error,
        unregistered_type_policy: UnregisteredTypePolicy::Strict,
    };
    let output = svc.create(cmd).unwrap();
    assert_eq!(
        output.value.meta.extra.get("priority"),
        Some(&Value::Number(3.0))
    );
}
