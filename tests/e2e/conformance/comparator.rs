#[cfg(test)]
mod tests {
    use super::*;
    use super::super::result::{
        ConformanceError, ConformanceResult, ConformanceValue, ConformanceWarning,
    };

    // --- compare: DiffClass::Match ---

    #[test]
    fn match_when_both_ok_identical() {
        let a = ConformanceResult::Ok {
            value: ConformanceValue::Int(42),
            warnings: vec![],
        };
        let b = a.clone();
        assert_eq!(compare(&a, &b), DiffClass::Match);
    }

    #[test]
    fn match_when_both_ok_with_warnings_identical() {
        let a = ConformanceResult::Ok {
            value: ConformanceValue::String("hello".into()),
            warnings: vec![ConformanceWarning {
                code: "W001".into(),
                message: "deprecated".into(),
            }],
        };
        let b = a.clone();
        assert_eq!(compare(&a, &b), DiffClass::Match);
    }

    #[test]
    fn match_when_both_err_identical() {
        let a = ConformanceResult::Err(ConformanceError {
            code: "E001".into(),
            message: "not found".into(),
            context: serde_json::Map::new(),
        });
        let b = a.clone();
        assert_eq!(compare(&a, &b), DiffClass::Match);
    }

    #[test]
    fn match_when_both_unsupported_identical() {
        let a = ConformanceResult::Unsupported {
            reason: "not implemented".into(),
        };
        let b = a.clone();
        assert_eq!(compare(&a, &b), DiffClass::Match);
    }

    #[test]
    fn match_when_both_setup_failed_identical() {
        let a = ConformanceResult::SetupFailed {
            reason: "db unavailable".into(),
        };
        let b = a.clone();
        assert_eq!(compare(&a, &b), DiffClass::Match);
    }

    // --- compare: DiffClass::VariantMismatch ---

    #[test]
    fn variant_mismatch_ok_vs_err() {
        let a = ConformanceResult::Ok {
            value: ConformanceValue::Null,
            warnings: vec![],
        };
        let b = ConformanceResult::Err(ConformanceError {
            code: "E001".into(),
            message: "fail".into(),
            context: serde_json::Map::new(),
        });
        assert_eq!(compare(&a, &b), DiffClass::VariantMismatch);
    }

    #[test]
    fn variant_mismatch_ok_vs_unsupported() {
        let a = ConformanceResult::Ok {
            value: ConformanceValue::Bool(true),
            warnings: vec![],
        };
        let b = ConformanceResult::Unsupported {
            reason: "n/a".into(),
        };
        assert_eq!(compare(&a, &b), DiffClass::VariantMismatch);
    }

    #[test]
    fn variant_mismatch_ok_vs_setup_failed() {
        let a = ConformanceResult::Ok {
            value: ConformanceValue::Null,
            warnings: vec![],
        };
        let b = ConformanceResult::SetupFailed {
            reason: "crash".into(),
        };
        assert_eq!(compare(&a, &b), DiffClass::VariantMismatch);
    }

    #[test]
    fn variant_mismatch_err_vs_unsupported() {
        let a = ConformanceResult::Err(ConformanceError {
            code: "E002".into(),
            message: "oops".into(),
            context: serde_json::Map::new(),
        });
        let b = ConformanceResult::Unsupported {
            reason: "not supported".into(),
        };
        assert_eq!(compare(&a, &b), DiffClass::VariantMismatch);
    }

    #[test]
    fn variant_mismatch_err_vs_setup_failed() {
        let a = ConformanceResult::Err(ConformanceError {
            code: "E003".into(),
            message: "error".into(),
            context: serde_json::Map::new(),
        });
        let b = ConformanceResult::SetupFailed {
            reason: "setup broke".into(),
        };
        assert_eq!(compare(&a, &b), DiffClass::VariantMismatch);
    }

    #[test]
    fn variant_mismatch_unsupported_vs_setup_failed() {
        let a = ConformanceResult::Unsupported {
            reason: "no driver".into(),
        };
        let b = ConformanceResult::SetupFailed {
            reason: "init failed".into(),
        };
        assert_eq!(compare(&a, &b), DiffClass::VariantMismatch);
    }

    // --- compare: DiffClass::ContentDiff ---

