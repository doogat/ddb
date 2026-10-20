/// Pull a required string field out of a step's JSON args.
///
/// Returns the field's static name on miss so the caller can craft a
/// `SetupFailed { reason: format!("missing arg: {field}") }`. Used by both
/// `CliDriver` and `GraphqlDriver`.
pub fn require_string(
    args: &serde_json::Value,
    name: &'static str,
) -> Result<String, &'static str> {
    args.get(name)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or(name)
}

/// Pull an optional string field out of a step's JSON args.
pub fn optional_string(args: &serde_json::Value, name: &str) -> Option<String> {
    args.get(name)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

/// Pull an optional list of strings out of a step's JSON args.
///
/// `Ok(None)` when the field is absent or null. A present field that is not
/// an array of strings is a malformed fixture and returns the field's name,
/// so callers report it as a `SetupFailed` instead of silently dropping a
/// filter.
pub fn optional_string_list(
    args: &serde_json::Value,
    name: &'static str,
) -> Result<Option<Vec<String>>, &'static str> {
    match args.get(name) {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::Array(items)) => items
            .iter()
            .map(|v| v.as_str().map(|s| s.to_string()).ok_or(name))
            .collect::<Result<Vec<_>, _>>()
            .map(Some),
        Some(_) => Err(name),
    }
}

/// Pull an optional string-to-string map out of a step's JSON args.
///
/// Same contract as `optional_string_list`: absent/null is `Ok(None)`, any
/// other non-object or non-string value is reported by field name.
pub fn optional_string_map(
    args: &serde_json::Value,
    name: &'static str,
) -> Result<Option<Vec<(String, String)>>, &'static str> {
    match args.get(name) {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::Object(map)) => map
            .iter()
            .map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())).ok_or(name))
            .collect::<Result<Vec<_>, _>>()
            .map(Some),
        Some(_) => Err(name),
    }
}

/// Pull an optional bool field out of a step's JSON args, defaulting to
/// `false` when absent or not a bool (mirrors the server's own `applySchema`
/// resolver default for `dryRun`/`allowDestructive`).
pub fn optional_bool(args: &serde_json::Value, name: &str) -> bool {
    args.get(name).and_then(|v| v.as_bool()).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn require_string_returns_value_when_present() {
        let args = json!({"title": "T"});
        assert_eq!(require_string(&args, "title"), Ok("T".to_string()));
    }

    #[test]
    fn require_string_returns_field_name_on_miss() {
        let args = json!({"title": "T"});
        assert_eq!(require_string(&args, "body"), Err("body"));
    }

    #[test]
    fn require_string_returns_field_name_when_not_a_string() {
        let args = json!({"count": 42});
        assert_eq!(require_string(&args, "count"), Err("count"));
    }

    #[test]
    fn optional_string_returns_some_when_present() {
        let args = json!({"body": "hello"});
        assert_eq!(optional_string(&args, "body"), Some("hello".to_string()));
    }

    #[test]
    fn optional_string_returns_none_when_absent() {
        let args = json!({"title": "T"});
        assert_eq!(optional_string(&args, "body"), None);
    }

    #[test]
    fn optional_string_returns_none_when_not_a_string() {
        let args = json!({"count": 42});
        assert_eq!(optional_string(&args, "count"), None);
    }

    #[test]
    fn optional_string_list_absent_or_null_is_none() {
        assert_eq!(optional_string_list(&json!({}), "types"), Ok(None));
        assert_eq!(
            optional_string_list(&json!({"types": null}), "types"),
            Ok(None)
        );
    }

    #[test]
    fn optional_string_list_returns_strings_in_order() {
        let args = json!({"types": ["recipe", "dessert"]});
        assert_eq!(
            optional_string_list(&args, "types"),
            Ok(Some(vec!["recipe".to_string(), "dessert".to_string()]))
        );
    }

    #[test]
    fn optional_string_list_rejects_non_string_items_and_non_arrays() {
        assert_eq!(
            optional_string_list(&json!({"types": ["ok", 1]}), "types"),
            Err("types")
        );
        assert_eq!(
            optional_string_list(&json!({"types": "recipe"}), "types"),
            Err("types")
        );
    }

    #[test]
    fn optional_string_map_returns_pairs_and_rejects_non_string_values() {
        assert_eq!(optional_string_map(&json!({}), "fields"), Ok(None));
        assert_eq!(
            optional_string_map(&json!({"fields": {"cuisine": "thai"}}), "fields"),
            Ok(Some(vec![("cuisine".to_string(), "thai".to_string())]))
        );
        assert_eq!(
            optional_string_map(&json!({"fields": {"n": 1}}), "fields"),
            Err("fields")
        );
    }

    #[test]
    fn optional_bool_returns_value_or_false_default() {
        assert!(optional_bool(&json!({"dry_run": true}), "dry_run"));
        assert!(!optional_bool(&json!({"dry_run": false}), "dry_run"));
        assert!(!optional_bool(&json!({}), "dry_run"));
        assert!(!optional_bool(&json!({"dry_run": "yes"}), "dry_run"));
    }
}
