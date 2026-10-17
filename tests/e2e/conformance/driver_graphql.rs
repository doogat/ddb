use std::time::Duration;

use super::super::common::{DdbTestRepo, ServerGuard};
use super::args::{optional_string, optional_string_list, optional_string_map, require_string};
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
        Self {
            _repo: repo,
            server,
        }
    }

    /// Test-only: kill the underlying server so subsequent `run_workflow`
    /// calls hit transport errors and return `ConformanceResult::SetupFailed`.
    /// Used to verify the transport-error mapping in `post_graphql`.
    pub fn kill_server_for_test(&mut self) {
        self.server.kill();
        std::thread::sleep(Duration::from_millis(50));
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
    /// per-request timeout so the driver honors `fixture.setup.timeout_ms`.
    ///
    /// Returns `Err(ConformanceResult::SetupFailed { reason })` on transport
    /// failures: request timeout, connection error, or malformed JSON
    /// response. Callers propagate the `SetupFailed` directly. This mirrors
    /// `CliDriver`, which maps `RunError::Timeout` to `SetupFailed` with a
    /// `"timeout"` reason. GraphQL application errors (HTTP 200 with a
    /// non-empty `errors` array) flow through the regular `Ok` result and
    /// are handled by callers via `non_empty_errors` + `graphql_err`.
    fn post_graphql(
        &self,
        body: serde_json::Value,
        timeout: Duration,
    ) -> Result<serde_json::Value, ConformanceResult> {
        let response = match reqwest::blocking::Client::new()
            .post(self.server.url())
            .header("Authorization", format!("Bearer {}", self.server.token))
            .json(&body)
            .timeout(timeout)
            .send()
        {
            Ok(r) => r,
            Err(e) if e.is_timeout() => {
                return Err(ConformanceResult::SetupFailed {
                    reason: format!("graphql request timed out after {}ms", timeout.as_millis()),
                });
            }
            Err(e) => {
                return Err(ConformanceResult::SetupFailed {
                    reason: format!("graphql request failed: {e}"),
                });
            }
        };
        response.json().map_err(|e| {
            if e.is_timeout() {
                ConformanceResult::SetupFailed {
                    reason: format!("graphql request timed out after {}ms", timeout.as_millis()),
                }
            } else {
                ConformanceResult::SetupFailed {
                    reason: format!("graphql response not valid json: {e}"),
                }
            }
        })
    }

    fn run_step(&self, step: &Step, timeout: Duration) -> ConformanceResult {
        match step.op {
            StepOp::CreateDoogat => self.run_create(&step.args, timeout),
            StepOp::ReadDoogat => self.run_read(&step.args, timeout),
            StepOp::UpdateDoogat => self.run_update(&step.args, timeout),
            StepOp::DeleteDoogat => self.run_delete(&step.args, timeout),
            StepOp::ListDoogats => self.run_list(timeout),
            StepOp::Search => self.run_search(&step.args, timeout),
            StepOp::ExecuteSql => self.run_execute_sql(&step.args, timeout),
        }
    }

    fn run_create(&self, args: &serde_json::Value, timeout: Duration) -> ConformanceResult {
        let title = match require_string(args, "title") {
            Ok(v) => v,
            Err(field) => return setup_failed_missing(field),
        };
        let body = optional_string(args, "body").unwrap_or_default();
        let mut input = serde_json::Map::new();
        input.insert("title".into(), title.into());
        input.insert("content".into(), body.into());
        // Optional typed/tagged create inputs are only sent when the fixture
        // names them, so untyped fixtures post the same input as before.
        match optional_string_list(args, "tags") {
            Ok(Some(tags)) => {
                input.insert("tags".into(), tags.into());
            }
            Ok(None) => {}
            Err(field) => return setup_failed_missing(field),
        }
        if let Some(doogat_type) = optional_string(args, "type") {
            input.insert("type".into(), doogat_type.into());
        }
        match optional_string_map(args, "fields") {
            Ok(Some(fields)) => {
                let object: serde_json::Map<String, serde_json::Value> =
                    fields.into_iter().map(|(k, v)| (k, v.into())).collect();
                // CreateDoogatInput.fields is a JSON-encoded string.
                let encoded = serde_json::Value::Object(object).to_string();
                input.insert("fields".into(), encoded.into());
            }
            Ok(None) => {}
            Err(field) => return setup_failed_missing(field),
        }
        let result = match self.post_graphql(
            serde_json::json!({
                "query": r#"mutation($input: CreateDoogatInput!) { createDoogat(input: $input) { id title body } }"#,
                "variables": { "input": input },
            }),
            timeout,
        ) {
            Ok(v) => v,
            Err(setup_failed) => return setup_failed,
        };
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
        let result = match self.post_graphql(
            serde_json::json!({
                "query": r#"query($id: ID!) { doogat(id: $id) { id title body tags } }"#,
                "variables": { "id": id },
            }),
            timeout,
        ) {
            Ok(v) => v,
            Err(setup_failed) => return setup_failed,
        };
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
        let result = match self.post_graphql(
            serde_json::json!({
                "query": r#"mutation($input: UpdateDoogatInput!) { updateDoogat(input: $input) { id title } }"#,
                "variables": { "input": { "id": id, "title": title } },
            }),
            timeout,
        ) {
            Ok(v) => v,
            Err(setup_failed) => return setup_failed,
        };
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
        let result = match self.post_graphql(
            serde_json::json!({
                "query": r#"mutation($id: ID!) { deleteDoogat(id: $id) }"#,
                "variables": { "id": id },
            }),
            timeout,
        ) {
            Ok(v) => v,
            Err(setup_failed) => return setup_failed,
        };
        if let Some(errors) = non_empty_errors(&result) {
            return graphql_err(&errors[0]);
        }
        ConformanceResult::Ok {
            value: ConformanceValue::Null,
            warnings: vec![],
        }
    }

    fn run_list(&self, timeout: Duration) -> ConformanceResult {
        let result = match self.post_graphql(
            serde_json::json!({"query": r#"{ doogats { id title } }"#}),
            timeout,
        ) {
            Ok(v) => v,
            Err(setup_failed) => return setup_failed,
        };
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
        let mut variables = serde_json::Map::new();
        variables.insert("q".into(), query.into());
        match optional_string_list(args, "types") {
            Ok(Some(types)) => {
                variables.insert("types".into(), types.into());
            }
            Ok(None) => {}
            Err(field) => return setup_failed_missing(field),
        }
        if let Some(tag) = optional_string(args, "tag") {
            variables.insert("tag".into(), tag.into());
        }
        // Select all ten core `SearchResult` fields so the value carries the
        // same field set the CLI prints and cross-driver tests can compare
        // hits field by field.
        let result = match self.post_graphql(
            serde_json::json!({
                "query": r#"query($q: String!, $types: [String], $tag: String) { search(query: $q, types: $types, tag: $tag) { hits { id title path snippet rank updated_at tags type fields created_at } totalCount } }"#,
                "variables": variables,
            }),
            timeout,
        ) {
            Ok(v) => v,
            Err(setup_failed) => return setup_failed,
        };
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

    fn run_execute_sql(&self, args: &serde_json::Value, timeout: Duration) -> ConformanceResult {
        let sql = match require_string(args, "sql") {
            Ok(v) => v,
            Err(field) => return setup_failed_missing(field),
        };
        let result = match self.post_graphql(
            serde_json::json!({
                "query": r#"mutation($sql: String!) { executeSql(sql: $sql) { message } }"#,
                "variables": { "sql": sql },
            }),
            timeout,
        ) {
            Ok(v) => v,
            Err(setup_failed) => return setup_failed,
        };
        if let Some(errors) = non_empty_errors(&result) {
            return graphql_err(&errors[0]);
        }
        ConformanceResult::Ok {
            value: ConformanceValue::Null,
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
    result
        .get("errors")
        .and_then(|e| e.as_array())
        .filter(|arr| !arr.is_empty())
}

fn graphql_err(error: &serde_json::Value) -> ConformanceResult {
    let code = error["extensions"]["code"]
        .as_str()
        .unwrap_or("GRAPHQL_ERROR")
        .to_string();
    let message = error["message"].as_str().unwrap_or_default().to_string();
    let context = error["extensions"].as_object().cloned().unwrap_or_default();
    ConformanceResult::Err(ConformanceError {
        code,
        message,
        context,
    })
}
