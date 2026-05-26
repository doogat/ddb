#[derive(Debug, Clone)]
pub struct WorkflowFixture {
    pub id: String,
    pub title: String,
    pub setup: SetupExpectation,
    pub steps: Vec<Step>,
    pub expected: ExpectedBehavior,
    pub interfaces: Vec<InterfaceId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetupExpectation {
    /// Auth mode the workflow expects the driver to set up.
    ///
    /// Currently only `AuthMode::None` is honored by drivers. Drivers fail
    /// loud on `Token` / `Embedded` (PRD 00148 Success Criterion 7;
    /// conformance-harness deferred-scope section). Adding a fixture that
    /// requires non-None auth requires teaching the corresponding driver(s)
    /// to satisfy it.
    pub auth_mode: AuthMode,
    /// Per-step timeout in milliseconds. `CliDriver` wraps each subprocess
    /// in this timeout; `GraphqlDriver` applies it to each HTTP request via
    /// reqwest client configuration.
    pub timeout_ms: u64,
    /// Named setup actions to perform before operation steps (e.g.
    /// "create_baseline_typedef"). Currently no driver interprets these — a
    /// non-empty `setup_steps` causes drivers to fail loud (`SetupFailed`).
    /// Adding fixtures that need setup actions requires wiring an interpreter
    /// into the relevant driver(s); see deferred-scope.
    pub setup_steps: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthMode {
    /// No auth required for the workflow. The only mode current drivers honor.
    None,
    /// Bearer-token auth read from the named env var. Not yet enforced by
    /// drivers; see deferred-scope.
    Token { env_var: String },
    /// Embedded / FFI auth (no network). Not yet enforced by drivers; see
    /// deferred-scope.
    Embedded,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Step {
    pub op: StepOp,
    pub args: serde_json::Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StepOp {
    CreateDoogat,
    ReadDoogat,
    UpdateDoogat,
    DeleteDoogat,
    ListDoogats,
    Search,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExpectedBehavior {
    /// The fixture's declared expected value/warnings/error. Currently
    /// **metadata-only** — the comparator compares driver outputs against each
    /// other (cross-driver equivalence) rather than against this expectation.
    /// Enforcement is deferred; see the conformance-harness deferred-scope
    /// section. When wired, the comparator will fold this into per-step
    /// classification using the new `DiffClass` categories (value/error/
    /// warning mismatch).
    pub value: Option<super::result::ConformanceValue>,
    /// See `ExpectedBehavior::value` — currently metadata-only.
    pub warnings: Vec<super::result::ConformanceWarning>,
    /// See `ExpectedBehavior::value` — currently metadata-only.
    pub error: Option<super::result::ConformanceError>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InterfaceId {
    Cli,
    Graphql,
    Rest,
    PgWire,
    Ffi,
    NosqlHttp,
}

#[cfg(test)]
mod tests {
    use super::super::result::{ConformanceValue, ConformanceWarning};
    use super::*;

    #[test]
    fn workflow_fixture_constructs_with_all_fields() {
        let fixture = WorkflowFixture {
            id: "crud_baseline".into(),
            title: "CRUD baseline".into(),
            setup: SetupExpectation {
                auth_mode: AuthMode::Token {
                    env_var: "DDB_AUTH_TOKEN".into(),
                },
                timeout_ms: 30000,
                setup_steps: vec![],
            },
            steps: vec![Step {
                op: StepOp::CreateDoogat,
                args: serde_json::json!({}),
            }],
            expected: ExpectedBehavior {
                value: Some(ConformanceValue::Null),
                warnings: vec![],
                error: None,
            },
            interfaces: vec![InterfaceId::Cli, InterfaceId::Graphql],
        };
        let _cloned = fixture.clone();
        let debug = format!("{:?}", fixture);
        assert!(debug.contains("crud_baseline"));
        assert!(debug.contains("CRUD baseline"));
    }

    #[test]
    fn setup_expectation_eq_compares_all_fields() {
        let a = SetupExpectation {
            auth_mode: AuthMode::None,
            timeout_ms: 1000,
            setup_steps: vec!["init".into()],
        };
        let b = SetupExpectation {
            auth_mode: AuthMode::None,
            timeout_ms: 1000,
            setup_steps: vec!["init".into()],
        };
        let c = SetupExpectation {
            auth_mode: AuthMode::None,
            timeout_ms: 2000,
            setup_steps: vec!["init".into()],
        };
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn auth_mode_token_carries_env_var() {
        let mode = AuthMode::Token {
            env_var: "DDB_AUTH_TOKEN".into(),
        };
        match mode {
            AuthMode::Token { env_var } => {
                assert_eq!(env_var, "DDB_AUTH_TOKEN");
            }
            _ => panic!("expected AuthMode::Token variant"),
        }
    }

    #[test]
    fn auth_mode_variants_are_distinct() {
        let none = AuthMode::None;
        let embedded = AuthMode::Embedded;
        let token = AuthMode::Token {
            env_var: "X".into(),
        };
        assert_ne!(none, embedded);
        assert_ne!(none, token);
        assert_ne!(embedded, token);
    }

    #[test]
    fn step_carries_op_and_args_separately() {
        let step = Step {
            op: StepOp::CreateDoogat,
            args: serde_json::json!({"type": "task", "title": "T"}),
        };
        assert_eq!(
            step.args.pointer("/type"),
            Some(&serde_json::Value::String("task".into()))
        );
        assert_eq!(
            step.args.pointer("/title"),
            Some(&serde_json::Value::String("T".into()))
        );
    }

    #[test]
    fn step_op_variants_are_six_and_distinct() {
        let variants: Vec<StepOp> = vec![
            StepOp::CreateDoogat,
            StepOp::ReadDoogat,
            StepOp::UpdateDoogat,
            StepOp::DeleteDoogat,
            StepOp::ListDoogats,
            StepOp::Search,
        ];
        assert_eq!(variants.len(), 6);
        for i in 0..variants.len() - 1 {
            assert_ne!(variants[i], variants[i + 1]);
        }
    }

    #[test]
    fn expected_behavior_default_construction() {
        let expected = ExpectedBehavior {
            value: None,
            warnings: vec![],
            error: None,
        };
        assert!(expected.value.is_none());
        assert!(expected.warnings.is_empty());
        assert!(expected.error.is_none());
    }

    #[test]
    fn expected_behavior_with_warning() {
        let expected = ExpectedBehavior {
            value: Some(ConformanceValue::String("ok".into())),
            warnings: vec![ConformanceWarning {
                code: "TITLE_FROM_TEMPLATE".into(),
                message: "templated".into(),
                fields: serde_json::Map::new(),
            }],
            error: None,
        };
        assert_eq!(expected.warnings.len(), 1);
        assert_eq!(expected.warnings[0].code, "TITLE_FROM_TEMPLATE");
    }

    #[test]
    fn interface_id_six_variants_are_distinct() {
        let all = [
            InterfaceId::Cli,
            InterfaceId::Graphql,
            InterfaceId::Rest,
            InterfaceId::PgWire,
            InterfaceId::Ffi,
            InterfaceId::NosqlHttp,
        ];
        for i in 0..all.len() {
            for j in 0..all.len() {
                if i != j {
                    assert_ne!(all[i], all[j]);
                }
            }
        }
    }

    #[test]
    fn interface_id_implements_copy() {
        fn takes_id(_: InterfaceId) {}
        let id = InterfaceId::Cli;
        takes_id(id);
        // Only compiles if InterfaceId is Copy: id remains usable after move-by-value.
        assert_eq!(id, InterfaceId::Cli);
    }
}
