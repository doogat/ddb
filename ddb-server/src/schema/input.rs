//! Shared decoding helpers for dynamic GraphQL mutation inputs.
//!
//! `createDoogat`, `createMany`, `updateDoogat`, and `batchUpdate` all pull the
//! same shapes (title, content, tags, type, fields-JSON, unsetFields,
//! onConflict) out of their dynamic input objects. These helpers keep that
//! decoding in one place so the resolvers stay thin and the parsing rules
//! (which `Value` kinds count, how absent vs wrong-typed fields behave) live
//! once.
//!
//! Helpers take `&IndexMap<Name, Value>` — the map behind an `ObjectAccessor`
//! (`ObjectAccessor::as_index_map`) and `ResolverContext.args` — so they can be
//! unit-tested without a live schema (`ObjectAccessor` has no public
//! constructor). String reads (`opt_string`) accept only `Value::String`. The
//! conflict-action read intentionally accepts EITHER a GraphQL enum value
//! (`Value::Enum`) OR its string spelling (`Value::String`): both map `"IGNORE"`
//! to `ConflictAction::Ignore` and everything else to `Error`. The schema types
//! `onConflict` as an enum, so in production only the enum form reaches the
//! resolver; the string form is for direct arg-map callers and tests.

use std::collections::BTreeMap;

use async_graphql::{Name, Value as GqlValue};
use ddb_core::types::{ConflictAction, Value as DdbValue};
use indexmap::IndexMap;

use super::base_types::parse_fields_json;

/// The underlying map of a dynamic GraphQL input object or argument set.
pub(crate) type GqlObject = IndexMap<Name, GqlValue>;

/// Optional string field. `None` when absent or not a string value.
pub(crate) fn opt_string(obj: &GqlObject, key: &str) -> Option<String> {
    match obj.get(key) {
        Some(GqlValue::String(s)) => Some(s.clone()),
        _ => None,
    }
}

/// Optional list-of-strings field. `None` when absent or not a list; non-string
/// list elements are skipped.
pub(crate) fn opt_string_list(obj: &GqlObject, key: &str) -> Option<Vec<String>> {
    match obj.get(key) {
        Some(GqlValue::List(items)) => Some(
            items
                .iter()
                .filter_map(|v| match v {
                    GqlValue::String(s) => Some(s.clone()),
                    _ => None,
                })
                .collect(),
        ),
        _ => None,
    }
}

/// List-of-strings field, empty when absent.
pub(crate) fn string_list(obj: &GqlObject, key: &str) -> Vec<String> {
    opt_string_list(obj, key).unwrap_or_default()
}

/// Decode the `fields` JSON-string field into a typed-field map. Empty map when
/// the field is absent or not a string. Errors propagate the JSON parse message.
pub(crate) fn fields_map(obj: &GqlObject) -> Result<BTreeMap<String, DdbValue>, String> {
    match obj.get("fields") {
        Some(GqlValue::String(json)) => parse_fields_json(json),
        _ => Ok(BTreeMap::new()),
    }
}

/// Like [`fields_map`] but `None` when the field is absent (batch-update
/// semantics: distinguish "leave fields unchanged" from "set fields to empty").
pub(crate) fn opt_fields_map(obj: &GqlObject) -> Result<Option<BTreeMap<String, DdbValue>>, String> {
    match obj.get("fields") {
        Some(GqlValue::String(json)) => Ok(Some(parse_fields_json(json)?)),
        _ => Ok(None),
    }
}

