//! Tier 1 service-level regression tests for the unregistered-type create
//! policy (PRD 00155). These live in the lib target so `cargo test-ci`
//! (`--lib --bins`) exercises them: the regression this PRD fixes escaped
//! Tier 1 for ~12 days because its only guard ran in Tier 2 (CI-only).
//! Keeping the policy-branch guard here keeps the fast local gate honest.

use crate::app_contract::{CreateCommand, UnregisteredTypePolicy};
use crate::error::{codes, DoogatError};
use crate::service::DoogatService;
use crate::types::{ConflictAction, Value};
use std::collections::BTreeMap;

fn init_service(dir: &std::path::Path) -> DoogatService {
    DoogatService::init(dir).expect("init repo")
}

#[test]
fn baseonly_unregistered_type_creates_base_doogat_with_warning() {
    let tmp = tempfile::tempdir().unwrap();
    let svc = init_service(tmp.path());

    let result = svc.create(CreateCommand {
        title: Some("Project Alpha".into()),
        tags: vec!["active".into()],
        doogat_type: Some("project".into()),
        body: Some("A project doogat".into()),
        fields: BTreeMap::new(),
        on_conflict: ConflictAction::Error,
        unregistered_type_policy: UnregisteredTypePolicy::BaseOnly,
    });

    let output = result.expect("expected Ok, got Err");
    assert_eq!(
        output.warnings.len(),
        1,
        "expected exactly one warning, got: {:?}",
        output.warnings.iter().map(|w| w.code).collect::<Vec<_>>()
    );
    let warning = output
        .warnings
        .iter()
        .find(|w| w.code == "UNREGISTERED_TYPE_BASE_ONLY")
        .unwrap_or_else(|| {
            panic!(
                "expected UNREGISTERED_TYPE_BASE_ONLY warning, got: {:?}",
                output.warnings.iter().map(|w| w.code).collect::<Vec<_>>()
            )
        });
    assert!(
        !warning.message.is_empty(),
        "UNREGISTERED_TYPE_BASE_ONLY warning must have a non-empty message"
    );
    assert_eq!(
        output.value.meta.doogat_type.as_deref(),
        Some("project"),
        "created doogat must retain its doogat_type"
    );
    assert_eq!(
        output.value.meta.title.as_deref(),
        Some("Project Alpha"),
        "created doogat must retain its title"
    );
}

#[test]
fn strict_unregistered_type_returns_type_not_registered() {
    let tmp = tempfile::tempdir().unwrap();
    let svc = init_service(tmp.path());

    let result = svc.create(CreateCommand {
        title: Some("Project Alpha".into()),
        tags: vec!["active".into()],
        doogat_type: Some("project".into()),
        body: Some("A project doogat".into()),
        fields: BTreeMap::new(),
        on_conflict: ConflictAction::Error,
        unregistered_type_policy: UnregisteredTypePolicy::Strict,
    });

    match result {
        Err(DoogatError::Structured { code, .. }) if code == codes::TYPE_NOT_REGISTERED => {}
        Err(other) => panic!(
            "expected DoogatError::Structured {{ code: TYPE_NOT_REGISTERED }}, got: {:?}",
            other
        ),
        Ok(output) => panic!(
            "expected Err(TYPE_NOT_REGISTERED), got Ok with warnings: {:?}",
            output.warnings.iter().map(|w| w.code).collect::<Vec<_>>()
        ),
    }
}

#[test]
fn baseonly_registered_type_uses_typed_pipeline() {
    let tmp = tempfile::tempdir().unwrap();
    let mut svc = init_service(tmp.path());
    svc.execute_sql("CREATE TABLE project (name TEXT)")
        .expect("create table");

    let mut fields = BTreeMap::new();
    fields.insert("name".to_string(), Value::String("Alpha".to_string()));

    let output = svc
        .create(CreateCommand {
            title: Some("Project Alpha".into()),
            tags: vec![],
            doogat_type: Some("project".into()),
            body: None,
            fields,
            on_conflict: ConflictAction::Error,
            unregistered_type_policy: UnregisteredTypePolicy::BaseOnly,
        })
        .expect("expected Ok for registered type");

    assert!(
        output
            .warnings
            .iter()
            .all(|w| w.code != "UNREGISTERED_TYPE_BASE_ONLY"),
        "registered type must NOT trigger UNREGISTERED_TYPE_BASE_ONLY warning, got: {:?}",
        output.warnings.iter().map(|w| w.code).collect::<Vec<_>>()
    );
}

#[test]
fn baseonly_typedef_type_creates_base_doogat_with_warning() {
    let tmp = tempfile::tempdir().unwrap();
    let svc = init_service(tmp.path());

    let output = svc
        .create(CreateCommand {
            title: Some("My Type".into()),
            tags: vec![],
            doogat_type: Some("_typedef".into()),
            body: None,
            fields: BTreeMap::new(),
            on_conflict: ConflictAction::Error,
            unregistered_type_policy: UnregisteredTypePolicy::BaseOnly,
        })
        .expect("expected Ok for _typedef with BaseOnly policy");

    assert!(
        output
            .warnings
            .iter()
            .any(|w| w.code == "UNREGISTERED_TYPE_BASE_ONLY"),
        "expected UNREGISTERED_TYPE_BASE_ONLY warning for unregistered _typedef type, got: {:?}",
        output.warnings.iter().map(|w| w.code).collect::<Vec<_>>()
    );
}
