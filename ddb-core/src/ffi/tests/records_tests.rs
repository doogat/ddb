//! Tier 1 tests for the FFI structured field input types and the
//! `CreateResultRecord` / `UpdateResultRecord` result records (PRD 00177).

use crate::app_contract::{
    AppOutput, AppWarning, CreateCommand, UnregisteredTypePolicy, UpdateCommand,
};
use crate::ffi::records::{
    core_to_field_value, field_value_to_core, fields_to_core, CreateResultRecord,
    FieldEntryValue, FieldScalar, FieldValue, UpdateResultRecord,
};
use crate::service::DoogatService;
use crate::types::{ConflictAction, DoogatId, DoogatMeta, ParsedDoogat, Value};
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

/// Strings that a trimming, normalizing, or case-folding conversion would
/// change. Every one must cross the FFI boundary byte for byte.
const UNNORMALIZED_STRINGS: &[&str] = &[
    "  x \n",
    "",
    " ",
    "\t",
    "Grüße, 東京 🚀",
    "e\u{301}",
    "MiXeD Case",
];

#[test]
fn field_value_strings_cross_the_boundary_verbatim_in_both_directions() {
    for s in UNNORMALIZED_STRINGS {
        assert_eq!(
            field_value_to_core(FieldValue::Str((*s).into())),
            Value::String((*s).into()),
            "FieldValue::Str({s:?}) must map to the identical core string"
        );
        assert!(
            matches!(core_to_field_value(Value::String((*s).into())), FieldValue::Str(ref got) if got == s),
            "core string {s:?} must map to the identical FieldValue::Str"
        );
    }

    let scalars: Vec<FieldScalar> = UNNORMALIZED_STRINGS
        .iter()
        .map(|s| FieldScalar::Str((*s).into()))
        .collect();
    let core_items: Vec<Value> = UNNORMALIZED_STRINGS
        .iter()
        .map(|s| Value::String((*s).into()))
        .collect();
    assert_eq!(
        field_value_to_core(FieldValue::List(scalars)),
        Value::List(core_items.clone()),
        "list string items must not be rewritten"
    );
    match core_to_field_value(Value::List(core_items)) {
        FieldValue::List(items) => {
            assert_eq!(items.len(), UNNORMALIZED_STRINGS.len());
            for (item, s) in items.iter().zip(UNNORMALIZED_STRINGS) {
                assert!(
                    matches!(item, FieldScalar::Str(got) if got == s),
                    "core list item {s:?} must map to the identical FieldScalar::Str"
                );
            }
        }
        _ => panic!("a core list must convert to FieldValue::List"),
    }
}

/// Numbers that any rounding or precision-reducing conversion would change.
const EXACT_NUMBERS: &[f64] = &[
    0.1,
    -0.3,
    1e-9,
    1234567.891,
    f64::MAX,
    f64::MIN,
    f64::MIN_POSITIVE,
    f64::EPSILON,
    9007199254740993.0,
];