/// Decode the `onConflict` argument. Defaults to `Error`; only an explicit
/// `IGNORE` (enum or string) selects `Ignore`.
pub(crate) fn conflict_action(args: &GqlObject) -> ConflictAction {
    let raw = match args.get("onConflict") {
        Some(GqlValue::Enum(s)) => Some(s.as_str()),
        Some(GqlValue::String(s)) => Some(s.as_str()),
        _ => None,
    };
    if raw == Some("IGNORE") {
        ConflictAction::Ignore
    } else {
        ConflictAction::Error
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obj(pairs: Vec<(&str, GqlValue)>) -> GqlObject {
        pairs.into_iter().map(|(k, v)| (Name::new(k), v)).collect()
    }

    fn s(v: &str) -> GqlValue {
        GqlValue::String(v.to_string())
    }

    fn str_list(items: &[&str]) -> GqlValue {
        GqlValue::List(items.iter().map(|i| s(i)).collect())
    }

    #[test]
    fn opt_string_reads_present_string() {
        let o = obj(vec![("title", s("Hello"))]);
        assert_eq!(opt_string(&o, "title"), Some("Hello".to_string()));
    }

    #[test]
    fn opt_string_none_when_absent() {
        assert_eq!(opt_string(&obj(vec![]), "title"), None);
    }

    #[test]
    fn opt_string_none_when_wrong_type() {
        let o = obj(vec![("title", GqlValue::Boolean(true))]);
        assert_eq!(opt_string(&o, "title"), None);
    }

    #[test]
    fn string_list_collects_strings() {
        let o = obj(vec![("tags", str_list(&["a", "b"]))]);
        assert_eq!(
            string_list(&o, "tags"),
            vec!["a".to_string(), "b".to_string()]
        );
    }

    #[test]
    fn string_list_empty_when_absent() {
        assert!(string_list(&obj(vec![]), "tags").is_empty());
    }

    #[test]
    fn string_list_skips_non_string_elements() {
        let o = obj(vec![(
            "tags",
            GqlValue::List(vec![s("a"), GqlValue::Boolean(true), s("b")]),
        )]);
        assert_eq!(
            string_list(&o, "tags"),
            vec!["a".to_string(), "b".to_string()]
        );
    }

    #[test]
    fn opt_string_list_distinguishes_none_from_empty() {
        assert_eq!(opt_string_list(&obj(vec![]), "tags"), None);
        assert_eq!(
            opt_string_list(&obj(vec![("tags", GqlValue::List(vec![]))]), "tags"),
            Some(vec![])
        );
    }

    fn num(n: f64) -> GqlValue {
        GqlValue::Number(async_graphql::Number::from_f64(n).unwrap())
    }

    /// A `fields` JSON-scalar object: the shape a `JSON` variable decodes to.
    fn fields_obj(pairs: Vec<(&str, GqlValue)>) -> GqlValue {
        GqlValue::Object(obj(pairs))
    }

    fn sorted(mut keys: Vec<String>) -> Vec<String> {
        keys.sort();
        keys
    }

    /// The legacy JSON-encoded-string shape, holding a perfectly valid object.
    fn legacy_string_fields() -> GqlValue {
        s(r#"{"author":"alice"}"#)
    }

    fn assert_migration_error<T: std::fmt::Debug>(result: Result<T, String>, case: &str) {
        match result {
            Err(msg) => assert!(
                msg.contains("JSON object") && msg.contains("JSON-encoded string"),
                "{case}: the error must name the string-to-object migration, got {msg:?}"
            ),
            Ok(v) => panic!("{case}: a JSON-encoded string must be rejected, not reparsed, got {v:?}"),
        }
    }

    #[test]
    fn fields_map_decodes_object_natively_and_drops_nulls() {
        let o = obj(vec![(
            "fields",
            fields_obj(vec![
                ("author", s("alice")),
                ("count", num(3.0)),
                ("done", GqlValue::Boolean(true)),
                ("gone", GqlValue::Null),
            ]),
        )]);
        let m = fields_map(&o).expect("an object fields value must decode");
        let expected = BTreeMap::from([
            ("author".to_string(), DdbValue::String("alice".to_string())),
            ("count".to_string(), DdbValue::Number(3.0)),
            ("done".to_string(), DdbValue::Bool(true)),
        ]);
        assert_eq!(
            m, expected,
            "create fields keep native variants and omit null keys entirely"
        );
    }

    #[test]
    fn fields_map_empty_when_absent_or_empty_object() {
        assert!(fields_map(&obj(vec![])).unwrap().is_empty());
        let o = obj(vec![("fields", fields_obj(vec![]))]);
        assert!(fields_map(&o).unwrap().is_empty());
    }

    #[test]
    fn fields_map_rejects_legacy_json_string_with_migration_error() {
        let o = obj(vec![("fields", legacy_string_fields())]);
        assert_migration_error(fields_map(&o), "fields_map");
    }

    #[test]
    fn fields_and_unsets_turns_null_into_unset_not_field() {
        let o = obj(vec![(
            "fields",
            fields_obj(vec![("theme", s("dark")), ("legacy", GqlValue::Null)]),
        )]);
        let (fields, unsets) = fields_and_unsets(&o).expect("null update field must normalize");
        assert_eq!(
            fields,
            BTreeMap::from([("theme".to_string(), DdbValue::String("dark".to_string()))]),
            "a null key must not appear in the set map"
        );
        assert_eq!(unsets, vec!["legacy".to_string()]);
    }

    #[test]
    fn fields_and_unsets_merges_explicit_unset_fields_with_null_unsets() {
        let o = obj(vec![
            (
                "fields",
                fields_obj(vec![("from_null", GqlValue::Null), ("set", GqlValue::Boolean(false))]),
            ),
            ("unsetFields", str_list(&["from_list", "from_null"])),
        ]);
        let (fields, unsets) = fields_and_unsets(&o).expect("disjoint unsets must merge");
        assert_eq!(
            fields,
            BTreeMap::from([("set".to_string(), DdbValue::Bool(false))])
        );
        assert_eq!(
            sorted(unsets),
            vec!["from_list".to_string(), "from_null".to_string()],
            "explicit unsetFields join null-derived unsets, each key once"
        );
    }

    #[test]
    fn fields_and_unsets_keeps_explicit_unsets_when_fields_absent() {
        let o = obj(vec![("unsetFields", str_list(&["a"]))]);
        let (fields, unsets) = fields_and_unsets(&o).expect("unsetFields alone must normalize");
        assert!(fields.is_empty(), "got fields {fields:?}");
        assert_eq!(unsets, vec!["a".to_string()]);
    }

    #[test]
    fn fields_and_unsets_rejects_key_both_set_and_explicitly_unset() {
        let o = obj(vec![
            ("fields", fields_obj(vec![("clash", num(1.0))])),
            ("unsetFields", str_list(&["clash"])),
        ]);
        match fields_and_unsets(&o) {
            Err(msg) => assert!(
                msg.contains("clash"),
                "the collision error must name the key 'clash', got {msg:?}"
            ),
            Ok(v) => panic!("a set/unset collision must be an error, got {v:?}"),
        }
    }

    #[test]
    fn fields_and_unsets_rejects_legacy_json_string_with_migration_error() {
        let o = obj(vec![("fields", legacy_string_fields())]);
        assert_migration_error(fields_and_unsets(&o), "fields_and_unsets");
    }

    #[test]
    fn opt_fields_and_unsets_none_when_fields_absent() {
        assert_eq!(opt_fields_and_unsets(&obj(vec![])).unwrap(), None);
    }

    #[test]
    fn opt_fields_and_unsets_some_with_null_as_unset_when_present() {
        let o = obj(vec![(
            "fields",
            fields_obj(vec![("x", num(1.5)), ("dropped", GqlValue::Null)]),
        )]);
        assert_eq!(
            opt_fields_and_unsets(&o).unwrap(),
            Some((
                BTreeMap::from([("x".to_string(), DdbValue::Number(1.5))]),
                vec!["dropped".to_string()]
            )),
            "present fields decode natively and a null key becomes an unset"
        );
    }

    #[test]
    fn opt_fields_and_unsets_some_empty_for_empty_object() {
        let o = obj(vec![("fields", fields_obj(vec![]))]);
        assert_eq!(
            opt_fields_and_unsets(&o).unwrap(),
            Some((BTreeMap::new(), vec![])),
            "an empty object is present-but-empty, not absent"
        );
    }

    #[test]
    fn opt_fields_and_unsets_rejects_legacy_json_string() {
        let o = obj(vec![("fields", legacy_string_fields())]);
        assert!(
            opt_fields_and_unsets(&o).is_err(),
            "a JSON-encoded string must be rejected, not reparsed or treated as absent"
        );
    }

    #[test]
    fn conflict_action_defaults_to_error() {
        assert_eq!(conflict_action(&obj(vec![])), ConflictAction::Error);
    }

    #[test]
    fn conflict_action_ignore_from_enum() {
        let o = obj(vec![("onConflict", GqlValue::Enum(Name::new("IGNORE")))]);
        assert_eq!(conflict_action(&o), ConflictAction::Ignore);
    }

    #[test]
    fn conflict_action_ignore_from_string() {
        let o = obj(vec![("onConflict", s("IGNORE"))]);
        assert_eq!(conflict_action(&o), ConflictAction::Ignore);
    }

    #[test]
    fn conflict_action_error_on_other_value() {
        let o = obj(vec![("onConflict", GqlValue::Enum(Name::new("ERROR")))]);
        assert_eq!(conflict_action(&o), ConflictAction::Error);
    }
}
