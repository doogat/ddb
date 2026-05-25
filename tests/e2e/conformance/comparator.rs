use super::result::ConformanceResult;

/// Classified difference between two driver results. Maps to the six
/// categories PRD 00148 lists for the "Difference classifier" capability,
/// plus `Match` (identical) and `VariantMismatch` (catch-all for incompatible
/// variant pairs like `Ok` vs `Err`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffClass {
    /// `ConformanceResult` PartialEq holds — results are identical.
    Match,
    /// Both `Ok`, but `value` fields differ (warnings may also differ; the
    /// value diff dominates the classification).
    ValueMismatch,
    /// Both `Ok`, `value` matches, but `warnings` differ.
    WarningMismatch,
    /// Both `Err`, but the `ConformanceError` shapes (code/message/context)
    /// differ.
    ErrorMismatch,
    /// At least one driver returned `Unsupported`. Either both are
    /// `Unsupported` with different reasons, or one driver said the operation
    /// was unsupported while the other did not.
    UnsupportedOperation,
    /// At least one driver returned `SetupFailed`. Either both reported a
    /// setup failure with different reasons, or one driver failed setup while
    /// the other ran the operation.
    SetupFailure,
    /// `compare_per_step` only: one slice ran out of results before the
    /// other. The unmatched trailing entries are reported as `MissingField`
    /// so a length divergence between drivers is not silently dropped.
    MissingField,
    /// Catch-all for variant pairs not covered by the more specific
    /// categories above (most commonly `Ok` vs `Err`).
    VariantMismatch,
}

pub fn compare(left: &ConformanceResult, right: &ConformanceResult) -> DiffClass {
    if left == right {
        return DiffClass::Match;
    }
    match (left, right) {
        (
            ConformanceResult::Ok {
                value: lv,
                warnings: lw,
            },
            ConformanceResult::Ok {
                value: rv,
                warnings: rw,
            },
        ) => {
            if lv != rv {
                DiffClass::ValueMismatch
            } else if lw != rw {
                DiffClass::WarningMismatch
            } else {
                // PartialEq above was false yet all fields match — unreachable
                // in practice; classify defensively as Match.
                DiffClass::Match
            }
        }
        (ConformanceResult::Err(_), ConformanceResult::Err(_)) => DiffClass::ErrorMismatch,
        (ConformanceResult::Unsupported { .. }, _) | (_, ConformanceResult::Unsupported { .. }) => {
            DiffClass::UnsupportedOperation
        }
        (ConformanceResult::SetupFailed { .. }, _) | (_, ConformanceResult::SetupFailed { .. }) => {
            DiffClass::SetupFailure
        }
        // Everything else is an incompatible variant pair (e.g. Ok vs Err).
        _ => DiffClass::VariantMismatch,
    }
}

pub fn compare_per_step(
    left: &[ConformanceResult],
    right: &[ConformanceResult],
) -> Vec<DiffClass> {
    let max_len = left.len().max(right.len());
    let mut result = Vec::with_capacity(max_len);
    for i in 0..max_len {
        match (left.get(i), right.get(i)) {
            (Some(l), Some(r)) => result.push(compare(l, r)),
            // One slice ran out of results — surface the divergence rather
            // than silently truncating (PRD 00148 cycle-2 F9).
            (Some(_), None) | (None, Some(_)) => result.push(DiffClass::MissingField),
            (None, None) => unreachable!("max_len bounds prevent both being None"),
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::super::result::{
        ConformanceError, ConformanceResult, ConformanceValue, ConformanceWarning,
    };
    use super::*;

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

    // --- compare: ValueMismatch / WarningMismatch ---

    #[test]
    fn value_mismatch_when_both_ok_with_different_values() {
        let a = ConformanceResult::Ok {
            value: ConformanceValue::Int(1),
            warnings: vec![],
        };
        let b = ConformanceResult::Ok {
            value: ConformanceValue::Int(2),
            warnings: vec![],
        };
        assert_eq!(compare(&a, &b), DiffClass::ValueMismatch);
    }

    #[test]
    fn warning_mismatch_when_value_matches_but_warnings_differ() {
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
        assert_eq!(compare(&a, &b), DiffClass::WarningMismatch);
    }

    // --- compare: ErrorMismatch ---

    #[test]
    fn error_mismatch_when_codes_differ() {
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
        assert_eq!(compare(&a, &b), DiffClass::ErrorMismatch);
    }

    #[test]
    fn error_mismatch_when_context_differs() {
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
        assert_eq!(compare(&a, &b), DiffClass::ErrorMismatch);
    }

    // --- compare: UnsupportedOperation ---

    #[test]
    fn unsupported_operation_when_both_unsupported_with_different_reasons() {
        let a = ConformanceResult::Unsupported {
            reason: "reason A".into(),
        };
        let b = ConformanceResult::Unsupported {
            reason: "reason B".into(),
        };
        assert_eq!(compare(&a, &b), DiffClass::UnsupportedOperation);
    }

    #[test]
    fn unsupported_operation_when_only_one_side_unsupported() {
        let a = ConformanceResult::Ok {
            value: ConformanceValue::Bool(true),
            warnings: vec![],
        };
        let b = ConformanceResult::Unsupported {
            reason: "n/a".into(),
        };
        assert_eq!(compare(&a, &b), DiffClass::UnsupportedOperation);
    }

    // --- compare: SetupFailure ---

    #[test]
    fn setup_failure_when_both_setup_failed_with_different_reasons() {
        let a = ConformanceResult::SetupFailed {
            reason: "timeout".into(),
        };
        let b = ConformanceResult::SetupFailed {
            reason: "missing env".into(),
        };
        assert_eq!(compare(&a, &b), DiffClass::SetupFailure);
    }

    #[test]
    fn setup_failure_when_only_one_side_setup_failed() {
        let a = ConformanceResult::Ok {
            value: ConformanceValue::Null,
            warnings: vec![],
        };
        let b = ConformanceResult::SetupFailed {
            reason: "crash".into(),
        };
        assert_eq!(compare(&a, &b), DiffClass::SetupFailure);
    }

    // --- compare: VariantMismatch catch-all ---

    #[test]
    fn variant_mismatch_when_ok_vs_err() {
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
    fn per_step_left_longer_reports_missing_field_for_extras() {
        let ok = ConformanceResult::Ok {
            value: ConformanceValue::Null,
            warnings: vec![],
        };
        let left = vec![ok.clone(), ok.clone(), ok.clone()];
        let right = vec![ok.clone()];
        let result = compare_per_step(&left, &right);
        assert_eq!(
            result,
            vec![DiffClass::Match, DiffClass::MissingField, DiffClass::MissingField]
        );
    }

    #[test]
    fn per_step_right_longer_reports_missing_field_for_extras() {
        let ok = ConformanceResult::Ok {
            value: ConformanceValue::Null,
            warnings: vec![],
        };
        let left = vec![ok.clone()];
        let right = vec![ok.clone(), ok.clone()];
        let result = compare_per_step(&left, &right);
        assert_eq!(result, vec![DiffClass::Match, DiffClass::MissingField]);
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
        assert_eq!(result, vec![DiffClass::Match, DiffClass::ValueMismatch]);

        let left2 = vec![ok_a.clone()];
        let right2 = vec![err.clone()];
        let result2 = compare_per_step(&left2, &right2);
        assert_eq!(result2, vec![DiffClass::VariantMismatch]);
    }
}
