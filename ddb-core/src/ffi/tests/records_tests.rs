//! Tier 1 tests for the FFI structured field input types and the
//! `CreateResultRecord` / `UpdateResultRecord` result records (PRD 00177).

use crate::app_contract::{AppOutput, CreateCommand, UnregisteredTypePolicy};
use crate::ffi::records::{
    core_to_field_value, field_value_to_core, fields_to_core, CreateResultRecord,
    FieldEntryValue, FieldScalar, FieldValue, UpdateResultRecord,
};
use crate::service::DoogatService;
use crate::types::{ConflictAction, ParsedDoogat, Value};
use std::collections::BTreeMap;

/// Convert FieldValue -> core, check the native variant, then core ->
/// FieldValue -> core and assert nothing changed.
fn assert_round_trips(fv: FieldValue, expected_core: Value) {
    let core = field_value_to_core(fv);
    assert_eq!(core, expected_core, "FieldValue must map to the native core Value variant");
    let back = field_value_to_core(core_to_field_value(core.clone()));
    assert_eq!(back, core, "core -> FieldValue -> core must be lossless");
}

#[test]
fn field_value_scalars_and_mixed_list_round_trip_through_core() {
    assert_round_trips(FieldValue::Str("hello".into()), Value::String("hello".into()));
    assert_round_trips(FieldValue::Num(3.5), Value::Number(3.5));
    assert_round_trips(FieldValue::Bool(true), Value::Bool(true));
    assert_round_trips(FieldValue::Bool(false), Value::Bool(false));
    assert_round_trips(
        FieldValue::List(vec![
            FieldScalar::Str("a".into()),
            FieldScalar::Num(-2.0),
            FieldScalar::Bool(true),
        ]),
        Value::List(vec![
            Value::String("a".into()),
            Value::Number(-2.0),
            Value::Bool(true),
        ]),
    );
    assert_round_trips(FieldValue::List(vec![]), Value::List(vec![]));
}

#[test]
fn core_map_inside_list_is_dropped_not_panicked() {
    let mut inner = BTreeMap::new();
    inner.insert("nested".to_string(), Value::String("x".into()));
    let core = Value::List(vec![
        Value::String("keep".into()),
        Value::Map(inner),
        Value::Number(1.0),
    ]);

    match core_to_field_value(core) {
        FieldValue::List(items) => {
            assert_eq!(items.len(), 2, "the Map element must be dropped, siblings kept");
            assert!(matches!(&items[0], FieldScalar::Str(s) if s == "keep"));
            assert!(matches!(items[1], FieldScalar::Num(n) if n == 1.0));
        }
        _ => panic!("a list with a Map element must still convert to FieldValue::List"),
    }
}

#[test]
fn core_top_level_map_degrades_without_panicking() {
    let mut inner = BTreeMap::new();
    inner.insert("nested".to_string(), Value::Bool(true));
    // Must return some FieldValue instead of panicking.
    let _ = core_to_field_value(Value::Map(inner));
}

#[test]
fn fields_to_core_duplicate_key_last_entry_wins() {
    let map = fields_to_core(vec![
        FieldEntryValue {
            key: "k".into(),
            value: FieldValue::Str("a".into()),
        },
        FieldEntryValue {
            key: "k".into(),
            value: FieldValue::Str("b".into()),
        },
    ]);
    assert_eq!(map.len(), 1, "duplicate keys must collapse to one entry");
    assert_eq!(map.get("k"), Some(&Value::String("b".into())));
}

#[test]
fn fields_to_core_preserves_native_number_and_bool_variants() {
    let map = fields_to_core(vec![
        FieldEntryValue {
            key: "count".into(),
            value: FieldValue::Num(42.0),
        },
        FieldEntryValue {
            key: "done".into(),
            value: FieldValue::Bool(true),
        },
        FieldEntryValue {
            key: "label".into(),
            value: FieldValue::Str("42".into()),
        },
    ]);
    assert_eq!(map.get("count"), Some(&Value::Number(42.0)));
    assert_eq!(map.get("done"), Some(&Value::Bool(true)));
    assert_eq!(map.get("label"), Some(&Value::String("42".into())));
}

/// Expected record contents captured from the core output before it is moved.
struct Expected {
    id: String,
    title: Option<String>,
    doogat_type: Option<String>,
    tags: Vec<String>,
    path: String,
    warnings: Vec<(String, String)>,
}

/// Create a doogat with an unregistered type under `BaseOnly`, which yields a
/// real `AppOutput<ParsedDoogat>` carrying an `UNREGISTERED_TYPE_BASE_ONLY`
/// warning.
fn create_with_warning(dir: &std::path::Path) -> (AppOutput<ParsedDoogat>, Expected) {
    let svc = DoogatService::init(dir).expect("init repo");
    let out = svc
        .create(CreateCommand {
            title: Some("Warned Doogat".into()),
            tags: vec!["alpha".into(), "beta".into()],
            doogat_type: Some("unregistered_widget".into()),
            body: Some("body".into()),
            fields: BTreeMap::new(),
            on_conflict: ConflictAction::Error,
            unregistered_type_policy: UnregisteredTypePolicy::BaseOnly,
        })
        .expect("BaseOnly create of an unregistered type must succeed");
    assert!(
        out.warnings.iter().any(|w| w.code == "UNREGISTERED_TYPE_BASE_ONLY"),
        "precondition: create output must carry the base-only warning"
    );
    let meta = &out.value.meta;
    let expected = Expected {
        id: meta.id.as_ref().expect("created doogat has an id").0.clone(),
        title: meta.title.clone(),
        doogat_type: meta.doogat_type.clone(),
        tags: meta.tags.clone(),
        path: out.value.path.clone(),
        warnings: out
            .warnings
            .iter()
            .map(|w| (w.code.to_string(), w.message.clone()))
            .collect(),
    };
    assert_eq!(expected.title.as_deref(), Some("Warned Doogat"));
    assert_eq!(expected.tags, vec!["alpha".to_string(), "beta".to_string()]);
    assert_eq!(expected.path, format!("ddb/{}.md", expected.id));
    (out, expected)
}

#[test]
fn create_result_record_copies_meta_and_preserves_warnings() {
    let tmp = tempfile::tempdir().unwrap();
    let (out, exp) = create_with_warning(tmp.path());
    let rec = CreateResultRecord::from_output(out);

    assert_eq!(rec.id, exp.id);
    assert_eq!(rec.title, exp.title);
    assert_eq!(rec.doogat_type, exp.doogat_type);
    assert_eq!(rec.tags, exp.tags);
    assert_eq!(rec.path, exp.path);
    let got: Vec<(String, String)> = rec
        .warnings
        .iter()
        .map(|w| (w.code.clone(), w.message.clone()))
        .collect();
    assert_eq!(got, exp.warnings, "every warning code/message must be forwarded");
}

#[test]
fn update_result_record_copies_meta_and_preserves_warnings() {
    let tmp = tempfile::tempdir().unwrap();
    let (out, exp) = create_with_warning(tmp.path());
    let rec = UpdateResultRecord::from_output(out);

    assert_eq!(rec.id, exp.id);
    assert_eq!(rec.title, exp.title);
    assert_eq!(rec.doogat_type, exp.doogat_type);
    assert_eq!(rec.tags, exp.tags);
    assert_eq!(rec.path, exp.path);
    let got: Vec<(String, String)> = rec
        .warnings
        .iter()
        .map(|w| (w.code.clone(), w.message.clone()))
        .collect();
    assert_eq!(got, exp.warnings, "every warning code/message must be forwarded");
}
