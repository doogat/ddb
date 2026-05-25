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
}
