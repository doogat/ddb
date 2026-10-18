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
        ]
    }

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
    fn update_rejects_a_key_both_set_and_explicitly_unset() {
        let result = normalize_update_fields(
            raw(vec![
                ("ok", Some(Value::Number(1.0))),
                ("clash", Some(Value::String("v".into()))),
            ]),
            vec!["clash".to_string()],
        );

        match result {
            Err(DoogatError::Validation(_)) => {}
            other => panic!("a set/unset collision must be a Validation error, got {other:?}"),
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
