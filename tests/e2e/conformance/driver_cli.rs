// CLI conformance driver.
//
// Ivan: define `pub struct CliDriver` and its `new()` + `run_workflow()`
// methods ABOVE the `#[cfg(test)] mod tests` block below. The tests in
// `mod tests` pin the contract; no implementation lives in this file yet.

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::fixture::{
        AuthMode, ExpectedBehavior, InterfaceId, SetupExpectation, Step, StepOp, WorkflowFixture,
    };
    use super::super::result::{ConformanceError, ConformanceResult, ConformanceValue};

    /// Build a minimal one-step fixture for driver tests.
    fn fx(step: Step, timeout_ms: u64) -> WorkflowFixture {
        WorkflowFixture {
            id: "test".into(),
            title: "Test".into(),
            setup: SetupExpectation {
                auth_mode: AuthMode::None,
                timeout_ms,
                setup_steps: vec![],
            },
            steps: vec![step],
            expected: ExpectedBehavior {
                value: None,
                warnings: vec![],
                error: None,
            },
            interfaces: vec![InterfaceId::Cli],
        }
    }

    #[test]
    fn cli_driver_constructs_with_fresh_repo() {
        let _driver = CliDriver::new();
    }

    #[test]
    fn missing_required_arg_returns_setup_failed() {
        let driver = CliDriver::new();
        let fixture = fx(
            Step {
                op: StepOp::ReadDoogat,
                args: serde_json::json!({}),
            },
            30_000,
        );
        let results = driver.run_workflow(&fixture);
        assert_eq!(results.len(), 1);
        match &results[0] {
            ConformanceResult::SetupFailed { reason } => {
                assert!(
                    reason.contains("missing arg"),
                    "expected reason to contain 'missing arg', got: {reason}"
                );
            }
            other => panic!("expected SetupFailed, got: {other:?}"),
        }
    }

    #[test]
    fn create_doogat_returns_id_in_value() {
        let driver = CliDriver::new();
        let fixture = fx(
            Step {
                op: StepOp::CreateDoogat,
                args: serde_json::json!({"title": "Test doogat"}),
            },
            30_000,
        );
        let results = driver.run_workflow(&fixture);
        assert_eq!(results.len(), 1);
        match &results[0] {
            ConformanceResult::Ok { value, warnings: _ } => match value {
                ConformanceValue::String(id) => {
                    assert_eq!(
                        id.len(),
                        14,
                        "expected 14-digit id, got {} chars: {id}",
                        id.len()
                    );
                    assert!(
                        id.chars().all(|c| c.is_ascii_digit()),
                        "expected all-digit id, got: {id}"
                    );
                }
                other => panic!("expected ConformanceValue::String(id), got: {other:?}"),
            },
            other => panic!("expected Ok, got: {other:?}"),
        }
    }

    #[test]
    fn create_then_read_round_trips() {
        let driver = CliDriver::new();

        // Step 1: create
        let create_fixture = fx(
            Step {
                op: StepOp::CreateDoogat,
                args: serde_json::json!({"title": "RT"}),
            },
            30_000,
        );
        let create_results = driver.run_workflow(&create_fixture);
        assert_eq!(create_results.len(), 1);
        let id = match &create_results[0] {
            ConformanceResult::Ok {
                value: ConformanceValue::String(id),
                ..
            } => id.clone(),
            other => panic!("expected Ok(String(id)) from create, got: {other:?}"),
        };

        // Step 2: read the id we just created
        let read_fixture = fx(
            Step {
                op: StepOp::ReadDoogat,
                args: serde_json::json!({"id": id}),
            },
            30_000,
        );
        let read_results = driver.run_workflow(&read_fixture);
        assert_eq!(read_results.len(), 1);
        match &read_results[0] {
            ConformanceResult::Ok {
                value: ConformanceValue::String(text),
                ..
            } => {
                assert!(
                    text.contains("RT"),
                    "expected read output to contain title 'RT', got: {text}"
                );
            }
            other => panic!("expected Ok(String(text)) from read, got: {other:?}"),
        }
    }

    #[test]
    fn read_nonexistent_returns_err() {
        let driver = CliDriver::new();
        let fixture = fx(
            Step {
                op: StepOp::ReadDoogat,
                args: serde_json::json!({"id": "99999999999999"}),
            },
            30_000,
        );
        let results = driver.run_workflow(&fixture);
        assert_eq!(results.len(), 1);
        match &results[0] {
            ConformanceResult::Err(ConformanceError { code, .. }) => {
                assert_eq!(code, "CLI_ERROR", "expected CLI_ERROR code, got: {code}");
            }
            other => panic!("expected Err(ConformanceError), got: {other:?}"),
        }
    }

    // TODO: Ivan — if this test is flaky on very fast machines, mark `#[ignore]`
    // and document the timing assumption. 1ms is intentionally tight so that
    // even a no-op `ddb list` against a fresh repo cannot complete in time.
    #[test]
    fn timeout_yields_setup_failed() {
        let driver = CliDriver::new();
        let fixture = fx(
            Step {
                op: StepOp::ListDoogats,
                args: serde_json::json!({}),
            },
            1,
        );
        let results = driver.run_workflow(&fixture);
        assert_eq!(results.len(), 1);
        match &results[0] {
            ConformanceResult::SetupFailed { reason } => {
                assert!(
                    reason.contains("timeout"),
                    "expected reason to contain 'timeout', got: {reason}"
                );
            }
            other => panic!("expected SetupFailed, got: {other:?}"),
        }
    }
}
