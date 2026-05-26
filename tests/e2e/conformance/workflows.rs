use super::fixture::{
    AuthMode, ExpectedBehavior, InterfaceId, SetupExpectation, Step, StepOp, WorkflowFixture,
};
use super::result::ConformanceError;

pub fn validation_error() -> WorkflowFixture {
    WorkflowFixture {
        id: "validation_error".into(),
        title: "Validation error scenario".into(),
        setup: SetupExpectation {
            auth_mode: AuthMode::None,
            timeout_ms: 30_000,
            setup_steps: vec![],
        },
        steps: vec![Step {
            op: StepOp::CreateDoogat,
            args: serde_json::json!({"title": ""}),
        }],
        expected: ExpectedBehavior {
            value: None,
            warnings: vec![],
            error: Some(ConformanceError {
                code: "VALIDATION_ERROR".into(),
                message: "Title cannot be empty".into(),
                context: serde_json::Map::new(),
            }),
        },
        interfaces: vec![InterfaceId::Cli, InterfaceId::Graphql],
    }
}

pub fn crud_baseline() -> WorkflowFixture {
    WorkflowFixture {
        id: "crud_baseline".into(),
        title: "CRUD baseline".into(),
        setup: SetupExpectation {
            auth_mode: AuthMode::None,
            timeout_ms: 30_000,
            setup_steps: vec![],
        },
        // Full CRUD cycle on a single doogat created in step 0, plus a
        // stable search probe (GW-2 high-risk reachable rows: response
        // shape and field names for CLI and GraphQL search) and a
        // stable not-found id for the CRUD baseline error-path contract.
        // Step indices: 0=Create, 1=Read, 2=Update, 3=Delete, 4=List,
        // 5=Search, 6=ReadDoogat(nonexistent).
        //
        // PRD 00150 T8: the Search step pins the promised search surface
        // for both drivers via existing harness `StepOp::Search`. Per the
        // metadata-only ExpectedBehavior contract documented in
        // `docs/src/technical/conformance-harness.md` (Deferred scope),
        // value-level pinning is not enforced by the comparator yet, so
        // the workflow's overall `expected` stays empty; the Step's mere
        // presence pins the cross-driver shape via per-step variant
        // comparison (`crud_baseline_no_step_has_variant_mismatch`).
        steps: vec![
            Step {
                op: StepOp::CreateDoogat,
                args: serde_json::json!({"title": "Test doogat"}),
            },
            Step {
                op: StepOp::ReadDoogat,
                args: serde_json::json!({"id": "$0.id"}),
            },
            Step {
                op: StepOp::UpdateDoogat,
                args: serde_json::json!({"id": "$0.id", "title": "Updated doogat"}),
            },
            Step {
                op: StepOp::DeleteDoogat,
                args: serde_json::json!({"id": "$0.id"}),
            },
            Step {
                op: StepOp::ListDoogats,
                args: serde_json::json!({}),
            },
            Step {
                op: StepOp::Search,
                args: serde_json::json!({"query": "Test"}),
            },
            Step {
                op: StepOp::ReadDoogat,
                args: serde_json::json!({"id": "99999999999999"}),
            },
        ],
        expected: ExpectedBehavior {
            value: None,
            warnings: vec![],
            error: None,
        },
        interfaces: vec![InterfaceId::Cli, InterfaceId::Graphql],
    }
}

#[cfg(test)]
mod tests {
    use super::super::fixture::*;
    use super::*;

    #[test]
    fn returns_crud_baseline_id() {
        let fixture = crud_baseline();
        assert_eq!(fixture.id, "crud_baseline");
    }

    #[test]
    fn returns_crud_baseline_title() {
        let fixture = crud_baseline();
        assert_eq!(fixture.title, "CRUD baseline");
    }

    #[test]
    fn includes_cli_interface() {
        let fixture = crud_baseline();
        assert!(fixture.interfaces.contains(&InterfaceId::Cli));
    }

    #[test]
    fn includes_graphql_interface() {
        let fixture = crud_baseline();
        assert!(fixture.interfaces.contains(&InterfaceId::Graphql));
    }

    #[test]
    fn steps_is_non_empty() {
        let fixture = crud_baseline();
        assert!(!fixture.steps.is_empty());
    }

    #[test]
    fn steps_contains_create_doogat() {
        let fixture = crud_baseline();
        let has_create = fixture.steps.iter().any(|s| s.op == StepOp::CreateDoogat);
        assert!(has_create);
    }

    #[test]
    fn steps_contains_list_doogats() {
        let fixture = crud_baseline();
        let has_list = fixture.steps.iter().any(|s| s.op == StepOp::ListDoogats);
        assert!(has_list);
    }

    // PRD 00150 T8: pins the Search step in crud_baseline so the
    // GW-2 high-risk reachable rows (CLI/GraphQL search response shape
    // + field names) cannot regress out of the fixture without this
    // test failing first.
    #[test]
    fn steps_contains_search() {
        let fixture = crud_baseline();
        let has_search = fixture.steps.iter().any(|s| s.op == StepOp::Search);
        assert!(has_search);
    }

    #[test]
    fn setup_auth_mode_is_none() {
        let fixture = crud_baseline();
        assert_eq!(fixture.setup.auth_mode, AuthMode::None);
    }

    #[test]
    fn setup_timeout_ms_is_30000() {
        let fixture = crud_baseline();
        assert_eq!(fixture.setup.timeout_ms, 30_000);
    }

    #[test]
    fn expected_error_is_none() {
        let fixture = crud_baseline();
        assert!(fixture.expected.error.is_none());
    }

    #[test]
    fn expected_warnings_is_empty() {
        let fixture = crud_baseline();
        assert!(fixture.expected.warnings.is_empty());
    }

    #[test]
    fn returns_validation_error_id() {
        let fixture = validation_error();
        assert_eq!(fixture.id, "validation_error");
    }

    #[test]
    fn validation_error_title_is_non_empty() {
        let fixture = validation_error();
        assert!(!fixture.title.is_empty());
    }

    #[test]
    fn validation_error_includes_cli_interface() {
        let fixture = validation_error();
        assert!(fixture.interfaces.contains(&InterfaceId::Cli));
    }

    #[test]
    fn validation_error_includes_graphql_interface() {
        let fixture = validation_error();
        assert!(fixture.interfaces.contains(&InterfaceId::Graphql));
    }

    #[test]
    fn validation_error_steps_is_non_empty() {
        let fixture = validation_error();
        assert!(!fixture.steps.is_empty());
    }

    #[test]
    fn validation_error_steps_contains_create_doogat() {
        let fixture = validation_error();
        let has_create = fixture.steps.iter().any(|s| s.op == StepOp::CreateDoogat);
        assert!(has_create);
    }

    #[test]
    fn validation_error_expected_error_is_some() {
        let fixture = validation_error();
        assert!(fixture.expected.error.is_some());
    }

    #[test]
    fn validation_error_error_code_is_non_empty() {
        let fixture = validation_error();
        assert!(!fixture.expected.error.as_ref().unwrap().code.is_empty());
    }

    #[test]
    fn validation_error_setup_auth_mode_is_none() {
        let fixture = validation_error();
        assert_eq!(fixture.setup.auth_mode, AuthMode::None);
    }

    #[test]
    fn validation_error_has_exactly_one_step() {
        let fixture = validation_error();
        assert_eq!(fixture.steps.len(), 1);
    }

    #[test]
    fn validation_error_interfaces_has_exactly_two() {
        let fixture = validation_error();
        assert_eq!(fixture.interfaces.len(), 2);
    }
}
