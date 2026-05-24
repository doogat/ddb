use super::fixture::{
    AuthMode, ExpectedBehavior, InterfaceId, SetupExpectation, Step, StepOp, WorkflowFixture,
};

pub fn crud_baseline() -> WorkflowFixture {
    todo!()
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
}
