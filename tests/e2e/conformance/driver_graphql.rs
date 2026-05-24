#[cfg(test)]
mod tests {
    use super::super::fixture::{
        AuthMode, ExpectedBehavior, InterfaceId, SetupExpectation, Step, StepOp, WorkflowFixture,
    };
    use super::super::result::{ConformanceError, ConformanceResult, ConformanceValue};
    use super::*;

    fn fx(steps: Vec<Step>, timeout_ms: u64) -> WorkflowFixture {
        WorkflowFixture {
            id: "test".into(),
            title: "Test".into(),
            setup: SetupExpectation {
                auth_mode: AuthMode::None,
                timeout_ms,
                setup_steps: vec![],
            },
            steps,
            expected: ExpectedBehavior {
                value: None,
                warnings: vec![],
                error: None,
            },
            interfaces: vec![InterfaceId::Graphql],
        }
    }

    #[test]
    fn missing_required_arg_returns_setup_failed() {
        let driver = GraphqlDriver::new();
        let fixture = fx(
            vec![Step {
                op: StepOp::ReadDoogat,
                args: serde_json::json!({}),
            }],
            30_000,
        );
        let results = driver.run_workflow(&fixture);
        assert_eq!(results.len(), 1);
        match &results[0] {
            ConformanceResult::SetupFailed { reason } => {
                assert!(!reason.is_empty(), "reason should be non-empty");
            }
            other => panic!("expected SetupFailed, got: {other:?}"),
        }
    }

    #[test]
    fn create_returns_id_in_value() {
        let driver = GraphqlDriver::new();
        let fixture = fx(
            vec![Step {
                op: StepOp::CreateDoogat,
                args: serde_json::json!({"title": "Test doogat"}),
            }],
            30_000,
        );
        let results = driver.run_workflow(&fixture);
        assert_eq!(results.len(), 1);
        match &results[0] {
            ConformanceResult::Ok { value, warnings: _ } => match value {
                ConformanceValue::String(id) => {
                    assert_eq!(id.len(), 14, "id should be 14 digits, got: {id}");
                    assert!(
                        id.chars().all(|c| c.is_ascii_digit()),
                        "id should be all ASCII digits, got: {id}"
                    );
                }
                other => panic!("expected String(id), got: {other:?}"),
            },
            other => panic!("expected Ok, got: {other:?}"),
        }
    }

    #[test]
    fn create_then_read_round_trip_shows_title() {
        let driver = GraphqlDriver::new();
        let title = "Round trip title";
        let create_step = Step {
            op: StepOp::CreateDoogat,
            args: serde_json::json!({"title": title}),
        };
        let create_fixture = fx(vec![create_step], 30_000);
        let create_results = driver.run_workflow(&create_fixture);
        assert_eq!(create_results.len(), 1);
        let id = match &create_results[0] {
            ConformanceResult::Ok {
                value: ConformanceValue::String(id),
                ..
            } => id.clone(),
            other => panic!("expected Ok(String(id)), got: {other:?}"),
        };

        let read_fixture = fx(
            vec![Step {
                op: StepOp::ReadDoogat,
                args: serde_json::json!({"id": id}),
            }],
            30_000,
        );
        let read_results = driver.run_workflow(&read_fixture);
        assert_eq!(read_results.len(), 1);
        match &read_results[0] {
            ConformanceResult::Ok {
                value: ConformanceValue::String(text),
                ..
            } => {
                assert!(!text.is_empty(), "read text should be non-empty");
                assert!(
                    text.contains(title),
                    "read text should contain title {title}, got: {text}"
                );
            }
            other => panic!("expected Ok(String(text)), got: {other:?}"),
        }
    }

    #[test]
    fn read_nonexistent_returns_err() {
        let driver = GraphqlDriver::new();
        let fixture = fx(
            vec![Step {
                op: StepOp::ReadDoogat,
                args: serde_json::json!({"id": "99999999999999"}),
            }],
            30_000,
        );
        let results = driver.run_workflow(&fixture);
        assert_eq!(results.len(), 1);
        match &results[0] {
            ConformanceResult::Err(ConformanceError { code, .. }) => {
                assert!(!code.is_empty(), "error code should be non-empty");
            }
            other => panic!("expected Err, got: {other:?}"),
        }
    }

    #[test]
    fn create_then_delete_succeeds_with_null_value() {
        let driver = GraphqlDriver::new();
        let create_fixture = fx(
            vec![Step {
                op: StepOp::CreateDoogat,
                args: serde_json::json!({"title": "To delete"}),
            }],
            30_000,
        );
        let create_results = driver.run_workflow(&create_fixture);
        assert_eq!(create_results.len(), 1);
        let id = match &create_results[0] {
            ConformanceResult::Ok {
                value: ConformanceValue::String(id),
                ..
            } => id.clone(),
            other => panic!("expected Ok(String(id)), got: {other:?}"),
        };

        let delete_fixture = fx(
            vec![Step {
                op: StepOp::DeleteDoogat,
                args: serde_json::json!({"id": id}),
            }],
            30_000,
        );
        let delete_results = driver.run_workflow(&delete_fixture);
        assert_eq!(delete_results.len(), 1);
        match &delete_results[0] {
            ConformanceResult::Ok { value, warnings: _ } => match value {
                ConformanceValue::Null => {}
                other => panic!("expected Null, got: {other:?}"),
            },
            other => panic!("expected Ok, got: {other:?}"),
        }
    }

    #[test]
    fn list_after_create_contains_created_title() {
        let driver = GraphqlDriver::new();
        let title = "Listed title";
        let create_fixture = fx(
            vec![Step {
                op: StepOp::CreateDoogat,
                args: serde_json::json!({"title": title}),
            }],
            30_000,
        );
        let create_results = driver.run_workflow(&create_fixture);
        assert_eq!(create_results.len(), 1);
        assert!(
            matches!(&create_results[0], ConformanceResult::Ok { .. }),
            "create should succeed, got: {:?}",
            create_results[0]
        );

        let list_fixture = fx(
            vec![Step {
                op: StepOp::ListDoogats,
                args: serde_json::json!({}),
            }],
            30_000,
        );
        let list_results = driver.run_workflow(&list_fixture);
        assert_eq!(list_results.len(), 1);
        match &list_results[0] {
            ConformanceResult::Ok {
                value: ConformanceValue::String(text),
                ..
            } => {
                assert!(
                    text.contains(title),
                    "list output should contain title {title}, got: {text}"
                );
            }
            other => panic!("expected Ok(String(text)), got: {other:?}"),
        }
    }
}
