use super::super::common::{DdbTestRepo, ServerGuard};
use super::fixture::{Step, StepOp, WorkflowFixture};
use super::result::{ConformanceError, ConformanceResult, ConformanceValue};

pub struct GraphqlDriver {
    _repo: DdbTestRepo,
    server: ServerGuard,
}

impl GraphqlDriver {
    pub fn new() -> Self {
        let repo = DdbTestRepo::init();
        let server = ServerGuard::start(&repo);
        Self { _repo: repo, server }
    }

    pub fn run_workflow(&self, fixture: &WorkflowFixture) -> Vec<ConformanceResult> {
        fixture.steps.iter().map(|step| self.run_step(step)).collect()
    }

    fn run_step(&self, step: &Step) -> ConformanceResult {
        match step.op {
            StepOp::CreateDoogat => {
                let title = match require_string(&step.args, "title") {
                    Ok(v) => v,
                    Err(field) => return setup_failed_missing(field),
                };
                let body = optional_string(&step.args, "body").unwrap_or_default();
                let result = self.server.graphql_with_vars(
                    r#"mutation($input: CreateDoogatInput!) { createDoogat(input: $input) { id title body } }"#,
                    serde_json::json!({ "input": { "title": title, "content": body } }),
                );
                if let Some(errors) = non_empty_errors(&result) {
                    return graphql_err(&errors[0]);
                }
                let id = result["data"]["createDoogat"]["id"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                ConformanceResult::Ok {
                    value: ConformanceValue::String(id),
                    warnings: vec![],
                }
            }
            StepOp::ReadDoogat => {
                let id = match require_string(&step.args, "id") {
                    Ok(v) => v,
                    Err(field) => return setup_failed_missing(field),
                };
                let result = self.server.graphql(&format!(
                    r#"{{ doogat(id: "{id}") {{ id title body tags }} }}"#
                ));
                if let Some(errors) = non_empty_errors(&result) {
                    return graphql_err(&errors[0]);
                }
                let doogat = &result["data"]["doogat"];
                let text = serde_json::to_string(doogat).unwrap_or_default();
                ConformanceResult::Ok {
                    value: ConformanceValue::String(text),
                    warnings: vec![],
                }
            }
            StepOp::UpdateDoogat => {
                let id = match require_string(&step.args, "id") {
                    Ok(v) => v,
                    Err(field) => return setup_failed_missing(field),
                };
                let title = match require_string(&step.args, "title") {
                    Ok(v) => v,
                    Err(field) => return setup_failed_missing(field),
                };
                let result = self.server.graphql_with_vars(
                    r#"mutation($input: UpdateDoogatInput!) { updateDoogat(input: $input) { id title } }"#,
                    serde_json::json!({ "input": { "id": id, "title": title } }),
                );
                if let Some(errors) = non_empty_errors(&result) {
                    return graphql_err(&errors[0]);
                }
                ConformanceResult::Ok {
                    value: ConformanceValue::Null,
                    warnings: vec![],
                }
            }
            StepOp::DeleteDoogat => {
                let id = match require_string(&step.args, "id") {
                    Ok(v) => v,
                    Err(field) => return setup_failed_missing(field),
                };
                let result = self.server.graphql(&format!(
                    r#"mutation {{ deleteDoogat(id: "{id}") }}"#
                ));
                if let Some(errors) = non_empty_errors(&result) {
                    return graphql_err(&errors[0]);
                }
                ConformanceResult::Ok {
                    value: ConformanceValue::Null,
                    warnings: vec![],
                }
            }
            StepOp::ListDoogats => {
                let result = self.server.graphql(r#"{ doogats { id title } }"#);
                if let Some(errors) = non_empty_errors(&result) {
                    return graphql_err(&errors[0]);
                }
                let array = &result["data"]["doogats"];
                let text = serde_json::to_string(array).unwrap_or_default();
                ConformanceResult::Ok {
                    value: ConformanceValue::String(text),
                    warnings: vec![],
                }
            }
            StepOp::Search => {
                let query = match require_string(&step.args, "query") {
                    Ok(v) => v,
                    Err(field) => return setup_failed_missing(field),
                };
                let result = self.server.graphql(&format!(
                    r#"{{ search(query: "{query}") {{ hits {{ id title }} totalCount }} }}"#
                ));
                if let Some(errors) = non_empty_errors(&result) {
                    return graphql_err(&errors[0]);
                }
                let hits = &result["data"]["search"]["hits"];
                let text = serde_json::to_string(hits).unwrap_or_default();
                ConformanceResult::Ok {
                    value: ConformanceValue::String(text),
                    warnings: vec![],
                }
            }
        }
    }
}

impl Default for GraphqlDriver {
    fn default() -> Self {
        Self::new()
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

fn setup_failed_missing(field: &str) -> ConformanceResult {
    ConformanceResult::SetupFailed {
        reason: format!("missing arg: {field}"),
    }
}

fn non_empty_errors(result: &serde_json::Value) -> Option<&Vec<serde_json::Value>> {
    result.get("errors").and_then(|e| e.as_array()).filter(|arr| !arr.is_empty())
}

fn graphql_err(error: &serde_json::Value) -> ConformanceResult {
    let code = error["extensions"]["code"]
        .as_str()
        .unwrap_or("GRAPHQL_ERROR")
        .to_string();
    let message = error["message"].as_str().unwrap_or_default().to_string();
    let context = error["extensions"]
        .as_object()
        .cloned()
        .unwrap_or_default();
    ConformanceResult::Err(ConformanceError { code, message, context })
}

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
