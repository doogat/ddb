//! CliDriver tests, split out from `driver_cli.rs` to keep the production
//! module under the 500-line file limit (PRD 00148 cycle-2 F2).

use super::driver_cli::*;
use super::fixture::{
    AuthMode, ExpectedBehavior, InterfaceId, SetupExpectation, Step, StepOp, WorkflowFixture,
};
use super::result::{ConformanceError, ConformanceResult, ConformanceValue};

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

/// Build a minimal multi-step fixture for driver tests.
fn fx_multi(steps: Vec<Step>, timeout_ms: u64) -> WorkflowFixture {
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
        interfaces: vec![InterfaceId::Cli],
    }
}

#[test]
fn resolves_dollar_zero_id_in_second_step_arg() {
    // AC1: $0.id in step 1's args is replaced with the id returned by step 0.
    // A wrong implementation that ignores refs would pass "$0.id" literally to
    // ddb read, which is not a 14-digit id, so ddb would return an error instead
    // of Ok. That distinguishes a correct implementation from a no-op one.
    let driver = CliDriver::new();
    let fixture = fx_multi(
        vec![
            Step {
                op: StepOp::CreateDoogat,
                args: serde_json::json!({"title": "T8 test"}),
            },
            Step {
                op: StepOp::ReadDoogat,
                args: serde_json::json!({"id": "$0.id"}),
            },
        ],
        30_000,
    );
    let results = driver.run_workflow(&fixture);
    assert_eq!(results.len(), 2, "expected 2 results, got {}", results.len());

    // step 0: create must succeed and return a 14-digit id
    let created_id = match &results[0] {
        ConformanceResult::Ok { value: ConformanceValue::String(id), .. } => {
            assert_eq!(
                id.len(), 14,
                "step 0: expected 14-digit id, got {} chars: {id}", id.len()
            );
            assert!(
                id.chars().all(|c| c.is_ascii_digit()),
                "step 0: expected all-digit id, got: {id}"
            );
            id.clone()
        }
        other => panic!("step 0: expected Ok(String(id)), got: {other:?}"),
    };

    // step 1: read must succeed and its output must reference the created doogat
    match &results[1] {
        ConformanceResult::Ok { value: ConformanceValue::String(text), .. } => {
            assert!(
                text.contains("T8 test"),
                "step 1: expected read output to contain title 'T8 test', got: {text}"
            );
            // Confirm the resolved id appears in the output, ruling out an
            // implementation that passes a different id.
            assert!(
                text.contains(&created_id),
                "step 1: expected output to reference created id {created_id}, got: {text}"
            );
        }
        other => panic!("step 1: expected Ok(String(text)), got: {other:?}"),
    }
}

#[test]
fn unresolvable_ref_passes_literal_and_causes_err() {
    // AC2: $99.id refers to step 99 which does not exist. The literal string
    // "$99.id" must be passed unchanged to ddb read, which rejects it as an
    // invalid id (non-zero exit). A wrong implementation that silently swallows
    // the unresolvable ref and passes an empty string would also produce Err,
    // but step 0 returning Ok still confirms the workflow itself ran correctly.
    let driver = CliDriver::new();
    let fixture = fx_multi(
        vec![
            Step {
                op: StepOp::CreateDoogat,
                args: serde_json::json!({"title": "T8 unresolvable"}),
            },
            Step {
                op: StepOp::ReadDoogat,
                args: serde_json::json!({"id": "$99.id"}),
            },
        ],
        30_000,
    );
    let results = driver.run_workflow(&fixture);
    assert_eq!(results.len(), 2, "expected 2 results, got {}", results.len());

    // step 0 must succeed - the workflow itself is sound
    match &results[0] {
        ConformanceResult::Ok { .. } => {}
        other => panic!("step 0: expected Ok, got: {other:?}"),
    }

    // step 1 must fail because "$99.id" is not a valid doogat id
    match &results[1] {
        ConformanceResult::Err(ConformanceError { code, .. }) => {
            assert_eq!(code, "CLI_ERROR", "step 1: expected CLI_ERROR, got: {code}");
        }
        other => panic!("step 1: expected Err(ConformanceError), got: {other:?}"),
    }
}

#[test]
fn three_step_workflow_resolves_step_zero_id_in_step_two() {
    // AC3: prior-result tracking accumulates correctly across 3 steps.
    // step 0: create, step 1: list (no refs, intermediate), step 2: read $0.id.
    // A wrong implementation that only tracks the immediately preceding result
    // would resolve $0.id as step 1's list output (a non-id string), causing
    // step 2 to fail. Correct tracking keeps ALL prior results indexed by step.
    let driver = CliDriver::new();
    let fixture = fx_multi(
        vec![
            Step {
                op: StepOp::CreateDoogat,
                args: serde_json::json!({"title": "T8 three step"}),
            },
            Step {
                op: StepOp::ListDoogats,
                args: serde_json::json!({}),
            },
            Step {
                op: StepOp::ReadDoogat,
                args: serde_json::json!({"id": "$0.id"}),
            },
        ],
        30_000,
    );
    let results = driver.run_workflow(&fixture);
    assert_eq!(results.len(), 3, "expected 3 results, got {}", results.len());

    // step 0: create must return a 14-digit id
    match &results[0] {
        ConformanceResult::Ok { value: ConformanceValue::String(id), .. } => {
            assert_eq!(
                id.len(), 14,
                "step 0: expected 14-digit id, got {} chars: {id}", id.len()
            );
            assert!(
                id.chars().all(|c| c.is_ascii_digit()),
                "step 0: expected all-digit id, got: {id}"
            );
        }
        other => panic!("step 0: expected Ok(String(id)), got: {other:?}"),
    }

    // step 1: list must return Ok with some string output
    match &results[1] {
        ConformanceResult::Ok { value: ConformanceValue::String(_), .. } => {}
        other => panic!("step 1: expected Ok(String(...)), got: {other:?}"),
    }

    // step 2: read via $0.id must succeed and contain the original title
    match &results[2] {
        ConformanceResult::Ok { value: ConformanceValue::String(text), .. } => {
            assert!(
                text.contains("T8 three step"),
                "step 2: expected read output to contain 'T8 three step', got: {text}"
            );
        }
        other => panic!("step 2: expected Ok(String(text)), got: {other:?}"),
    }
}

// Note: a previous `#[ignore]`'d `timeout_yields_setup_failed` test was
// removed in PRD 00148 cycle-2 (F8). It tried to make `ddb list` exceed a
// 1ms timeout, but clap exits in ~2ms before the timer fires. No always-fast
// no-op subcommand exists that reliably exceeds short timeouts on every
// machine, so the timeout-conformance gap is documented in
// docs/src/technical/conformance-harness.md "Deferred scope" until a
// suitable fixture lands.
