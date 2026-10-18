//! Tier 1 tests for the structured FFI CRUD entry points
//! `DoogatDriver::create_structured` / `update_structured` (PRD 00177).

use super::{fresh_driver, head_at, setup_app_config_singleton_typedef};
use crate::error::codes::SINGLETON_VIOLATION;
use crate::ffi::records::{
    ConflictActionRecord, DdbError, DdbErrorContextEntry, FieldEntryValue, FieldScalar, FieldValue,
    RebuildWarningRecord, UpdateStructuredInput,
};
use crate::ffi::DoogatDriver;
use crate::types::{DoogatMeta, ParsedDoogat, Value};

/// The warning code a base-only create of an unregistered type must carry.
const UNREGISTERED_TYPE_BASE_ONLY: &str = "UNREGISTERED_TYPE_BASE_ONLY";

fn field(key: &str, value: &str) -> FieldEntryValue {
    FieldEntryValue {
        key: key.into(),
        value: FieldValue::Str(value.into()),
    }
}

fn typed_field(key: &str, value: FieldValue) -> FieldEntryValue {
    FieldEntryValue {
        key: key.into(),
        value,
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

fn codes_of(warnings: &[RebuildWarningRecord]) -> Vec<&str> {
    warnings.iter().map(|w| w.code.as_str()).collect()
}

/// Every doogat path git currently holds, sorted: the observable write set.
fn doogat_paths(driver: &DoogatDriver) -> Vec<String> {
    let mut paths = driver.list_doogats().expect("listing doogats must succeed");
    paths.sort();
    paths
}

fn sql_rows(driver: &DoogatDriver, sql: &str) -> Vec<Vec<String>> {
    driver
        .execute_sql(sql.to_string())
        .unwrap_or_else(|e| panic!("`{sql}` must execute, got {e:?}"))
        .rows
}

/// Re-read the stored file and parse its three zones, so assertions can target
/// the frontmatter zone itself (a field written into the body fails them) and
/// so a broken quoting of a field value fails the parse loudly.
fn stored_doogat(driver: &DoogatDriver, id: &str, path: &str) -> ParsedDoogat {
    let raw = driver
        .read_doogat(id.to_string())
        .unwrap_or_else(|e| panic!("the stored doogat {id} must be readable, got {e:?}"));
    crate::parser::parse(&raw, path)
        .unwrap_or_else(|e| panic!("the stored file must still parse, got {e:?} for:\n{raw}"))
}

fn scalar_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        other => panic!("expected a scalar frontmatter value, got {other:?}"),
    }
}

/// The frontmatter value stored under `key`, rendered as text. Panics when the
/// key is absent, so a dropped field is a failure rather than a silent skip.
fn frontmatter_text(meta: &DoogatMeta, key: &str) -> String {
    let value = meta.extra.get(key).unwrap_or_else(|| {
        panic!(
            "frontmatter must carry {key:?}, got keys {:?}",
            meta.extra.keys().collect::<Vec<_>>()
        )
    });
    scalar_text(value)
}

fn frontmatter_list_text(meta: &DoogatMeta, key: &str) -> Vec<String> {
    match meta.extra.get(key) {
        Some(Value::List(items)) => items.iter().map(scalar_text).collect(),
        other => panic!("frontmatter {key:?} must be a list value, got {other:?}"),
    }
}

fn context_value(context: &[DdbErrorContextEntry], key: &str) -> Option<String> {
    context
        .iter()
        .find(|entry| entry.key == key)
        .and_then(|entry| entry.value.clone())
}

