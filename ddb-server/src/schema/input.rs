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

    #[test]
    fn fields_map_parses_json() {
        let o = obj(vec![("fields", s(r#"{"author":"alice"}"#))]);
        let m = fields_map(&o).unwrap();
        assert_eq!(
            m.get("author"),
            Some(&DdbValue::String("alice".to_string()))
        );
    }

    #[test]
    fn fields_map_empty_when_absent() {
        assert!(fields_map(&obj(vec![])).unwrap().is_empty());
    }

    #[test]
    fn fields_map_errors_on_bad_json() {
        let o = obj(vec![("fields", s("not json"))]);
        assert!(fields_map(&o).is_err());
    }

    #[test]
    fn opt_fields_map_none_when_absent() {
        assert_eq!(opt_fields_map(&obj(vec![])).unwrap(), None);
    }

    #[test]
    fn opt_fields_map_some_when_present() {
        let o = obj(vec![("fields", s(r#"{"x":"1"}"#))]);
        assert_eq!(
            opt_fields_map(&o).unwrap(),
            Some(BTreeMap::from([(
                "x".to_string(),
                DdbValue::String("1".to_string())
            )]))
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
