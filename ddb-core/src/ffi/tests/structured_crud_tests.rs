//! Tier 1 tests for the structured FFI CRUD entry points
//! `DoogatDriver::create_structured` / `update_structured` (PRD 00177).

use super::{fresh_driver, setup_app_config_singleton_typedef};
use crate::error::codes::SINGLETON_VIOLATION;
use crate::ffi::records::{ConflictActionRecord, DdbError, FieldEntryValue, FieldValue};

/// The warning code a base-only create of an unregistered type must carry.
const UNREGISTERED_TYPE_BASE_ONLY: &str = "UNREGISTERED_TYPE_BASE_ONLY";

fn field(key: &str, value: &str) -> FieldEntryValue {
    FieldEntryValue {
        key: key.into(),
        value: FieldValue::Str(value.into()),
    }
}

/// A minted id is exactly the 14-digit timestamp form, not a uuid or a path.
fn assert_14_digit_id(id: &str) {
    assert_eq!(id.len(), 14, "id must be a 14-digit timestamp, got {id:?}");
    assert!(
        id.chars().all(|c| c.is_ascii_digit()),
        "id must be all ASCII digits, got {id:?}"
    );
}

#[test]
fn create_structured_registered_type_echoes_meta_and_persists_fields() {
    let (_tmp, driver) = fresh_driver();
    setup_app_config_singleton_typedef(&driver);

    let rec = driver
        .create_structured(
            Some("Config".into()),
            vec!["cfg".into(), "ui".into()],
            Some("app_config".into()),
            Some("Body text".into()),
            vec![field("theme", "dark")],
            ConflictActionRecord::Error,
        )
        .expect("create_structured with a registered type must succeed");

    assert_14_digit_id(&rec.id);
    assert_eq!(rec.title.as_deref(), Some("Config"), "title must echo the input");
    assert_eq!(
        rec.doogat_type.as_deref(),
        Some("app_config"),
        "doogat_type must echo the input"
    );
    assert_eq!(
        rec.tags,
        vec!["cfg".to_string(), "ui".to_string()],
        "tags must echo the input, in order"
    );
    assert_eq!(rec.path, format!("ddb/{}.md", rec.id), "path must point at the stored doogat");
    let warning_codes: Vec<&str> = rec.warnings.iter().map(|w| w.code.as_str()).collect();
    assert!(
        warning_codes.is_empty(),
        "a registered-type create must warn about nothing, got {warning_codes:?}"
    );

    let raw = driver
        .read_doogat(rec.id.clone())
        .expect("the created doogat must be readable by its returned id");
    assert!(
        raw.contains("theme: dark"),
        "structured fields must land in the stored frontmatter, got:\n{raw}"
    );
    assert!(raw.contains("Config"), "the title must be stored, got:\n{raw}");
}

/// Create the one allowed `app_config` singleton and return its id.
fn create_first_singleton(driver: &crate::ffi::DoogatDriver) -> String {
    let rec = driver
        .create_structured(
            Some("Config".into()),
            vec![],
            Some("app_config".into()),
            None,
            vec![field("theme", "dark")],
            ConflictActionRecord::Error,
        )
        .expect("the first singleton create must succeed");
    rec.id
}

#[test]
fn create_structured_singleton_conflict_with_ignore_returns_existing_id() {
    let (_tmp, driver) = fresh_driver();
    setup_app_config_singleton_typedef(&driver);
    let first_id = create_first_singleton(&driver);

    let rec = driver
        .create_structured(
            Some("Config 2".into()),
            vec![],
            Some("app_config".into()),
            None,
            vec![field("theme", "light")],
            ConflictActionRecord::Ignore,
        )
        .expect("Ignore must resolve the singleton collision instead of erroring");

    assert_eq!(
        rec.id, first_id,
        "Ignore must return the existing doogat's id, not mint a second one"
    );
    let raw = driver
        .read_doogat(first_id.clone())
        .expect("the existing singleton must still be readable");
    assert!(
        raw.contains("theme: dark"),
        "Ignore must leave the existing doogat untouched, got:\n{raw}"
    );
    assert!(
        !raw.contains("theme: light"),
        "Ignore must not apply the colliding input, got:\n{raw}"
    );
}

