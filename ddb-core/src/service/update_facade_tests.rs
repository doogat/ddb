//! Tier 1 service-level tests for the `DoogatService::update` app-contract
//! facade and `UpdateCommand.unset_fields` (PRD 00149). These live in the lib
//! target so `cargo test-ci` (`--lib --bins`) exercises them.

use crate::app_contract::UpdateCommand;
use crate::error::DoogatError;
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

/// SET-only update helper, used by the data-safety tests below to seed
/// frontmatter fields without repeating the full `UpdateCommand` shape.
fn set_fields(svc: &DoogatService, id: &str, fields: BTreeMap<String, Value>) {
    svc.update(UpdateCommand {
        id: id.to_string(),
        title: None,
        tags: None,
        doogat_type: None,
        body: None,
        fields,
        unset_fields: vec![],
    })
    .expect("set-fields update must return Ok");
}

#[test]
fn update_empty_unset_fields_preserves_existing_frontmatter() {
    // Data safety: an empty `unset_fields` vec must be a no-op. A title-only
    // update must NOT silently clear previously-set custom fields.
    let tmp = tempfile::tempdir().unwrap();
    let svc = init_service(tmp.path());

    let id = svc
        .create_doogat("Base", &[], None, "")
        .expect("create doogat");

    let mut fields = BTreeMap::new();
    fields.insert("alpha".to_string(), Value::String("one".into()));
    fields.insert("beta".to_string(), Value::String("two".into()));
    set_fields(&svc, &id, fields);

    let after = svc
        .update(UpdateCommand {
            id: id.clone(),
            title: Some("Renamed".into()),
            tags: None,
            doogat_type: None,
            body: None,
            fields: BTreeMap::new(),
            unset_fields: vec![],
        })
        .expect("title-only update must return Ok");

    assert_eq!(after.value.meta.title.as_deref(), Some("Renamed"));
    assert!(
        after.value.meta.extra.contains_key("alpha")
            && after.value.meta.extra.contains_key("beta"),
        "empty unset_fields must preserve all existing frontmatter fields, got: {:?}",
        after.value.meta.extra.keys().collect::<Vec<_>>()
    );
}

#[test]
fn update_unset_removes_only_named_key_keeping_siblings() {
    // Unset must remove exactly the named key and leave siblings intact.
    let tmp = tempfile::tempdir().unwrap();
    let svc = init_service(tmp.path());

    let id = svc
        .create_doogat("Base", &[], None, "")
        .expect("create doogat");

    let mut fields = BTreeMap::new();
    fields.insert("keep".to_string(), Value::String("stay".into()));
    fields.insert("drop".to_string(), Value::String("gone".into()));
    set_fields(&svc, &id, fields);

    let after = svc
        .update(UpdateCommand {
            id: id.clone(),
            title: None,
            tags: None,
            doogat_type: None,
            body: None,
            fields: BTreeMap::new(),
            unset_fields: vec!["drop".to_string()],
        })
        .expect("unset update must return Ok");

    assert!(
        !after.value.meta.extra.contains_key("drop"),
        "named key 'drop' must be removed"
    );
    assert!(
        after.value.meta.extra.contains_key("keep"),
        "sibling key 'keep' must survive an unset of 'drop'"
    );
}

#[test]
fn update_unset_of_absent_key_is_safe_noop() {
    // Unsetting a key that was never set must not error and must not disturb
    // unrelated existing fields.
    let tmp = tempfile::tempdir().unwrap();
    let svc = init_service(tmp.path());

    let id = svc
        .create_doogat("Base", &[], None, "")
        .expect("create doogat");

    let mut fields = BTreeMap::new();
    fields.insert("present".to_string(), Value::String("here".into()));
    set_fields(&svc, &id, fields);

    let after = svc
        .update(UpdateCommand {
            id: id.clone(),
            title: None,
            tags: None,
            doogat_type: None,
            body: None,
            fields: BTreeMap::new(),
            unset_fields: vec!["never_existed".to_string()],
        })
        .expect("unset of an absent key must still return Ok");

    assert!(
        after.value.meta.extra.contains_key("present"),
        "unsetting an absent key must not remove unrelated fields"
    );
}

#[test]
fn update_nonexistent_id_returns_not_found_error() {
    // Service-layer NOT_FOUND: updating an id with no backing doogat must
    // surface `DoogatError::NotFound`, the vocabulary the transports map to a
    // NOT_FOUND code / 404.
    let tmp = tempfile::tempdir().unwrap();
    let svc = init_service(tmp.path());

    let err = svc
        .update(UpdateCommand {
            id: "00000000000000".to_string(),
            title: Some("ignored".into()),
            tags: None,
            doogat_type: None,
            body: None,
            fields: BTreeMap::new(),
            unset_fields: vec![],
        })
        .expect_err("update on a nonexistent id must fail");

    assert!(
        matches!(err, DoogatError::NotFound(_)),
        "service update of a missing id must return DoogatError::NotFound, got: {err:?}"
    );
}
