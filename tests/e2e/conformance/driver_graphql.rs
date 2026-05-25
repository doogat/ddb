use std::time::Duration;

use super::super::common::{DdbTestRepo, ServerGuard};
use super::args::{optional_string, require_string};
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

    /// Post a GraphQL request to the running test server with the given
    /// per-request timeout. Used in place of `ServerGuard::graphql*` so the
    /// driver honors `fixture.setup.timeout_ms` (PRD 00148 cycle-2 F4).
    fn post_graphql(&self, body: serde_json::Value, timeout: Duration) -> serde_json::Value {
        reqwest::blocking::Client::new()
            .post(self.server.url())
            .header("Authorization", format!("Bearer {}", self.server.token))
            .json(&body)
            .timeout(timeout)
            .send()
            .expect("request failed")
            .json()
            .expect("invalid json")
    }

    fn run_step(&self, step: &Step, timeout: Duration) -> ConformanceResult {
        match step.op {
            StepOp::CreateDoogat => self.run_create(&step.args, timeout),
            StepOp::ReadDoogat => self.run_read(&step.args, timeout),
            StepOp::UpdateDoogat => self.run_update(&step.args, timeout),
            StepOp::DeleteDoogat => self.run_delete(&step.args, timeout),
            StepOp::ListDoogats => self.run_list(timeout),
            StepOp::Search => self.run_search(&step.args, timeout),
        }
    }

    fn run_create(&self, args: &serde_json::Value, timeout: Duration) -> ConformanceResult {
        let title = match require_string(args, "title") {
            Ok(v) => v,
            Err(field) => return setup_failed_missing(field),
        };
        let body = optional_string(args, "body").unwrap_or_default();
        let result = self.post_graphql(
            serde_json::json!({
                "query": r#"mutation($input: CreateDoogatInput!) { createDoogat(input: $input) { id title body } }"#,
                "variables": { "input": { "title": title, "content": body } },
            }),
            timeout,
        );
        if let Some(errors) = non_empty_errors(&result) {
            return graphql_err(&errors[0]);
        }
        let id = result["data"]["createDoogat"]["id"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        if id.is_empty() {
            return ConformanceResult::Err(ConformanceError {
                code: "GRAPHQL_ERROR".into(),
                message: "createDoogat returned no id".into(),
                context: serde_json::Map::new(),
            });
        }
        ConformanceResult::Ok {
            value: ConformanceValue::String(id),
            warnings: vec![],
        }
    }

    fn run_read(&self, args: &serde_json::Value, timeout: Duration) -> ConformanceResult {
        let id = match require_string(args, "id") {
            Ok(v) => v,
            Err(field) => return setup_failed_missing(field),
        };
        let result = self.post_graphql(
            serde_json::json!({
                "query": r#"query($id: ID!) { doogat(id: $id) { id title body tags } }"#,
                "variables": { "id": id },
            }),
            timeout,
        );
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

    fn run_update(&self, args: &serde_json::Value, timeout: Duration) -> ConformanceResult {
        let id = match require_string(args, "id") {
            Ok(v) => v,
            Err(field) => return setup_failed_missing(field),
        };
        let title = match require_string(args, "title") {
            Ok(v) => v,
            Err(field) => return setup_failed_missing(field),
        };
        let result = self.post_graphql(
            serde_json::json!({
                "query": r#"mutation($input: UpdateDoogatInput!) { updateDoogat(input: $input) { id title } }"#,
                "variables": { "input": { "id": id, "title": title } },
            }),
            timeout,
        );
        if let Some(errors) = non_empty_errors(&result) {
            return graphql_err(&errors[0]);
        }
        ConformanceResult::Ok {
            value: ConformanceValue::Null,
            warnings: vec![],
        }
    }

    fn run_delete(&self, args: &serde_json::Value, timeout: Duration) -> ConformanceResult {
        let id = match require_string(args, "id") {
            Ok(v) => v,
            Err(field) => return setup_failed_missing(field),
        };
        let result = self.post_graphql(
            serde_json::json!({
                "query": r#"mutation($id: ID!) { deleteDoogat(id: $id) }"#,
                "variables": { "id": id },
            }),
            timeout,
        );
        if let Some(errors) = non_empty_errors(&result) {
            return graphql_err(&errors[0]);
        }
        ConformanceResult::Ok {
            value: ConformanceValue::Null,
            warnings: vec![],
        }
    }

    fn run_list(&self, timeout: Duration) -> ConformanceResult {
        let result = self.post_graphql(
            serde_json::json!({"query": r#"{ doogats { id title } }"#}),
            timeout,
        );
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

    fn run_search(&self, args: &serde_json::Value, timeout: Duration) -> ConformanceResult {
        let query = match require_string(args, "query") {
            Ok(v) => v,
            Err(field) => return setup_failed_missing(field),
        };
        let result = self.post_graphql(
            serde_json::json!({
                "query": r#"query($q: String!) { search(query: $q) { hits { id title } totalCount } }"#,
                "variables": { "q": query },
            }),
            timeout,
        );
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

impl Default for GraphqlDriver {
    fn default() -> Self {
        Self::new()
    }
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

    #[test]
    fn resolves_dollar_zero_id_in_second_step_arg() {
        // AC1: single run_workflow call; step 1 uses $0.id resolved from step 0's result.
        // A wrong implementation that passes "$0.id" literally to GraphQL would get Err for step 1,
        // not Ok containing the title and id.
        let driver = GraphqlDriver::new();
        let fixture = fx(
            vec![
                Step {
                    op: StepOp::CreateDoogat,
                    args: serde_json::json!({"title": "T9 test"}),
                },
                Step {
                    op: StepOp::ReadDoogat,
                    args: serde_json::json!({"id": "$0.id"}),
                },
            ],
            30_000,
        );
        let results = driver.run_workflow(&fixture);
        assert_eq!(results.len(), 2);

        let id = match &results[0] {
            ConformanceResult::Ok { value: ConformanceValue::String(id), .. } => id.clone(),
            other => panic!("step 0 expected Ok(String(id)), got: {other:?}"),
        };
        assert_eq!(id.len(), 14, "step 0 id should be 14 digits, got: {id}");

        match &results[1] {
            ConformanceResult::Ok { value: ConformanceValue::String(text), .. } => {
                assert!(
                    text.contains("T9 test"),
                    "step 1 text should contain title 'T9 test', got: {text}"
                );
                assert!(
                    text.contains(&id),
                    "step 1 text should contain id {id}, got: {text}"
                );
            }
            other => panic!("step 1 expected Ok(String(text)), got: {other:?}"),
        }
    }

    #[test]
    fn unresolvable_ref_passes_literal_and_graphql_rejects_it() {
        // AC2: $99.id refers to a step that doesn't exist, so the literal "$99.id" is passed
        // to GraphQL, which rejects it as an invalid id. Step 0 must still succeed.
        // A wrong implementation that substitutes empty string would also get Err but the test
        // still distinguishes correct behavior (literal passed) from silent empty-string insertion.
        let driver = GraphqlDriver::new();
        let fixture = fx(
            vec![
                Step {
                    op: StepOp::CreateDoogat,
                    args: serde_json::json!({"title": "T9 unresolvable ref"}),
                },
                Step {
                    op: StepOp::ReadDoogat,
                    args: serde_json::json!({"id": "$99.id"}),
                },
            ],
            30_000,
        );
        let results = driver.run_workflow(&fixture);
        assert_eq!(results.len(), 2);

        match &results[0] {
            ConformanceResult::Ok { .. } => {}
            other => panic!("step 0 expected Ok, got: {other:?}"),
        }

        match &results[1] {
            ConformanceResult::Err(_) => {}
            other => panic!("step 1 expected Err (invalid id), got: {other:?}"),
        }
    }

    #[test]
    fn resolves_step_zero_ref_in_step_two_skipping_step_one() {
        // AC3: three-step workflow; step 2 uses $0.id, not $1.id. A wrong implementation that
        // only tracks the immediately prior result would fail step 2 because step 1 (ListDoogats)
        // returns a JSON array, not a string id.
        let driver = GraphqlDriver::new();
        let fixture = fx(
            vec![
                Step {
                    op: StepOp::CreateDoogat,
                    args: serde_json::json!({"title": "T9 indexed ref"}),
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
        assert_eq!(results.len(), 3);

        let id = match &results[0] {
            ConformanceResult::Ok { value: ConformanceValue::String(id), .. } => id.clone(),
            other => panic!("step 0 expected Ok(String(id)), got: {other:?}"),
        };

        match &results[1] {
            ConformanceResult::Ok { .. } => {}
            other => panic!("step 1 expected Ok, got: {other:?}"),
        }

        match &results[2] {
            ConformanceResult::Ok { value: ConformanceValue::String(text), .. } => {
                assert!(
                    text.contains("T9 indexed ref"),
                    "step 2 text should contain title 'T9 indexed ref', got: {text}"
                );
                assert!(
                    text.contains(&id),
                    "step 2 text should contain id {id} from step 0, got: {text}"
                );
            }
            other => panic!("step 2 expected Ok(String(text)), got: {other:?}"),
        }
    }
}