#[test]
fn field_value_numbers_are_preserved_bit_exact_in_both_directions() {
    for &n in EXACT_NUMBERS {
        match field_value_to_core(FieldValue::Num(n)) {
            Value::Number(got) => assert_eq!(got.to_bits(), n.to_bits(), "FieldValue::Num({n:e}) changed"),
            other => panic!("FieldValue::Num({n:e}) must map to Value::Number, got {other:?}"),
        }
        match core_to_field_value(Value::Number(n)) {
            FieldValue::Num(got) => assert_eq!(got.to_bits(), n.to_bits(), "core Number({n:e}) changed"),
            _ => panic!("core Number({n:e}) must map to FieldValue::Num"),
        }
    }

    let list = FieldValue::List(EXACT_NUMBERS.iter().map(|&n| FieldScalar::Num(n)).collect());
    match field_value_to_core(list) {
        Value::List(items) => {
            assert_eq!(items.len(), EXACT_NUMBERS.len());
            for (item, &n) in items.iter().zip(EXACT_NUMBERS) {
                assert!(
                    matches!(item, Value::Number(got) if got.to_bits() == n.to_bits()),
                    "list number {n:e} must be preserved exactly, got {item:?}"
                );
            }
        }
        other => panic!("FieldValue::List must map to Value::List, got {other:?}"),
    }
    match core_to_field_value(Value::List(EXACT_NUMBERS.iter().map(|&n| Value::Number(n)).collect())) {
        FieldValue::List(items) => {
            assert_eq!(items.len(), EXACT_NUMBERS.len());
            for (item, &n) in items.iter().zip(EXACT_NUMBERS) {
                assert!(
                    matches!(item, FieldScalar::Num(got) if got.to_bits() == n.to_bits()),
                    "core list number {n:e} must be preserved exactly"
                );
            }
        }
        _ => panic!("a core list must convert to FieldValue::List"),
    }
}

#[test]
fn list_conversion_keeps_repeated_items_in_order() {
    let core = field_value_to_core(FieldValue::List(vec![
        FieldScalar::Str("a".into()),
        FieldScalar::Str("a".into()),
        FieldScalar::Num(1.0),
        FieldScalar::Num(1.0),
        FieldScalar::Bool(true),
        FieldScalar::Bool(true),
        FieldScalar::Str("a".into()),
    ]));
    let expected = Value::List(vec![
        Value::String("a".into()),
        Value::String("a".into()),
        Value::Number(1.0),
        Value::Number(1.0),
        Value::Bool(true),
        Value::Bool(true),
        Value::String("a".into()),
    ]);
    assert_eq!(core, expected, "repeated list items must be kept, in input order");

    match core_to_field_value(expected) {
        FieldValue::List(items) => {
            assert_eq!(items.len(), 7, "core -> FieldValue must not dedupe list items");
            assert!(matches!(&items[0], FieldScalar::Str(s) if s == "a"));
            assert!(matches!(&items[1], FieldScalar::Str(s) if s == "a"));
            assert!(matches!(items[2], FieldScalar::Num(n) if n == 1.0));
            assert!(matches!(items[3], FieldScalar::Num(n) if n == 1.0));
            assert!(matches!(items[4], FieldScalar::Bool(true)));
            assert!(matches!(items[5], FieldScalar::Bool(true)));
            assert!(matches!(&items[6], FieldScalar::Str(s) if s == "a"));
        }
        _ => panic!("a core list must convert to FieldValue::List"),
    }
}

#[test]
fn core_to_field_value_yields_native_top_level_variants() {
    assert!(matches!(core_to_field_value(Value::Number(3.5)), FieldValue::Num(n) if n == 3.5));
    assert!(matches!(core_to_field_value(Value::Number(-2.0)), FieldValue::Num(n) if n == -2.0));
    assert!(matches!(core_to_field_value(Value::Bool(true)), FieldValue::Bool(true)));
    assert!(matches!(core_to_field_value(Value::Bool(false)), FieldValue::Bool(false)));
    assert!(matches!(core_to_field_value(Value::String("hello".into())), FieldValue::Str(s) if s == "hello"));
    // Strings that look like numbers or booleans stay strings.
    assert!(matches!(core_to_field_value(Value::String("3.5".into())), FieldValue::Str(s) if s == "3.5"));
    assert!(matches!(core_to_field_value(Value::String("true".into())), FieldValue::Str(s) if s == "true"));
}

#[test]
fn field_value_to_core_never_decodes_tagged_looking_strings() {
    for s in [
        "tagged-looking string",
        "num:3.5",
        "bool:true",
        "__num__3.5",
        "__bool__true",
        "Num(3.5)",
        "Bool(false)",
        "{\"num\":3.5}",
        "3.5",
        "true",
        "false",
    ] {
        assert_eq!(
            field_value_to_core(FieldValue::Str(s.into())),
            Value::String(s.into()),
            "FieldValue::Str({s:?}) must stay a core string"
        );
    }
}

