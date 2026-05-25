use super::fixture::{AuthMode, SetupExpectation};
use super::result::ConformanceResult;

/// Returns `Some(SetupFailed)` if the fixture's setup expectation names
/// capabilities the drivers do not yet implement. Returns `None` for the
/// supported subset (currently `AuthMode::None` + empty `setup_steps`).
///
/// Centralized here so both `CliDriver` and `GraphqlDriver` reject the same
/// unsupported inputs identically — see the conformance-harness
/// deferred-scope section.
pub fn check_setup_supported(setup: &SetupExpectation) -> Option<ConformanceResult> {
    if !matches!(setup.auth_mode, AuthMode::None) {
        return Some(ConformanceResult::SetupFailed {
            reason: format!(
                "unsupported auth_mode {:?}; drivers currently only honor AuthMode::None",
                setup.auth_mode
            ),
        });
    }
    if !setup.setup_steps.is_empty() {
        return Some(ConformanceResult::SetupFailed {
            reason: format!(
                "setup_steps not yet executed by drivers; fixture requested {} step(s)",
                setup.setup_steps.len()
            ),
        });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn none_auth_and_empty_steps_returns_none() {
        let setup = SetupExpectation {
            auth_mode: AuthMode::None,
            timeout_ms: 30_000,
            setup_steps: vec![],
        };
        assert!(check_setup_supported(&setup).is_none());
    }

    #[test]
    fn token_auth_returns_setup_failed() {
        let setup = SetupExpectation {
            auth_mode: AuthMode::Token {
                env_var: "DDB_TOKEN".into(),
            },
            timeout_ms: 30_000,
            setup_steps: vec![],
        };
        let result = check_setup_supported(&setup).expect("should fail loud");
        match result {
            ConformanceResult::SetupFailed { reason } => {
                assert!(reason.contains("auth_mode"), "reason: {reason}");
            }
            other => panic!("expected SetupFailed, got {other:?}"),
        }
    }

    #[test]
    fn embedded_auth_returns_setup_failed() {
        let setup = SetupExpectation {
            auth_mode: AuthMode::Embedded,
            timeout_ms: 30_000,
            setup_steps: vec![],
        };
        assert!(check_setup_supported(&setup).is_some());
    }

    #[test]
    fn non_empty_setup_steps_returns_setup_failed() {
        let setup = SetupExpectation {
            auth_mode: AuthMode::None,
            timeout_ms: 30_000,
            setup_steps: vec!["init_baseline".into()],
        };
        let result = check_setup_supported(&setup).expect("should fail loud");
        match result {
            ConformanceResult::SetupFailed { reason } => {
                assert!(reason.contains("setup_steps"), "reason: {reason}");
            }
            other => panic!("expected SetupFailed, got {other:?}"),
        }
    }
}