#[test]
fn create_structured_singleton_conflict_with_error_rejects() {
    let (_tmp, driver) = fresh_driver();
    setup_app_config_singleton_typedef(&driver);
    let first_id = create_first_singleton(&driver);

    let err = driver
        .create_structured(
            Some("Config 2".into()),
            vec![],
            Some("app_config".into()),
            None,
            vec![field("theme", "light")],
            ConflictActionRecord::Error,
        )
        .expect_err("Error must reject the second singleton create");

    match err {
        DdbError::Validation { code, .. } => assert_eq!(
            code.as_deref(),
            Some(SINGLETON_VIOLATION),
            "the rejection must carry the singleton-violation code"
        ),
        other => panic!("expected a Validation error for the singleton collision, got {other:?}"),
    }
    let raw = driver
        .read_doogat(first_id.clone())
        .expect("the existing singleton must survive the rejected create");
    assert!(
        !raw.contains("theme: light"),
        "the rejected create must not have written anything, got:\n{raw}"
    );
}

#[test]
fn update_structured_unsets_key_and_sets_field_value() {
    let (_tmp, driver) = fresh_driver();
    let created = driver
        .create_structured(
            Some("Note".into()),
            vec!["t1".into()],
            None,
            Some("Body text".into()),
            vec![field("legacy_flag", "yes"), field("theme_hint", "old")],
            ConflictActionRecord::Error,
        )
        .expect("create_structured without a type must succeed");
    let before = driver.read_doogat(created.id.clone()).expect("read after create");
    assert!(
        before.contains("legacy_flag"),
        "precondition: the key to unset must exist first, got:\n{before}"
    );

    let rec = driver
        .update_structured(
            created.id.clone(),
            None,
            None,
            None,
            None,
            vec![field("theme_hint", "new")],
            vec!["legacy_flag".into()],
        )
        .expect("update_structured must succeed");

    assert_eq!(rec.id, created.id, "the update must report the same id");
    assert_eq!(rec.path, created.path, "the update must not move the doogat");
    for warning in &rec.warnings {
        assert!(!warning.code.is_empty(), "a forwarded warning must carry a code");
        assert!(!warning.message.is_empty(), "a forwarded warning must carry a message");
    }

    let raw = driver.read_doogat(created.id.clone()).expect("read after update");
    assert!(
        !raw.contains("legacy_flag"),
        "the unset key must be gone from the frontmatter, got:\n{raw}"
    );
    assert!(
        raw.contains("theme_hint: new"),
        "the field set through `fields` must land, got:\n{raw}"
    );
    assert!(
        !raw.contains("theme_hint: old"),
        "the old field value must be replaced, got:\n{raw}"
    );
    assert!(raw.contains("Body text"), "the untouched body must survive, got:\n{raw}");
}

#[test]
fn create_structured_unregistered_type_succeeds_with_base_only_warning() {
    let (_tmp, driver) = fresh_driver();

    let rec = driver
        .create_structured(
            Some("Widget".into()),
            vec![],
            Some("unregistered_widget".into()),
            None,
            vec![field("size", "large")],
            ConflictActionRecord::Error,
        )
        .expect("an unregistered doogat_type must not error");

    assert_14_digit_id(&rec.id);
    assert_eq!(
        rec.doogat_type.as_deref(),
        Some("unregistered_widget"),
        "the unregistered type must still be recorded"
    );
    let codes: Vec<&str> = rec.warnings.iter().map(|w| w.code.as_str()).collect();
    assert_eq!(
        codes,
        vec![UNREGISTERED_TYPE_BASE_ONLY],
        "an unregistered type must yield exactly the base-only warning code"
    );
    assert!(
        rec.warnings.iter().all(|w| !w.message.is_empty()),
        "the base-only warning must carry a message"
    );
    let raw = driver.read_doogat(rec.id.clone()).expect("read after base-only create");
    assert!(
        raw.contains("size: large"),
        "base-only create must still persist the given fields, got:\n{raw}"
    );
}
