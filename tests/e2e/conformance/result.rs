#[derive(Debug, Clone, PartialEq)]
pub enum ConformanceResult {
    Ok {
        value: ConformanceValue,
        warnings: Vec<ConformanceWarning>,
    },
    Err(ConformanceError),
    Unsupported {
        reason: String,
    },
    SetupFailed {
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum ConformanceValue {
    Null,
    Bool(bool),
    Int(i64),
    #[expect(dead_code, reason = "reserved for future drivers")]
    Float(f64),
    String(String),
    Array(Vec<ConformanceValue>),
    Object(std::collections::BTreeMap<String, ConformanceValue>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConformanceWarning {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConformanceError {
    pub code: String,
    pub message: String,
    pub context: serde_json::Map<String, serde_json::Value>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn ok_variant_round_trips_value_and_warnings() {
        let original = ConformanceResult::Ok {
            value: ConformanceValue::Int(42),
            warnings: vec![ConformanceWarning {
                code: "TITLE_FROM_TEMPLATE".into(),
                message: "title was templated".into(),
            }],
        };
        let cloned = original.clone();
        assert_eq!(cloned, original);
    }

    #[test]
    fn err_variant_carries_full_error_envelope() {
        let result = ConformanceResult::Err(ConformanceError {
            code: "UNIQUE_VIOLATION".into(),
            message: "row exists".into(),
            context: serde_json::Map::from_iter([(
                "existing_id".to_string(),
                serde_json::Value::String("20260523120000".into()),
            )]),
        });
        let debug = format!("{:?}", result);
        assert!(debug.contains("UNIQUE_VIOLATION"));
        assert!(debug.contains("existing_id"));
    }

    #[test]
    fn unsupported_variant_carries_reason() {
        let result = ConformanceResult::Unsupported {
            reason: "PgWire has no search()".into(),
        };
        let debug = format!("{:?}", result);
        assert!(debug.contains("PgWire has no search()"));
    }

    #[test]
    fn setup_failed_variant_carries_reason() {
        let result = ConformanceResult::SetupFailed {
            reason: "ddb binary not found".into(),
        };
        let debug = format!("{:?}", result);
        assert!(debug.contains("ddb binary not found"));
    }

    #[test]
    fn conformance_value_object_compares_structurally() {
        let mut a = BTreeMap::new();
        a.insert("alpha".to_string(), ConformanceValue::Int(1));
        a.insert("beta".to_string(), ConformanceValue::String("two".into()));

        let mut b = BTreeMap::new();
        b.insert("beta".to_string(), ConformanceValue::String("two".into()));
        b.insert("alpha".to_string(), ConformanceValue::Int(1));

        assert_eq!(ConformanceValue::Object(a), ConformanceValue::Object(b));
    }

    #[test]
    fn conformance_value_array_of_mixed_types_compares() {
        let a = ConformanceValue::Array(vec![
            ConformanceValue::Null,
            ConformanceValue::Bool(true),
            ConformanceValue::Int(7),
            ConformanceValue::String("x".into()),
        ]);
        let b = ConformanceValue::Array(vec![
            ConformanceValue::Null,
            ConformanceValue::Bool(true),
            ConformanceValue::Int(7),
            ConformanceValue::String("x".into()),
        ]);
        assert_eq!(a, b);
    }

    #[test]
    fn conformance_warning_eq_compares_code_and_message() {
        let base = ConformanceWarning {
            code: "W1".into(),
            message: "first".into(),
        };
        let same = ConformanceWarning {
            code: "W1".into(),
            message: "first".into(),
        };
        let diff_code = ConformanceWarning {
            code: "W2".into(),
            message: "first".into(),
        };
        assert_eq!(base, same);
        assert_ne!(base, diff_code);
    }

    #[test]
    fn conformance_error_eq_compares_all_three_fields() {
        let a = ConformanceError {
            code: "E1".into(),
            message: "boom".into(),
            context: serde_json::Map::from_iter([(
                "k".to_string(),
                serde_json::Value::String("v".into()),
            )]),
        };
        let b = ConformanceError {
            code: "E1".into(),
            message: "boom".into(),
            context: serde_json::Map::from_iter([(
                "k".to_string(),
                serde_json::Value::String("v".into()),
            )]),
        };
        let c = ConformanceError {
            code: "E1".into(),
            message: "boom".into(),
            context: serde_json::Map::from_iter([(
                "k".to_string(),
                serde_json::Value::String("different".into()),
            )]),
        };
        assert_eq!(a, b);
        assert_ne!(a, c);
    }
}