#[test]
fn fields_to_core_keeps_every_distinct_key() {
    let entries: Vec<(&str, FieldValue, Value)> = vec![
        ("k1", FieldValue::Str("one".into()), Value::String("one".into())),
        ("k2", FieldValue::Num(2.0), Value::Number(2.0)),
        ("k3", FieldValue::Bool(false), Value::Bool(false)),
        ("k4", FieldValue::Str(" four ".into()), Value::String(" four ".into())),
        ("k5", FieldValue::Num(0.1), Value::Number(0.1)),
        (
            "k6",
            FieldValue::List(vec![FieldScalar::Str("x".into()), FieldScalar::Str("x".into())]),
            Value::List(vec![Value::String("x".into()), Value::String("x".into())]),
        ),
        ("k7", FieldValue::Bool(true), Value::Bool(true)),
    ];
    let mut inputs = Vec::new();
    let mut expected = Vec::new();
    for (k, fv, core) in entries {
        inputs.push(FieldEntryValue {
            key: k.into(),
            value: fv,
        });
        expected.push((k, core));
    }
    let map = fields_to_core(inputs);
    assert_eq!(map.len(), expected.len(), "every distinct key must survive");
    for (k, core) in &expected {
        assert_eq!(map.get(*k), Some(core), "key {k} must map to its own value");
    }
}

/// Build an `AppOutput<ParsedDoogat>` by hand with values a normalizing
/// conversion would change: a non-standard path, a title with edge spaces,
/// a mixed-case type, and unsorted, repeated tags.
fn hand_built_output(warnings: Vec<AppWarning>) -> AppOutput<ParsedDoogat> {
    AppOutput {
        value: ParsedDoogat {
            meta: DoogatMeta {
                id: Some(DoogatId("20260101000000".into())),
                title: Some("  Spaced Title  ".into()),
                date: None,
                doogat_type: Some("MixedCase_Type".into()),
                tags: vec!["zeta".into(), "alpha".into(), "zeta".into()],
                extra: BTreeMap::new(),
            },
            body: String::new(),
            sections: Vec::new(),
            reference_section: String::new(),
            inline_fields: Vec::new(),
            links: Vec::new(),
            body_tags: Vec::new(),
            checkboxes: Vec::new(),
            path: "notes/Custom Path.MD".into(),
            updated_at: None,
        },
        warnings,
    }
}

fn three_warnings() -> Vec<AppWarning> {
    vec![
        AppWarning {
            code: "FIRST_CODE",
            message: "first message".into(),
        },
        AppWarning {
            code: "SECOND_CODE",
            message: "  second message  ".into(),
        },
        AppWarning {
            code: "THIRD_CODE",
            message: "third message".into(),
        },
    ]
}

/// Assert a record copies the hand-built output exactly. A macro, because
/// `CreateResultRecord` and `UpdateResultRecord` are separate types.
macro_rules! assert_mirrors_hand_built {
    ($rec:expr, $expected_warnings:expr) => {{
        let rec = $rec;
        assert_eq!(rec.id, "20260101000000");
        assert_eq!(rec.title.as_deref(), Some("  Spaced Title  "), "title must not be trimmed");
        assert_eq!(rec.doogat_type.as_deref(), Some("MixedCase_Type"), "type must not be case-folded");
        assert_eq!(
            rec.tags,
            vec!["zeta".to_string(), "alpha".to_string(), "zeta".to_string()],
            "tags must be copied in order, without sorting or deduping"
        );
        assert_eq!(rec.path, "notes/Custom Path.MD", "path must be copied, not rebuilt from the id");
        let got: Vec<(String, String)> = rec
            .warnings
            .iter()
            .map(|w| (w.code.clone(), w.message.clone()))
            .collect();
        let expected: Vec<(String, String)> = $expected_warnings;
        assert_eq!(got, expected, "every warning must be forwarded, in order");
    }};
}

