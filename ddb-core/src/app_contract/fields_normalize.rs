//! One normalization chokepoint for transport field input: a `None` value is
//! a null, which create omits and update turns into an unset.

use std::collections::BTreeMap;

use crate::error::{DoogatError, Result};
use crate::types::Value;

/// Field input as a transport received it; `None` marks an explicit null.
pub type RawFieldsInput = BTreeMap<String, Option<Value>>;

/// Drop null entries; every present value passes through unchanged.
pub fn normalize_create_fields(raw: RawFieldsInput) -> BTreeMap<String, Value> {
    raw.into_iter().filter_map(|(k, v)| v.map(|v| (k, v))).collect()
}

/// Split raw input into sets and unsets. Null entries join `unset_fields`
/// (deduplicated); a key both set and explicitly unset is a validation error.
pub fn normalize_update_fields(
    raw: RawFieldsInput,
    unset_fields: Vec<String>,
) -> Result<(BTreeMap<String, Value>, Vec<String>)> {
    let mut fields = BTreeMap::new();
    let mut nulls = Vec::new();
    for (key, value) in raw {
        match value {
            Some(v) => {
                fields.insert(key, v);
            }
            None => nulls.push(key),
        }
    }
    let mut unset: Vec<String> = Vec::new();
    for key in unset_fields.into_iter().chain(nulls) {
        if fields.contains_key(&key) {
            return Err(DoogatError::Validation(format!(
                "field '{key}' is both set and unset"
            )));
        }
        if !unset.contains(&key) {
            unset.push(key);
        }
    }
    Ok((fields, unset))
}

#[cfg(test)]
mod tests {
    use crate::app_contract::{normalize_create_fields, normalize_update_fields, RawFieldsInput};
    use crate::error::DoogatError;
    use crate::types::Value;
    use std::collections::BTreeMap;

    fn raw(entries: Vec<(&str, Option<Value>)>) -> RawFieldsInput {
        entries
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect()
    }

    fn sorted(mut keys: Vec<String>) -> Vec<String> {
        keys.sort();
        keys
    }

    /// One value of every `Value` variant, nested containers included.
    fn every_variant() -> Vec<(&'static str, Value)> {
        let mut nested = BTreeMap::new();
        nested.insert("inner".to_string(), Value::Number(-1.5));
        nested.insert("flag".to_string(), Value::Bool(false));
        vec![
            ("s", Value::String("true".into())),
            ("n", Value::Number(42.25)),
            ("zero", Value::Number(0.0)),
            ("b_true", Value::Bool(true)),
            ("b_false", Value::Bool(false)),
            (
                "l",
                Value::List(vec![
                    Value::String("a".into()),
                    Value::Number(2.0),
                    Value::Bool(true),
                ]),
            ),
            ("m", Value::Map(nested)),
            ("empty_s", Value::String(String::new())),
            ("empty_l", Value::List(vec![])),
            ("empty_m", Value::Map(BTreeMap::new())),
        ]
    }

    /// The keys of `every_variant()` holding empty-but-present values, which
    /// must never be mistaken for null.
    const EMPTY_KEYS: [&str; 3] = ["empty_s", "empty_l", "empty_m"];

    #[test]
    fn create_passes_every_value_variant_through_unchanged() {
        let input: Vec<(&str, Option<Value>)> = every_variant()
            .into_iter()
            .map(|(k, v)| (k, Some(v)))
            .collect();
        let expected: BTreeMap<String, Value> = every_variant()
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect();

        let out = normalize_create_fields(raw(input));

        for key in EMPTY_KEYS {
            assert!(
                out.contains_key(key),
                "empty value at '{key}' is not null and must not be omitted, got {out:?}"
            );
        }
        assert_eq!(
            out, expected,
            "create must keep native Number/Bool variants, never coerce to strings"
        );
    }

    #[test]
    fn create_omits_null_entries_entirely() {
        let out = normalize_create_fields(raw(vec![
            ("kept", Some(Value::Number(7.0))),
            ("gone", None),
            ("also_gone", None),
        ]));

        let mut expected = BTreeMap::new();
        expected.insert("kept".to_string(), Value::Number(7.0));
        assert_eq!(
            out, expected,
            "a null create field must be absent, not written as an empty or sentinel value"
        );
    }

    #[test]
    fn create_with_only_nulls_yields_an_empty_map() {
        let out = normalize_create_fields(raw(vec![("a", None), ("b", None)]));
        assert!(out.is_empty(), "all-null create input must yield no fields, got {out:?}");
    }

    #[test]
    fn update_passes_every_value_variant_through_unchanged() {
        let input: Vec<(&str, Option<Value>)> = every_variant()
            .into_iter()
            .map(|(k, v)| (k, Some(v)))
            .collect();
        let expected: BTreeMap<String, Value> = every_variant()
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect();

        let (fields, unset) =
            normalize_update_fields(raw(input), vec![]).expect("plain sets must normalize");

        for key in EMPTY_KEYS {
            assert!(
                fields.contains_key(key),
                "an empty value set at '{key}' must stay a set, got {fields:?}"
            );
            assert!(
                !unset.iter().any(|u| u == key),
                "an empty value set at '{key}' must not become an unset, got {unset:?}"
            );
        }
        assert_eq!(fields, expected, "update must keep native value variants");
        assert!(unset.is_empty(), "plain sets must unset nothing, got {unset:?}");
    }

