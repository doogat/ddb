use std::path::Path;
use std::sync::OnceLock;
use std::time::Instant;

use crate::error::{Result, DoogatError};
use crate::traits::GitBackend;
use crate::types::MaintenanceReport;

static GIT_MAINTENANCE_AVAILABLE: OnceLock<bool> = OnceLock::new();

fn probe_git_maintenance() -> bool {
    *GIT_MAINTENANCE_AVAILABLE.get_or_init(|| {
        std::process::Command::new("git")
            .args(["maintenance", "run", "-h"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    })
}

pub fn run(repo_path: &Path, tasks: Option<&[&str]>) -> Result<MaintenanceReport> {
    let start = Instant::now();
    let has_maintenance = probe_git_maintenance();

    let (output, fallback_used, tasks_run) = if has_maintenance {
        let mut args = vec!["maintenance".to_string(), "run".to_string()];
        let task_names: Vec<String>;
        if let Some(task_list) = tasks {
            for t in task_list {
                args.push(format!("--task={t}"));
            }
            task_names = task_list.iter().map(|t| t.to_string()).collect();
        } else {
            args.push("--auto".to_string());
            task_names = vec!["auto".to_string()];
        }
        let out = std::process::Command::new("git")
            .args(&args)
            .current_dir(repo_path)
            .output()
            .map_err(DoogatError::Io)?;
        (out, false, task_names)
    } else {
        let out = std::process::Command::new("git")
            .args(["gc", "--auto"])
            .current_dir(repo_path)
            .output()
            .map_err(DoogatError::Io)?;
        (out, true, vec!["gc-auto".to_string()])
    };

    let success = output.status.success();
    if !success {
        let stderr = String::from_utf8_lossy(&output.stderr);
        tracing::warn!(stderr = %stderr, "git maintenance failed");
    }

    Ok(MaintenanceReport {
        tasks_run,
        success,
        duration_ms: start.elapsed().as_millis() as u64,
        fallback_used,
    })
}

/// Check if session commit count has crossed the write threshold;
/// if so, run maintenance and reset the counter.
pub fn check_write_threshold(repo: &impl GitBackend) {
    let count = repo.increment_session_commits();
    let config = match repo.load_config() {
        Ok(c) => c,
        Err(_) => return,
    };
    if !config.maintenance.auto_enabled || config.maintenance.write_threshold == 0 {
        return;
    }
    if count >= config.maintenance.write_threshold {
        repo.reset_session_commits();
        match run(repo.repo_path(), None) {
            Ok(report) => {
                tracing::info!(
                    success = report.success,
                    duration_ms = report.duration_ms,
                    trigger = "write_threshold",
                    "auto-maintenance completed"
                );
            }
            Err(e) => {
                tracing::warn!(error = %e, "write-threshold auto-maintenance failed");
            }
        }
    }
}

pub fn maybe_auto_run(repo: &impl GitBackend) {
    let config = match repo.load_config() {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(error = %e, "auto-maintenance skipped: failed to load config");
            return;
        }
    };
    if !config.maintenance.auto_enabled {
        return;
    }
    match run(repo.repo_path(), None) {
        Ok(report) => {
            tracing::info!(
                success = report.success,
                duration_ms = report.duration_ms,
                fallback = report.fallback_used,
                "auto-maintenance completed"
            );
        }
        Err(e) => {
            tracing::warn!(error = %e, "auto-maintenance failed");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git_ops::GitRepo;

    #[test]
    fn probe_returns_consistent_result() {
        let a = probe_git_maintenance();
        let b = probe_git_maintenance();
        assert_eq!(a, b);
    }

    #[test]
    fn run_succeeds_on_temp_repo() {
        let dir = tempfile::tempdir().unwrap();
        std::process::Command::new("git")
            .args(["init"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        // Create an initial commit so the repo is non-empty.
        std::process::Command::new("git")
            .args(["commit", "--allow-empty", "-m", "init"])
            .current_dir(dir.path())
            .output()
            .unwrap();

        let report = run(dir.path(), None).unwrap();
        assert!(report.success);
    }

    #[test]
    fn run_with_deleted_dir_returns_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_path_buf();
        drop(dir); // delete the directory
        let result = run(&path, None);
        assert!(result.is_err());
    }

    #[test]
    fn run_reports_fallback_field() {
        let dir = tempfile::tempdir().unwrap();
        std::process::Command::new("git")
            .args(["init"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args(["commit", "--allow-empty", "-m", "init"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        let report = run(dir.path(), None).unwrap();
        // fallback_used reflects whether git maintenance subcommand was available
        assert_eq!(report.fallback_used, !probe_git_maintenance());
    }

    #[test]
    fn run_with_explicit_task() {
        let dir = tempfile::tempdir().unwrap();
        std::process::Command::new("git")
            .args(["init"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args(["commit", "--allow-empty", "-m", "init"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        let report = run(dir.path(), Some(&["commit-graph"]));
        // If git maintenance is available, explicit task should work.
        // If not (fallback), it falls back to gc --auto regardless.
        if probe_git_maintenance() {
            let report = report.unwrap();
            assert!(!report.fallback_used);
        }
    }

    #[test]
    fn maybe_auto_run_skips_when_disabled() {
        let dir = tempfile::tempdir().unwrap();
        let repo = GitRepo::init(dir.path()).unwrap();
        // Default config has auto_enabled = false; this should be a no-op.
        maybe_auto_run(&repo);
    }

    #[test]
    fn maybe_auto_run_runs_when_enabled() {
        let dir = tempfile::tempdir().unwrap();
        let repo = GitRepo::init(dir.path()).unwrap();
        let toml = "[maintenance]\nauto_enabled = true\n";
        repo.commit_file(".ddb.toml", toml, "enable maintenance")
            .unwrap();
        maybe_auto_run(&repo);
    }

    #[test]
    fn check_write_threshold_skips_when_disabled() {
        let dir = tempfile::tempdir().unwrap();
        let repo = GitRepo::init(dir.path()).unwrap();
        // Default: auto_enabled = false. Should increment but not trigger maintenance.
        for _ in 0..60 {
            check_write_threshold(&repo);
        }
        // No panic, no error — just a no-op.
    }

    #[test]
    fn check_write_threshold_triggers_at_threshold() {
        let dir = tempfile::tempdir().unwrap();
        let repo = GitRepo::init(dir.path()).unwrap();
        let toml = "[maintenance]\nauto_enabled = true\nwrite_threshold = 3\n";
        repo.commit_file(".ddb.toml", toml, "enable maintenance")
            .unwrap();
        // Counter was incremented by commit_file above (via check_write_threshold in commit_files).
        // Reset to control the test precisely.
        repo.reset_session_commits();

        check_write_threshold(&repo); // count=1
        check_write_threshold(&repo); // count=2
        check_write_threshold(&repo); // count=3 — triggers + resets
                                      // After trigger, counter resets; next call should not trigger.
        check_write_threshold(&repo); // count=1
    }
}