fn warning_pairs(warnings: &[AppWarning]) -> Vec<(String, String)> {
    warnings
        .iter()
        .map(|w| (w.code.to_string(), w.message.clone()))
        .collect()
}

#[test]
fn create_result_record_copies_unnormalized_meta_and_all_warnings_in_order() {
    let warnings = three_warnings();
    let expected = warning_pairs(&warnings);
    assert_mirrors_hand_built!(CreateResultRecord::from_output(hand_built_output(warnings)), expected);
}

#[test]
fn update_result_record_copies_unnormalized_meta_and_all_warnings_in_order() {
    let warnings = three_warnings();
    let expected = warning_pairs(&warnings);
    assert_mirrors_hand_built!(UpdateResultRecord::from_output(hand_built_output(warnings)), expected);
}

#[test]
fn result_records_with_no_warnings_have_empty_warning_lists() {
    assert_mirrors_hand_built!(CreateResultRecord::from_output(hand_built_output(Vec::new())), Vec::new());
    assert_mirrors_hand_built!(UpdateResultRecord::from_output(hand_built_output(Vec::new())), Vec::new());
}

#[test]
fn update_result_record_reflects_a_real_update_output() {
    let tmp = tempfile::tempdir().unwrap();
    let (_created, exp) = create_with_warning(tmp.path());
    let svc = DoogatService::open(tmp.path()).expect("open repo");
    let out = svc
        .update(UpdateCommand {
            id: exp.id.clone(),
            title: Some("Renamed Doogat".into()),
            tags: Some(vec!["omega".into(), "alpha".into()]),
            doogat_type: None,
            body: None,
            fields: BTreeMap::new(),
            unset_fields: Vec::new(),
        })
        .expect("update must succeed");
    let expected_path = out.value.path.clone();
    let expected_warnings = warning_pairs(&out.warnings);
    let rec = UpdateResultRecord::from_output(out);

    assert_eq!(rec.id, exp.id);
    assert_eq!(rec.title.as_deref(), Some("Renamed Doogat"));
    assert_eq!(rec.tags, vec!["omega".to_string(), "alpha".to_string()]);
    assert_eq!(rec.path, expected_path);
    let got: Vec<(String, String)> = rec
        .warnings
        .iter()
        .map(|w| (w.code.clone(), w.message.clone()))
        .collect();
    assert_eq!(got, expected_warnings);
}

/// Every structured-CRUD input/output type must be reachable from the
/// crate's own public re-export path (`crate::ffi::<Type>`), not only from
/// the private `crate::ffi::records` module. Without the re-export, this
/// `use` fails to compile, so an external caller cannot name or construct
/// these types to call `create_structured`/`update_structured`.
#[test]
fn structured_crud_types_are_reexported_from_the_ffi_module_root() {
    use crate::ffi::{
        ConflictActionRecord, CreateResultRecord, FieldEntryValue, FieldScalar, FieldValue,
        UpdateResultRecord, UpdateStructuredInput,
    };

    let entry = FieldEntryValue {
        key: "k".into(),
        value: FieldValue::List(vec![FieldScalar::Str("v".into())]),
    };
    let input = UpdateStructuredInput {
        title: None,
        tags: None,
        doogat_type: None,
        body: None,
        fields: vec![entry],
        unset_fields: vec![],
    };
    assert_eq!(
        input.fields.len(),
        1,
        "the publicly re-exported input type must be constructible from outside `ffi::records`"
    );
    assert_eq!(ConflictActionRecord::Error, ConflictActionRecord::Error);

    fn assert_reexported<T>() {}
    assert_reexported::<CreateResultRecord>();
    assert_reexported::<UpdateResultRecord>();
}