    #[test]
    fn content_diff_ok_different_value() {
        let a = ConformanceResult::Ok {
            value: ConformanceValue::Int(1),
            warnings: vec![],
        };
        let b = ConformanceResult::Ok {
            value: ConformanceValue::Int(2),
            warnings: vec![],
        };
        assert_eq!(compare(&a, &b), DiffClass::ContentDiff);
    }

    #[test]
    fn content_diff_ok_different_warnings() {
        let a = ConformanceResult::Ok {
            value: ConformanceValue::Null,
            warnings: vec![],
        };
        let b = ConformanceResult::Ok {
            value: ConformanceValue::Null,
            warnings: vec![ConformanceWarning {
                code: "W001".into(),
                message: "something deprecated".into(),
            }],
        };
        assert_eq!(compare(&a, &b), DiffClass::ContentDiff);
    }

    #[test]
    fn content_diff_err_different_code() {
        let a = ConformanceResult::Err(ConformanceError {
            code: "E001".into(),
            message: "same message".into(),
            context: serde_json::Map::new(),
        });
        let b = ConformanceResult::Err(ConformanceError {
            code: "E002".into(),
            message: "same message".into(),
            context: serde_json::Map::new(),
        });
        assert_eq!(compare(&a, &b), DiffClass::ContentDiff);
    }

    #[test]
    fn content_diff_err_different_context() {
        let mut ctx = serde_json::Map::new();
        ctx.insert("key".into(), serde_json::Value::Bool(true));
        let a = ConformanceResult::Err(ConformanceError {
            code: "E001".into(),
            message: "same".into(),
            context: serde_json::Map::new(),
        });
        let b = ConformanceResult::Err(ConformanceError {
            code: "E001".into(),
            message: "same".into(),
            context: ctx,
        });
        assert_eq!(compare(&a, &b), DiffClass::ContentDiff);
    }

    #[test]
    fn content_diff_unsupported_different_reason() {
        let a = ConformanceResult::Unsupported {
            reason: "reason A".into(),
        };
        let b = ConformanceResult::Unsupported {
            reason: "reason B".into(),
        };
        assert_eq!(compare(&a, &b), DiffClass::ContentDiff);
    }

    #[test]
    fn content_diff_setup_failed_different_reason() {
        let a = ConformanceResult::SetupFailed {
            reason: "timeout".into(),
        };
        let b = ConformanceResult::SetupFailed {
            reason: "missing env".into(),
        };
        assert_eq!(compare(&a, &b), DiffClass::ContentDiff);
    }

    // --- compare_per_step ---

    #[test]
    fn per_step_empty_slices_returns_empty_vec() {
        let result = compare_per_step(&[], &[]);
        assert_eq!(result, vec![]);
    }

    #[test]
    fn per_step_single_equal_pair_returns_match() {
        let a = ConformanceResult::Ok {
            value: ConformanceValue::Int(7),
            warnings: vec![],
        };
        let b = a.clone();
        assert_eq!(compare_per_step(&[a], &[b]), vec![DiffClass::Match]);
    }

    #[test]
    fn per_step_stops_at_shorter_slice() {
        let ok = ConformanceResult::Ok {
            value: ConformanceValue::Null,
            warnings: vec![],
        };
        let left = vec![ok.clone(), ok.clone(), ok.clone()];
        let right = vec![ok.clone()];
        let result = compare_per_step(&left, &right);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0], DiffClass::Match);
    }

    #[test]
    fn per_step_classifies_each_pair_independently() {
        let ok_a = ConformanceResult::Ok {
            value: ConformanceValue::Int(1),
            warnings: vec![],
        };
        let ok_b = ConformanceResult::Ok {
            value: ConformanceValue::Int(2),
            warnings: vec![],
        };
        let err = ConformanceResult::Err(ConformanceError {
            code: "E001".into(),
            message: "fail".into(),
            context: serde_json::Map::new(),
        });

        let left = vec![ok_a.clone(), ok_a.clone()];
        let right = vec![ok_a.clone(), ok_b.clone()];
        let result = compare_per_step(&left, &right);
        assert_eq!(result, vec![DiffClass::Match, DiffClass::ContentDiff]);

        let left2 = vec![ok_a.clone()];
        let right2 = vec![err.clone()];
        let result2 = compare_per_step(&left2, &right2);
        assert_eq!(result2, vec![DiffClass::VariantMismatch]);
    }
}
