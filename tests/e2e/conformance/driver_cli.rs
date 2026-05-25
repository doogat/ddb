// CLI conformance driver.

use std::process::Stdio;
use std::thread;
use std::time::{Duration, Instant};

use super::super::common::{ddb_bin, DdbTestRepo};
use super::fixture::{Step, StepOp, WorkflowFixture};
use super::result::{ConformanceError, ConformanceResult, ConformanceValue};

pub struct CliDriver {
    repo: DdbTestRepo,
}

impl CliDriver {
    pub fn new() -> Self {
        Self {
            repo: DdbTestRepo::init(),
        }
    }

    pub fn run_workflow(&self, fixture: &WorkflowFixture) -> Vec<ConformanceResult> {
        if let Some(failure) = super::setup::check_setup_supported(&fixture.setup) {
            return vec![failure; fixture.steps.len()];
        }
        let timeout = Duration::from_millis(fixture.setup.timeout_ms);
        let mut results: Vec<ConformanceResult> = Vec::new();
        for step in &fixture.steps {
            let resolved_args = super::step_refs::resolve_refs(&step.args, &results);
            let resolved_step = Step {
                args: resolved_args,
                op: step.op,
            };
            results.push(self.run_step(&resolved_step, timeout));
        }
        results
    }

    fn run_step(&self, step: &Step, timeout: Duration) -> ConformanceResult {
        let args = match build_args(step) {
            Ok(args) => args,
            Err(missing) => {
                return ConformanceResult::SetupFailed {
                    reason: format!("missing arg: {missing}"),
                };
            }
        };

        let output = match run_ddb(self.repo.path(), &args, timeout) {
            Ok(out) => out,
            Err(RunError::Timeout) => {
                return ConformanceResult::SetupFailed {
                    reason: "timeout exceeded".into(),
                };
            }
            Err(RunError::Spawn(msg)) => {
                return ConformanceResult::SetupFailed { reason: msg };
            }
        };

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            let exit_code = output.status.code().unwrap_or(-1);
            let mut context = serde_json::Map::new();
            context.insert("exit_code".into(), serde_json::Value::from(exit_code));
            return ConformanceResult::Err(ConformanceError {
                code: "CLI_ERROR".into(),
                message: stderr,
                context,
            });
        }

        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let value = match step.op {
            StepOp::CreateDoogat
            | StepOp::ReadDoogat
            | StepOp::ListDoogats
            | StepOp::Search => ConformanceValue::String(stdout),
            StepOp::UpdateDoogat | StepOp::DeleteDoogat => ConformanceValue::Null,
        };

        ConformanceResult::Ok {
            value,
            warnings: vec![],
        }
    }
}

impl Default for CliDriver {
    fn default() -> Self {
        Self::new()
    }
}

fn build_args(step: &Step) -> Result<Vec<String>, &'static str> {
    match step.op {
        StepOp::CreateDoogat => {
            let title = require_string(&step.args, "title")?;
            let body = optional_string(&step.args, "body").unwrap_or_default();
            Ok(vec![
                "create".into(),
                "--title".into(),
                title,
                "--body".into(),
                body,
            ])
        }
        StepOp::ReadDoogat => {
            let id = require_string(&step.args, "id")?;
            Ok(vec!["read".into(), id])
        }
        StepOp::UpdateDoogat => {
            let id = require_string(&step.args, "id")?;
            let title = require_string(&step.args, "title")?;
            Ok(vec!["update".into(), id, "--title".into(), title])
        }
        StepOp::DeleteDoogat => {
            let id = require_string(&step.args, "id")?;
            Ok(vec!["delete".into(), id])
        }
        StepOp::ListDoogats => Ok(vec!["query".into(), "SELECT id, title FROM doogats".into()]),
        StepOp::Search => {
            let query = require_string(&step.args, "query")?;
            Ok(vec!["search".into(), query])
        }
    }
}

fn require_string(args: &serde_json::Value, name: &'static str) -> Result<String, &'static str> {
    args.get(name)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or(name)
}

fn optional_string(args: &serde_json::Value, name: &str) -> Option<String> {
    args.get(name).and_then(|v| v.as_str()).map(|s| s.to_string())
}

enum RunError {
    Timeout,
    Spawn(String),
}

fn run_ddb(
    repo: &std::path::Path,
    args: &[String],
    timeout: Duration,
) -> Result<std::process::Output, RunError> {
    let mut child = std::process::Command::new(ddb_bin())
        .arg("--repo")
        .arg(repo)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| RunError::Spawn(format!("spawn failed: {e}")))?;

    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => {
                return child
                    .wait_with_output()
                    .map_err(|e| RunError::Spawn(format!("wait failed: {e}")));
            }
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(RunError::Timeout);
                }
                thread::sleep(Duration::from_millis(5));
            }
            Err(e) => return Err(RunError::Spawn(format!("try_wait failed: {e}"))),
        }
    }
}

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
}
