// CLI conformance driver.

use std::process::Stdio;
use std::thread;
use std::time::{Duration, Instant};

use super::super::common::{ddb_bin, DdbTestRepo};
use super::args::{optional_string, require_string};
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
            let resolved_step = super::step_refs::resolve_step(step, &results);
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