    #[test]
    fn update_turns_null_entries_into_unsets_not_fields() {
        let (fields, unset) = normalize_update_fields(
            raw(vec![
                ("theme", Some(Value::String("dark".into()))),
                ("legacy", None),
                ("old_count", None),
            ]),
            vec![],
        )
        .expect("a null update field must normalize to an unset");

        let mut expected = BTreeMap::new();
        expected.insert("theme".to_string(), Value::String("dark".into()));
        assert_eq!(fields, expected, "a null key must not appear in the set map");
        assert_eq!(
            sorted(unset),
            vec!["legacy".to_string(), "old_count".to_string()],
            "every null key must become exactly one unset"
        );
    }

    #[test]
    fn update_merges_explicit_unsets_with_null_unsets() {
        let (fields, unset) = normalize_update_fields(
            raw(vec![("from_null", None), ("set", Some(Value::Bool(true)))]),
            vec!["from_list".to_string()],
        )
        .expect("disjoint null and explicit unsets must merge");

        assert_eq!(fields.get("set"), Some(&Value::Bool(true)));
        assert_eq!(fields.len(), 1, "only the set key belongs in fields, got {fields:?}");
        assert_eq!(
            sorted(unset),
            vec!["from_list".to_string(), "from_null".to_string()],
            "explicit unsetFields must be kept alongside null-derived unsets"
        );
    }

    #[test]
    fn update_deduplicates_a_key_both_null_and_explicitly_unset() {
        let (fields, unset) = normalize_update_fields(
            raw(vec![("legacy", None)]),
            vec!["legacy".to_string(), "other".to_string()],
        )
        .expect("null plus explicit unset of the same key is not a collision");

        assert!(fields.is_empty(), "no key was set, got {fields:?}");
        assert_eq!(
            sorted(unset),
            vec!["legacy".to_string(), "other".to_string()],
            "a key unset by both null and unsetFields must appear exactly once"
        );
    }

    #[test]
    fn update_deduplicates_a_key_repeated_within_explicit_unsets() {
        let (fields, unset) = normalize_update_fields(
            RawFieldsInput::new(),
            vec!["a".to_string(), "a".to_string()],
        )
        .expect("a repeated explicit unset is not a collision");

        assert!(fields.is_empty(), "no key was set, got {fields:?}");
        assert_eq!(
            unset,
            vec!["a".to_string()],
            "a key repeated in unsetFields must appear exactly once"
        );
    }

    #[test]
    fn update_keeps_every_unset_when_three_or_more_are_combined() {
        let (fields, unset) = normalize_update_fields(
            raw(vec![
                ("null_a", None),
                ("null_b", None),
                ("kept", Some(Value::Number(3.0))),
            ]),
            vec!["explicit_c".to_string(), "explicit_d".to_string()],
        )
        .expect("disjoint null and explicit unsets must merge");

        assert_eq!(fields.len(), 1, "only the set key belongs in fields, got {fields:?}");
        assert_eq!(
            sorted(unset),
            vec![
                "explicit_c".to_string(),
                "explicit_d".to_string(),
                "null_a".to_string(),
                "null_b".to_string(),
            ],
            "every distinct unset key must be returned, none dropped"
        );
    }

    #[test]
    fn update_with_explicit_unsets_only_keeps_sets_and_unsets_apart() {
        let (fields, unset) = normalize_update_fields(
            raw(vec![("a", Some(Value::String("v".into())))]),
            vec!["b".to_string()],
        )
        .expect("a set key and a different unset key must not collide");

        let mut expected = BTreeMap::new();
        expected.insert("a".to_string(), Value::String("v".into()));
        assert_eq!(fields, expected, "the set key must stay in fields");
        assert_eq!(
            unset,
            vec!["b".to_string()],
            "the explicit unset must be returned as given"
        );
    }

    type NormalizedUpdate = crate::error::Result<(BTreeMap<String, Value>, Vec<String>)>;

    fn assert_collision_on_clash(result: NormalizedUpdate, case: &str) {
        match result {
            Err(DoogatError::Validation(msg)) => assert!(
                msg.contains("clash"),
                "{case}: the collision error must name the key 'clash', got {msg:?}"
            ),
            other => {
                panic!("{case}: a set/unset collision must be a Validation error, got {other:?}")
            }
        }
    }

    #[test]
    fn update_rejects_a_key_both_set_and_explicitly_unset() {
        let result = normalize_update_fields(
            raw(vec![
                ("ok", Some(Value::Number(1.0))),
                ("clash", Some(Value::String("v".into()))),
            ]),
            vec!["clash".to_string()],
        );
        assert_collision_on_clash(result, "clash is the only unset");

        let result = normalize_update_fields(
            raw(vec![("clash", Some(Value::String("v".into())))]),
            vec!["other".to_string(), "clash".to_string()],
        );
        assert_collision_on_clash(result, "clash is not the first unset");

        let mut map = BTreeMap::new();
        map.insert("k".to_string(), Value::Bool(true));
        for (case, value) in [
            ("Number value", Value::Number(1.0)),
            ("Bool value", Value::Bool(false)),
            ("List value", Value::List(vec![Value::Number(1.0)])),
            ("Map value", Value::Map(map)),
        ] {
            let result = normalize_update_fields(
                raw(vec![("clash", Some(value))]),
                vec!["clash".to_string()],
            );
            assert_collision_on_clash(result, case);
        }
    }

    #[test]
    fn update_with_empty_input_yields_no_fields_and_no_unsets() {
        let (fields, unset) = normalize_update_fields(RawFieldsInput::new(), vec![])
            .expect("empty update input must normalize without error");
        assert!(fields.is_empty(), "got fields {fields:?}");
        assert!(unset.is_empty(), "got unsets {unset:?}");
    }
}
