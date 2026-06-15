//! Tier 1 service-level tests for the `DoogatService::update` app-contract
//! facade and `UpdateCommand.unset_fields` (PRD 00149). These live in the lib
//! target so `cargo test-ci` (`--lib --bins`) exercises them.

use crate::app_contract::UpdateCommand;
use crate::service::DoogatService;
use crate::types::Value;
use std::collections::BTreeMap;

fn init_service(dir: &std::path::Path) -> DoogatService {
    DoogatService::init(dir).expect("init repo")
}

#[test]
fn update_facade_returns_empty_warnings_and_new_title() {
    let tmp = tempfile::tempdir().unwrap();
    let svc = init_service(tmp.path());

    let id = svc
        .create_doogat("Original Title", &[], None, "")
        .expect("create doogat");

    let output = svc
        .update(UpdateCommand {
            id: id.clone(),
            title: Some("Updated Title".into()),
            tags: None,
            doogat_type: None,
            body: None,
            fields: BTreeMap::new(),
            unset_fields: vec![],
        })
        .expect("update must return Ok");

    assert!(
        output.warnings.is_empty(),
        "update facade must emit no warnings, got: {:?}",
        output.warnings.iter().map(|w| w.code).collect::<Vec<_>>()
    );
    assert_eq!(
        output.value.meta.title.as_deref(),
        Some("Updated Title"),
        "returned value must reflect the new title"
    );
}

#[test]
fn update_facade_unset_fields_clears_frontmatter_field() {
    let tmp = tempfile::tempdir().unwrap();
    let svc = init_service(tmp.path());

    let id = svc
        .create_doogat("Base Doogat", &[], None, "")
        .expect("create doogat");

    // First update: set a custom frontmatter field.
    let mut fields = BTreeMap::new();
    fields.insert("custom_key".to_string(), Value::String("custom_value".into()));

    let after_set = svc
        .update(UpdateCommand {
            id: id.clone(),
            title: None,
            tags: None,
            doogat_type: None,
            body: None,
            fields,
            unset_fields: vec![],
        })
        .expect("first update (set field) must return Ok");

    assert!(
        after_set.value.meta.extra.contains_key("custom_key"),
        "after setting the field, meta.extra must contain 'custom_key'"
    );

    // Second update: unset the same field.
    let after_unset = svc
        .update(UpdateCommand {
            id: id.clone(),
            title: None,
            tags: None,
            doogat_type: None,
            body: None,
            fields: BTreeMap::new(),
            unset_fields: vec!["custom_key".to_string()],
        })
        .expect("second update (unset field) must return Ok");

    assert!(
        !after_unset.value.meta.extra.contains_key("custom_key"),
        "after unsetting the field, meta.extra must NOT contain 'custom_key'"
    );
}