#[test]
fn create_structured_registered_type_persists_meta_fields_and_body() {
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
    assert!(
        rec.warnings.is_empty(),
        "a registered-type create must warn about nothing, got {:?}",
        codes_of(&rec.warnings)
    );
    let paths = doogat_paths(&driver);
    assert!(
        paths.contains(&rec.path),
        "the reported path must be the doogat's own stored path, got {:?} among {paths:?}",
        rec.path
    );

    let stored = stored_doogat(&driver, &rec.id, &rec.path);
    assert_eq!(
        (rec.title.as_deref(), rec.doogat_type.as_deref(), &rec.tags),
        (
            stored.meta.title.as_deref(),
            stored.meta.doogat_type.as_deref(),
            &stored.meta.tags
        ),
        "the returned record must report what was stored, not echo its own arguments"
    );
    assert_eq!(stored.meta.title.as_deref(), Some("Config"));
    assert_eq!(stored.meta.doogat_type.as_deref(), Some("app_config"));
    assert_eq!(stored.meta.tags, vec!["cfg".to_string(), "ui".to_string()]);
    assert_eq!(
        frontmatter_text(&stored.meta, "theme"),
        "dark",
        "a structured field must land in the frontmatter zone"
    );
    assert!(
        stored.body.contains("Body text"),
        "the given body must be stored, got:\n{}",
        stored.body
    );
    assert_eq!(
        sql_rows(&driver, "SELECT theme FROM app_config"),
        vec![vec!["dark".to_string()]],
        "the created field must be queryable in its typed table"
    );
}

#[test]
fn create_structured_with_a_second_registered_type_yields_no_warnings() {
    let (_tmp, driver) = fresh_driver();
    driver.reindex().expect("reindex must succeed");
    driver
        .execute_sql("CREATE TABLE widget (size TEXT)".into())
        .expect("CREATE TABLE must register a second type");

    let rec = driver
        .create_structured(
            Some("Widget".into()),
            vec![],
            Some("widget".into()),
            None,
            vec![],
            ConflictActionRecord::Error,
        )
        .expect("a registered type must create without error");

    assert_14_digit_id(&rec.id);
    assert_eq!(rec.doogat_type.as_deref(), Some("widget"));
    assert!(
        rec.warnings.is_empty(),
        "any registered type must produce no warnings, got {:?}",
        codes_of(&rec.warnings)
    );
}

/// Create the one allowed `app_config` singleton and return its id.
fn create_first_singleton(driver: &DoogatDriver) -> String {
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
    let (tmp, driver) = fresh_driver();
    setup_app_config_singleton_typedef(&driver);
    let first_id = create_first_singleton(&driver);
    let paths_before = doogat_paths(&driver);
    let head_before = head_at(tmp.path());

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
    assert_eq!(
        doogat_paths(&driver),
        paths_before,
        "Ignore must not add a doogat"
    );
    assert_eq!(
        head_at(tmp.path()),
        head_before,
        "Ignore must detect the collision before writing: a write-then-recover moves HEAD"
    );
    let stored = stored_doogat(&driver, &first_id, &rec.path);
    assert_eq!(
        frontmatter_text(&stored.meta, "theme"),
        "dark",
        "Ignore must leave the existing doogat untouched"
    );
}

#[test]
fn create_structured_singleton_conflict_with_error_rejects() {
    let (_tmp, driver) = fresh_driver();
    setup_app_config_singleton_typedef(&driver);
    let first_id = create_first_singleton(&driver);
    let paths_before = doogat_paths(&driver);

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
        DdbError::Conflict { msg, code, context } => {
            assert_eq!(
                code.as_deref(),
                Some(SINGLETON_VIOLATION),
                "the rejection must carry the singleton-violation code"
            );
            assert!(!msg.is_empty(), "the rejection must carry a message");
            assert_eq!(
                context_value(&context, "table").as_deref(),
                Some("app_config"),
                "the rejection must name the colliding table, got {context:?}"
            );
            assert_eq!(
                context_value(&context, "existing_id").as_deref(),
                Some(first_id.as_str()),
                "the rejection must name the existing doogat, got {context:?}"
            );
        }
        other => panic!("expected a Conflict error for the singleton collision, got {other:?}"),
    }
    assert_eq!(
        doogat_paths(&driver),
        paths_before,
        "the rejected create must not have written anything"
    );
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
        codes_of(&rec.warnings),
        vec![UNREGISTERED_TYPE_BASE_ONLY],
        "an unregistered type must yield exactly the base-only warning code"
    );
    assert!(
        rec.warnings.iter().all(|w| !w.message.is_empty()),
        "the base-only warning must carry a message"
    );
    let stored = stored_doogat(&driver, &rec.id, &rec.path);
    assert_eq!(
        stored.meta.doogat_type.as_deref(),
        Some("unregistered_widget"),
        "the unregistered type must still be stored"
    );
    assert_eq!(
        frontmatter_text(&stored.meta, "size"),
        "large",
        "a base-only create must still persist the given fields"
    );
}