/// Create a doogat whose body, meta fields, sections and links are all
/// non-trivial, so a conversion that drops any of them is observable.
fn create_with_rich_payload(dir: &std::path::Path) -> AppOutput<ParsedDoogat> {
    let svc = DoogatService::init(dir).expect("init repo");
    let mut fields = BTreeMap::new();
    fields.insert("date".to_string(), Value::String("2026-01-01".into()));
    fields.insert("extra_field".to_string(), Value::String("extra_value".into()));
    svc.create(CreateCommand {
        title: Some("Rich Doogat".into()),
        tags: vec!["one".into()],
        doogat_type: None,
        body: Some("## Heading\n\nSome text with a [link](target.md).".into()),
        fields,
        on_conflict: ConflictAction::Error,
        unregistered_type_policy: UnregisteredTypePolicy::BaseOnly,
    })
    .expect("create with a rich payload must succeed")
}

#[test]
fn create_result_record_mirrors_body_extra_sections_links_and_updated_at() {
    let tmp = tempfile::tempdir().unwrap();
    let out = create_with_rich_payload(tmp.path());

    let expected_body = out.value.body.clone();
    let expected_date = format!("{:?}", out.value.meta.date);
    let expected_extra = format!("{:?}", out.value.meta.extra);
    let expected_sections = format!("{:?}", out.value.sections);
    let expected_links = format!("{:?}", out.value.links);
    let expected_updated_at = format!("{:?}", out.value.updated_at);

    let rec = CreateResultRecord::from_output(out);

    assert_eq!(rec.body, expected_body, "the record must carry the stored body");
    assert_eq!(format!("{:?}", rec.date), expected_date, "the record must carry meta.date");
    assert_eq!(format!("{:?}", rec.extra), expected_extra, "the record must carry meta.extra");
    assert_eq!(format!("{:?}", rec.sections), expected_sections, "the record must carry sections");
    assert_eq!(format!("{:?}", rec.links), expected_links, "the record must carry links");
    assert_eq!(
        format!("{:?}", rec.updated_at),
        expected_updated_at,
        "the record must carry updated_at"
    );
}

#[test]
fn update_result_record_mirrors_body_extra_sections_links_and_updated_at() {
    let tmp = tempfile::tempdir().unwrap();
    let created = create_with_rich_payload(tmp.path());
    let id = created.value.meta.id.as_ref().expect("created doogat has an id").0.clone();
    let svc = DoogatService::open(tmp.path()).expect("open repo");
    let mut fields = BTreeMap::new();
    fields.insert("date".to_string(), Value::String("2026-02-02".into()));
    let out = svc
        .update(UpdateCommand {
            id,
            title: None,
            tags: None,
            doogat_type: None,
            body: Some("## New Heading\n\nUpdated text with a [link](new-target.md).".into()),
            fields,
            unset_fields: Vec::new(),
        })
        .expect("update with a rich payload must succeed");

    let expected_body = out.value.body.clone();
    let expected_date = format!("{:?}", out.value.meta.date);
    let expected_extra = format!("{:?}", out.value.meta.extra);
    let expected_sections = format!("{:?}", out.value.sections);
    let expected_links = format!("{:?}", out.value.links);
    let expected_updated_at = format!("{:?}", out.value.updated_at);

    let rec = UpdateResultRecord::from_output(out);

    assert_eq!(rec.body, expected_body, "the record must carry the stored body");
    assert_eq!(format!("{:?}", rec.date), expected_date, "the record must carry meta.date");
    assert_eq!(format!("{:?}", rec.extra), expected_extra, "the record must carry meta.extra");
    assert_eq!(format!("{:?}", rec.sections), expected_sections, "the record must carry sections");
    assert_eq!(format!("{:?}", rec.links), expected_links, "the record must carry links");
    assert_eq!(
        format!("{:?}", rec.updated_at),
        expected_updated_at,
        "the record must carry updated_at"
    );
}
