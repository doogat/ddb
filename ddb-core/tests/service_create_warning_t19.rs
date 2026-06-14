use ddb_core::app_contract::{CreateCommand, UnregisteredTypePolicy};
use ddb_core::service::DoogatService;
use ddb_core::types::{ConflictAction, Value};
use std::collections::BTreeMap;

fn init_service(dir: &std::path::Path) -> DoogatService {
    DoogatService::init(dir).expect("init repo")
}

#[test]
fn service_create_emits_title_from_template_warning_when_title_omitted() {
    let tmp = tempfile::tempdir().unwrap();
    let mut svc = init_service(tmp.path());
    svc.execute_sql("CREATE TABLE bookmark (url TEXT NOT NULL)")
        .expect("create table");
    svc.execute_sql("ALTER TABLE bookmark SET TITLE TEMPLATE '{url}'")
        .expect("set title template");

    let mut fields = BTreeMap::new();
    fields.insert(
        "url".to_string(),
        Value::String("https://example.com".to_string()),
    );
    let output = svc
        .create(CreateCommand {
            title: None,
            body: None,
            tags: vec![],
            doogat_type: Some("bookmark".into()),
            fields,
            on_conflict: ConflictAction::Error,
            unregistered_type_policy: UnregisteredTypePolicy::Strict,
        })
        .expect("create succeeded");

    let warning = output
        .warnings
        .iter()
        .find(|w| w.code == "TITLE_FROM_TEMPLATE")
        .unwrap_or_else(|| {
            panic!(
                "expected TITLE_FROM_TEMPLATE warning, got warnings: {:?}",
                output.warnings.iter().map(|w| w.code).collect::<Vec<_>>()
            )
        });
    assert!(
        !warning.message.is_empty(),
        "TITLE_FROM_TEMPLATE warning must have a non-empty message"
    );

    let title = output.value.meta.title.as_deref().unwrap_or("");
    assert!(
        !title.is_empty(),
        "expected title rendered from title_template, got empty string"
    );
    assert!(
        title.contains("example.com"),
        "expected rendered title to contain 'example.com', got {title:?}"
    );
}

#[test]
fn service_create_emits_no_warning_when_title_provided() {
    let tmp = tempfile::tempdir().unwrap();
    let mut svc = init_service(tmp.path());
    svc.execute_sql("CREATE TABLE bookmark (url TEXT NOT NULL)")
        .expect("create table");
    svc.execute_sql("ALTER TABLE bookmark SET TITLE TEMPLATE '{url}'")
        .expect("set title template");

    let mut fields = BTreeMap::new();
    fields.insert(
        "url".to_string(),
        Value::String("https://example.com".to_string()),
    );
    let output = svc
        .create(CreateCommand {
            title: Some("My Bookmark".into()),
            body: None,
            tags: vec![],
            doogat_type: Some("bookmark".into()),
            fields,
            on_conflict: ConflictAction::Error,
            unregistered_type_policy: UnregisteredTypePolicy::Strict,
        })
        .expect("create succeeded");

    assert!(
        output.warnings.is_empty(),
        "expected no warnings when title is provided, got: {:?}",
        output.warnings.iter().map(|w| w.code).collect::<Vec<_>>()
    );
    assert_eq!(
        output.value.meta.title.as_deref(),
        Some("My Bookmark"),
        "title must match the caller-supplied value"
    );
}

#[test]
fn service_create_emits_no_warning_for_untyped_with_title() {
    let tmp = tempfile::tempdir().unwrap();
    let svc = init_service(tmp.path());

    let output = svc
        .create(CreateCommand {
            title: Some("Plain".into()),
            body: None,
            tags: vec![],
            doogat_type: None,
            fields: BTreeMap::new(),
            on_conflict: ConflictAction::Error,
            unregistered_type_policy: UnregisteredTypePolicy::Strict,
        })
        .expect("create succeeded");

    assert!(
        output.warnings.is_empty(),
        "expected no warnings for untyped create with explicit title, got: {:?}",
        output.warnings.iter().map(|w| w.code).collect::<Vec<_>>()
    );
}