#[test]
fn create_structured_stores_num_bool_list_and_quoted_field_values() {
    let (_tmp, driver) = fresh_driver();

    let rec = driver
        .create_structured(
            Some("Values".into()),
            vec![],
            None,
            None,
            vec![
                typed_field("count", FieldValue::Num(7.0)),
                typed_field("enabled", FieldValue::Bool(true)),
                typed_field(
                    "parts",
                    FieldValue::List(vec![
                        FieldScalar::Str("a".into()),
                        FieldScalar::Num(2.0),
                        FieldScalar::Bool(false),
                    ]),
                ),
                field("tricky", "a: b"),
                field("unclosed", "[unclosed"),
            ],
            ConflictActionRecord::Error,
        )
        .expect("non-string field values must be accepted");

    let meta = stored_doogat(&driver, &rec.id, &rec.path).meta;
    assert_eq!(
        frontmatter_text(&meta, "count"),
        "7",
        "a numeric field must survive storage"
    );
    assert_eq!(
        frontmatter_text(&meta, "enabled"),
        "true",
        "a boolean field must survive storage"
    );
    assert_eq!(
        frontmatter_list_text(&meta, "parts"),
        vec!["a".to_string(), "2".to_string(), "false".to_string()],
        "a list field must survive storage, in order"
    );
    assert_eq!(
        frontmatter_text(&meta, "tricky"),
        "a: b",
        "a value containing a colon must round-trip through the frontmatter"
    );
    assert_eq!(
        frontmatter_text(&meta, "unclosed"),
        "[unclosed",
        "a value opening a YAML flow sequence must round-trip through the frontmatter"
    );
}

/// A base doogat with two frontmatter extras, a title, a tag and a body.
fn create_note_with_two_fields(driver: &DoogatDriver) -> crate::ffi::records::CreateResultRecord {
    driver
        .create_structured(
            Some("Note".into()),
            vec!["t1".into()],
            None,
            Some("Body text".into()),
            vec![field("legacy_flag", "yes"), field("theme_hint", "old")],
            ConflictActionRecord::Error,
        )
        .expect("create_structured without a type must succeed")
}

#[test]
fn update_structured_unsets_key_and_sets_field_keeping_other_frontmatter() {
    let (_tmp, driver) = fresh_driver();
    let created = create_note_with_two_fields(&driver);
    let before = stored_doogat(&driver, &created.id, &created.path);
    assert_eq!(
        frontmatter_text(&before.meta, "legacy_flag"),
        "yes",
        "precondition: the key to unset must exist first"
    );

    let rec = driver
        .update_structured(
            created.id.clone(),
            UpdateStructuredInput {
                title: None,
                tags: None,
                doogat_type: None,
                body: None,
                fields: vec![field("theme_hint", "new")],
                unset_fields: vec!["legacy_flag".into()],
            },
        )
        .expect("update_structured must succeed");

    assert_eq!(rec.id, created.id, "the update must report the same id");
    assert_eq!(rec.path, created.path, "the update must not move the doogat");
    for warning in &rec.warnings {
        assert!(
            !warning.code.is_empty() && !warning.message.is_empty(),
            "a forwarded warning must carry a code and a message, got {warning:?}"
        );
    }

    let after = stored_doogat(&driver, &created.id, &created.path);
    assert!(
        !after.meta.extra.contains_key("legacy_flag"),
        "the unset key must be gone from the frontmatter, got {:?}",
        after.meta.extra
    );
    assert_eq!(
        frontmatter_text(&after.meta, "theme_hint"),
        "new",
        "the field set through `fields` must land in the frontmatter zone"
    );
    assert_eq!(
        after.meta.title.as_deref(),
        Some("Note"),
        "a None title must leave the stored title alone"
    );
    assert_eq!(
        after.meta.tags,
        vec!["t1".to_string()],
        "None tags must leave the stored tags alone"
    );
    assert_eq!(
        after.meta.id.as_ref().map(|id| id.0.as_str()),
        Some(created.id.as_str()),
        "the stored id must survive the update"
    );
    assert!(
        after.body.contains("Body text"),
        "a None body must leave the stored body alone, got:\n{}",
        after.body
    );
}

