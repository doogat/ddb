// Tess: this module's `pub fn resolve_refs` is unimplemented yet — Ivan will add
// it next. Tests below must compile and FAIL (RED phase of TDD).

use super::result::{ConformanceError, ConformanceResult, ConformanceValue, ConformanceWarning};

pub fn resolve_refs(_args: &serde_json::Value, _prior: &[ConformanceResult]) -> serde_json::Value {
    unimplemented!("Ivan implements this")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    fn ok_string(s: &str) -> ConformanceResult {
        ConformanceResult::Ok {
            value: ConformanceValue::String(s.into()),
            warnings: vec![],
        }
    }

    fn ok_null() -> ConformanceResult {
        ConformanceResult::Ok {
            value: ConformanceValue::Null,
            warnings: vec![],
        }
    }

    fn err_result() -> ConformanceResult {
        ConformanceResult::Err(ConformanceError {
            code: "X".into(),
            message: "boom".into(),
            context: serde_json::Map::new(),
        })
    }

    #[test]
    fn replaces_top_level_dollar_0_id_string_with_prior_ok_string() {
        let args = Value::String("$0.id".into());
        let prior = vec![ok_string("ID123")];
        let result = resolve_refs(&args, &prior);
        assert_eq!(result, Value::String("ID123".into()));
    }

    #[test]
    fn leaves_unrelated_strings_alone() {
        let args = Value::String("hello".into());
        let prior = vec![ok_string("ID123")];
        let result = resolve_refs(&args, &prior);
        assert_eq!(result, Value::String("hello".into()));
    }

    #[test]
    fn recurses_into_object_values() {
        let args = json!({"id": "$0.id", "title": "T"});
        let prior = vec![ok_string("ID123")];
        let result = resolve_refs(&args, &prior);
        assert_eq!(result, json!({"id": "ID123", "title": "T"}));
    }

    #[test]
    fn recurses_into_array_elements() {
        let args = json!(["$0.id", "static"]);
        let prior = vec![ok_string("ID123")];
        let result = resolve_refs(&args, &prior);
        assert_eq!(result, json!(["ID123", "static"]));
    }

    #[test]
    fn recurses_into_nested_object_inside_array_inside_object() {
        let args = json!({
            "outer": [
                {"inner": "$0.id"},
                "unchanged"
            ]
        });
        let prior = vec![ok_string("DEEP")];
        let result = resolve_refs(&args, &prior);
        assert_eq!(
            result,
            json!({
                "outer": [
                    {"inner": "DEEP"},
                    "unchanged"
                ]
            })
        );
    }

    #[test]
    fn index_out_of_bounds_returns_string_unchanged() {
        let args = Value::String("$5.id".into());
        let prior = vec![ok_string("A"), ok_string("B")];
        let result = resolve_refs(&args, &prior);
        assert_eq!(result, Value::String("$5.id".into()));
    }

    #[test]
    fn prior_err_returns_string_unchanged() {
        let args = Value::String("$0.id".into());
        let prior = vec![err_result()];
        let result = resolve_refs(&args, &prior);
        assert_eq!(result, Value::String("$0.id".into()));
    }

    #[test]
    fn prior_unsupported_returns_string_unchanged() {
        let args = Value::String("$0.id".into());
        let prior = vec![ConformanceResult::Unsupported {
            reason: "no".into(),
        }];
        let result = resolve_refs(&args, &prior);
        assert_eq!(result, Value::String("$0.id".into()));
    }

    #[test]
    fn prior_setup_failed_returns_string_unchanged() {
        let args = Value::String("$0.id".into());
        let prior = vec![ConformanceResult::SetupFailed {
            reason: "no".into(),
        }];
        let result = resolve_refs(&args, &prior);
        assert_eq!(result, Value::String("$0.id".into()));
    }

    #[test]
    fn prior_ok_with_non_string_value_returns_string_unchanged() {
        let args = Value::String("$0.id".into());
        let prior = vec![ok_null()];
        let result = resolve_refs(&args, &prior);
        assert_eq!(result, Value::String("$0.id".into()));
    }

    #[test]
    fn string_with_dot_id_but_no_dollar_returns_unchanged() {
        let args = Value::String("0.id".into());
        let prior = vec![ok_string("ID123")];
        let result = resolve_refs(&args, &prior);
        assert_eq!(result, Value::String("0.id".into()));
    }

    #[test]
    fn string_with_dollar_but_no_dot_id_returns_unchanged() {
        let args = Value::String("$0".into());
        let prior = vec![ok_string("ID123")];
        let result = resolve_refs(&args, &prior);
        assert_eq!(result, Value::String("$0".into()));
    }

    #[test]
    fn string_with_dollar_id_but_trailing_space_returns_unchanged() {
        let args = Value::String("$0.id ".into());
        let prior = vec![ok_string("ID123")];
        let result = resolve_refs(&args, &prior);
        assert_eq!(result, Value::String("$0.id ".into()));
    }

    #[test]
    fn string_with_dollar_id_but_extra_suffix_returns_unchanged() {
        let args = Value::String("$0.id.extra".into());
        let prior = vec![ok_string("ID123")];
        let result = resolve_refs(&args, &prior);
        assert_eq!(result, Value::String("$0.id.extra".into()));
    }

    #[test]
    fn multi_digit_index_resolves_when_prior_has_enough_entries() {
        let mut prior: Vec<ConformanceResult> =
            (0..10).map(|i| ok_string(&format!("S{i}"))).collect();
        prior.push(ok_string("X"));
        assert_eq!(prior.len(), 11);
        let args = Value::String("$10.id".into());
        let result = resolve_refs(&args, &prior);
        assert_eq!(result, Value::String("X".into()));
    }

    #[test]
    fn non_string_integer_returned_as_is() {
        let args = json!(42);
        let prior: Vec<ConformanceResult> = vec![];
        let result = resolve_refs(&args, &prior);
        assert_eq!(result, json!(42));
    }

    #[test]
    fn non_string_null_returned_as_is() {
        let args = Value::Null;
        let prior: Vec<ConformanceResult> = vec![];
        let result = resolve_refs(&args, &prior);
        assert_eq!(result, Value::Null);
    }

    #[test]
    fn empty_prior_with_dollar_zero_id_returns_unchanged() {
        let args = Value::String("$0.id".into());
        let prior: &[ConformanceResult] = &[];
        let result = resolve_refs(&args, prior);
        assert_eq!(result, Value::String("$0.id".into()));
    }

    #[test]
    fn string_with_non_numeric_index_returns_unchanged() {
        let args = Value::String("$abc.id".into());
        let prior = vec![ok_string("ID123")];
        let result = resolve_refs(&args, &prior);
        assert_eq!(result, Value::String("$abc.id".into()));
    }

    #[test]
    fn string_matching_dollar_name_field_returns_unchanged() {
        let args = Value::String("$0.name".into());
        let prior = vec![ok_string("ID123")];
        let result = resolve_refs(&args, &prior);
        assert_eq!(result, Value::String("$0.name".into()));
    }

    #[test]
    fn string_with_bare_dot_id_dollar_returns_unchanged() {
        // "$.id" — no digits between $ and .id
        let args = Value::String("$.id".into());
        let prior = vec![ok_string("ID123")];
        let result = resolve_refs(&args, &prior);
        assert_eq!(result, Value::String("$.id".into()));
    }

    #[test]
    fn string_with_prefix_before_dollar_returns_unchanged() {
        let args = Value::String("prefix$0.id".into());
        let prior = vec![ok_string("ID123")];
        let result = resolve_refs(&args, &prior);
        assert_eq!(result, Value::String("prefix$0.id".into()));
    }

    #[test]
    fn prior_ok_with_int_value_returns_string_unchanged() {
        let args = Value::String("$0.id".into());
        let prior = vec![ConformanceResult::Ok {
            value: ConformanceValue::Int(99),
            warnings: vec![],
        }];
        let result = resolve_refs(&args, &prior);
        assert_eq!(result, Value::String("$0.id".into()));
    }

    #[test]
    fn prior_ok_with_warning_still_resolves_string() {
        let args = Value::String("$0.id".into());
        let prior = vec![ConformanceResult::Ok {
            value: ConformanceValue::String("ID456".into()),
            warnings: vec![ConformanceWarning {
                code: "WARN".into(),
                message: "minor warning".into(),
            }],
        }];
        let result = resolve_refs(&args, &prior);
        assert_eq!(result, Value::String("ID456".into()));
    }
}