#[test]
fn update_structured_applies_new_title_tags_type_and_body() {
    let (_tmp, driver) = fresh_driver();
    setup_app_config_singleton_typedef(&driver);
    let created = create_note_with_two_fields(&driver);

    let rec = driver
        .update_structured(
            created.id.clone(),
            UpdateStructuredInput {
                title: Some("Renamed".into()),
                tags: Some(vec!["t2".into(), "t3".into()]),
                doogat_type: Some("app_config".into()),
                body: Some("Fresh body".into()),
                fields: vec![field("theme", "dark")],
                unset_fields: vec![],
            },
        )
        .expect("update_structured must apply every Some argument");

    let stored = stored_doogat(&driver, &created.id, &rec.path);
    assert_eq!(
        stored.meta.title.as_deref(),
        Some("Renamed"),
        "a Some title must be stored"
    );
    assert_eq!(
        stored.meta.tags,
        vec!["t2".to_string(), "t3".to_string()],
        "Some tags must replace the stored tags"
    );
    assert_eq!(
        stored.meta.doogat_type.as_deref(),
        Some("app_config"),
        "a Some doogat_type must be stored"
    );
    assert!(
        stored.body.contains("Fresh body"),
        "a Some body must be stored, got:\n{}",
        stored.body
    );
    assert!(
        !stored.body.contains("Body text"),
        "the replaced body must be gone, got:\n{}",
        stored.body
    );
    assert_eq!(
        (rec.title.as_deref(), rec.doogat_type.as_deref(), &rec.tags),
        (
            stored.meta.title.as_deref(),
            stored.meta.doogat_type.as_deref(),
            &stored.meta.tags
        ),
        "the returned record must report what was stored"
    );
}

#[test]
fn update_structured_field_is_queryable_in_the_typed_table() {
    let (_tmp, driver) = fresh_driver();
    setup_app_config_singleton_typedef(&driver);
    let first_id = create_first_singleton(&driver);

    driver
        .update_structured(
            first_id.clone(),
            UpdateStructuredInput {
                title: None,
                tags: None,
                doogat_type: None,
                body: None,
                fields: vec![field("theme", "light")],
                unset_fields: vec![],
            },
        )
        .expect("update_structured must succeed");

    assert_eq!(
        sql_rows(&driver, "SELECT theme FROM app_config"),
        vec![vec!["light".to_string()]],
        "the updated field must be visible to SQL without an intervening reindex"
    );
}

#[test]
fn update_structured_on_an_unknown_id_errors_without_creating_a_doogat() {
    let (_tmp, driver) = fresh_driver();
    let paths_before = doogat_paths(&driver);
    let missing = "20200101000000";

    let err = driver
        .update_structured(
            missing.to_string(),
            UpdateStructuredInput {
                title: Some("Ghost".into()),
                tags: None,
                doogat_type: None,
                body: Some("Ghost body".into()),
                fields: vec![field("theme_hint", "new")],
                unset_fields: vec![],
            },
        )
        .expect_err("updating an id that does not exist must error");

    match err {
        DdbError::NotFound { .. } => {}
        other => panic!("expected exactly a NotFound error for an unknown id, got {other:?}"),
    }
    assert_eq!(
        doogat_paths(&driver),
        paths_before,
        "a failed update must not create a doogat"
    );
    assert!(
        driver.read_doogat(missing.to_string()).is_err(),
        "the unknown id must still not resolve after the failed update"
    );
}

#[test]
fn update_structured_to_an_unregistered_type_forwards_the_base_only_warning() {
    let (_tmp, driver) = fresh_driver();
    let created = create_note_with_two_fields(&driver);

    let rec = driver
        .update_structured(
            created.id.clone(),
            UpdateStructuredInput {
                title: None,
                tags: None,
                doogat_type: Some("unregistered_widget".into()),
                body: None,
                fields: vec![],
                unset_fields: vec![],
            },
        )
        .expect("update_structured switching to an unregistered type must not error");

    assert_eq!(
        codes_of(&rec.warnings),
        vec![UNREGISTERED_TYPE_BASE_ONLY],
        "switching to an unregistered type must forward exactly the base-only warning code"
    );
    assert!(
        rec.warnings.iter().all(|w| !w.message.is_empty()),
        "the forwarded warning must carry a non-empty message, got {:?}",
        rec.warnings
    );
}
