use super::fixture::{
    AuthMode, ExpectedBehavior, InterfaceId, SetupExpectation, Step, StepOp, WorkflowFixture,
};
use super::result::{ConformanceError, ConformanceWarning};

/// A client-input rejection both CLI and GraphQL must produce for the same
/// malformed request.
///
/// The original trigger — `CreateDoogat{title:""}` — was not real: ddb accepts
/// an empty title (create succeeds, exit 0), so the expected error never fired
/// and only the result-count assertion exercised the fixture. An empty **search
/// query** is a genuine input rejection reachable through the shared CRUD driver
/// surface: the service rejects it with `DoogatError::BadRequest` ("invalid
/// search query") on every interface.
///
/// Parity here is variant-level (both interfaces return an error), not
/// code-level: CLI structured errors are `Specialized` (text + exit code,
/// decision D-CLI-1), so the CLI driver reports the catch-all `CLI_ERROR` while
/// GraphQL carries the machine-readable `BAD_REQUEST` in `extensions.code`.
/// `cross_driver_validation.rs` asserts both return `Err` and that GraphQL
/// reports this fixture's `expected.error.code`.
pub fn validation_error() -> WorkflowFixture {
    WorkflowFixture {
        id: "validation_error".into(),
        title: "Client rejects malformed input (empty search query)".into(),
        setup: SetupExpectation {
            auth_mode: AuthMode::None,
            timeout_ms: 30_000,
            setup_steps: vec![],
        },
        steps: vec![Step {
            op: StepOp::Search,
            args: serde_json::json!({"query": ""}),
        }],
        expected: ExpectedBehavior {
            value: None,
            warnings: vec![],
            error: Some(ConformanceError {
                code: "BAD_REQUEST".into(),
                message: "invalid search query".into(),
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
        // Full CRUD cycle on a single doogat, plus a live search probe and a
        // stable not-found id for the baseline error-path contract. Value-level
        // ExpectedBehavior enforcement is still deferred, so search is pinned at
        // the variant level by `crud_baseline_no_step_has_variant_mismatch` (all
        // steps) plus the dedicated `crud_baseline_search_returns_ok_on_both_drivers`
        // in `cross_driver_crud.rs`.
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
                op: StepOp::Search,
                args: serde_json::json!({"query": "Test"}),
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

/// Search with `SearchCommand`'s `types` / `tag` filters (PRD 00176).
///
/// Seeds two registered types (so GraphQL's strict typed create accepts them
/// and `type` / `fields` come back non-null) plus one untyped doogat, with
/// overlapping tags, all sharing the search term `filterterm`. Then runs the
/// same search four ways: type only, tag only, type + tag, and unfiltered as
/// the control proving the filters actually narrow the result.
/// `cross_driver_crud.rs` asserts CLI and GraphQL return the same seeded
/// doogats with all ten core `SearchResult` fields equal (modulo per-repo ids
/// and timestamps; see `comparator::anonymize_search_hits`).
pub fn filtered_search() -> WorkflowFixture {
    WorkflowFixture {
        id: "filtered_search".into(),
        title: "Search filtered by type and tag returns the same full hits".into(),
        setup: SetupExpectation {
            auth_mode: AuthMode::None,
            timeout_ms: 30_000,
            setup_steps: vec![],
        },
        steps: vec![
            Step {
                op: StepOp::ExecuteSql,
                args: serde_json::json!({"sql": "CREATE TABLE fs_recipe (cuisine VARCHAR(100))"}),
            },
            Step {
                op: StepOp::ExecuteSql,
                args: serde_json::json!({"sql": "CREATE TABLE fs_dessert (cuisine VARCHAR(100))"}),
            },
            Step {
                op: StepOp::CreateDoogat,
                args: serde_json::json!({
                    "title": "Thai soup", "body": "filterterm soup", "type": "fs_recipe",
                    "tags": ["dinner"], "fields": {"cuisine": "thai"},
                }),
            },
            Step {
                op: StepOp::CreateDoogat,
                args: serde_json::json!({
                    "title": "Lime cake", "body": "filterterm cake", "type": "fs_dessert",
                    "tags": ["dinner", "sweet"], "fields": {"cuisine": "thai"},
                }),
            },
            Step {
                op: StepOp::CreateDoogat,
                args: serde_json::json!({
                    "title": "Plain note", "body": "filterterm note", "tags": ["sweet"],
                }),
            },
            Step {
                op: StepOp::Search,
                args: serde_json::json!({"query": "filterterm", "types": ["fs_recipe"]}),
            },
            Step {
                op: StepOp::Search,
                args: serde_json::json!({"query": "filterterm", "tag": "sweet"}),
            },
            Step {
                op: StepOp::Search,
                args: serde_json::json!({
                    "query": "filterterm", "types": ["fs_dessert"], "tag": "dinner",
                }),
            },
            Step {
                op: StepOp::Search,
                args: serde_json::json!({"query": "filterterm"}),
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

/// Create with native JSON-typed `fields` (PRD 00148 Phase 2, GraphQL leg).
///
/// Registers a type with a BOOLEAN `done` and an INTEGER `priority` column,
/// creates a doogat with `fields: {"done": true, "priority": 3}` as a JSON
/// object (not a JSON-encoded string), then reads the materialized row back
/// with SQLite's `typeof()` so the assertion sees the stored storage class,
/// not a stringified rendering. `cross_driver_crud.rs` asserts `done` is not
/// stored as text and `priority` is an integer.
///
/// GraphQL only: the CLI driver passes fields as `key=value` strings, and the
/// REST leg the PRD names is owned by PRD 00175 (no REST driver exists yet).
pub fn typed_fields_create() -> WorkflowFixture {
    WorkflowFixture {
        id: "typed_fields_create".into(),
        title: "Create with JSON-typed fields stores a native boolean and number".into(),
        setup: SetupExpectation {
            auth_mode: AuthMode::None,
            timeout_ms: 30_000,
            setup_steps: vec![],
        },
        steps: vec![
            Step {
                op: StepOp::ExecuteSql,
                args: serde_json::json!({
                    "sql": "CREATE TABLE tf_task (done BOOLEAN, priority INTEGER)",
                }),
            },
            Step {
                op: StepOp::CreateDoogat,
                args: serde_json::json!({
                    "title": "Typed task", "type": "tf_task",
                    "fields": {"done": true, "priority": 3},
                }),
            },
            Step {
                op: StepOp::ExecuteSql,
                args: serde_json::json!({
                    "sql": "SELECT done, typeof(done), priority, typeof(priority) FROM tf_task",
                }),
            },
        ],
        expected: ExpectedBehavior {
            value: None,
            warnings: vec![],
            error: None,
        },
        interfaces: vec![InterfaceId::Graphql],
    }
}

// PRD 00150 blind-review I-1: pin the GraphQL WarningEntry shape contract.
//
// The release-readiness §3.3 checklist in
// `dev/local/notes/interface-deprecations.md` requires a conformance fixture
// that asserts the `WarningEntry` shape (code + message + structured fields)
// returned on GraphQL for the workflow where a warning fires (inventory
// entry GW-12). This fixture declares that shape contract at the metadata
// level: any future change to the warning shape must be reflected in this
// fixture's `expected.warnings`, and the structural tests below will fail
// if any of the three dimensions (code, message, structured fields) is
// dropped.
//
// Driver execution: deferred. GraphQL warning forwarding is tracked by PRD
// 00154 (graphql-response-extension-warnings-v1); until that lands, drivers
// return empty warnings on this fixture. When PRD 00154 ships and the
// comparator gains value-level enforcement (see deferred-scope in PRD
// 00148), the same fixture activates without modification — the trigger
// `Step` will be replaced with a real warning-emitting mutation and the
// driver output will be compared against `expected.warnings`.
pub fn warnings_shape_contract() -> WorkflowFixture {
    WorkflowFixture {
        id: "warnings_shape_contract".into(),
        title: "Warning shape contract: WarningEntry carries code + message + structured fields"
            .into(),
        setup: SetupExpectation {
            auth_mode: AuthMode::None,
            timeout_ms: 30_000,
            setup_steps: vec![],
        },
        // Placeholder mutation. The fixture is currently read by the
        // structural shape tests, not executed against drivers. When PRD
        // 00154 lands GraphQL warning forwarding, this step is replaced
        // with a real trigger (e.g. typed update with singleton conflict,
        // backlink cascade) and the driver-output comparator will fold
        // `expected.warnings` into per-step classification.
        steps: vec![Step {
            op: StepOp::CreateDoogat,
            args: serde_json::json!({"title": "Placeholder for warning-trigger mutation"}),
        }],
        expected: ExpectedBehavior {
            value: None,
            warnings: vec![ConformanceWarning {
                code: "LIST_ROW_DROPPED".into(),
                message: "list: row decode failed; row skipped".into(),
                fields: serde_json::Map::from_iter([(
                    "path".to_string(),
                    serde_json::Value::String("ddb/20260101000000.md".into()),
                )]),
            }],
            error: None,
        },
        interfaces: vec![InterfaceId::Graphql],
    }
}

/// PRD 00184: index-evolution apply workflow (REST deferred per the PRD's own
/// Transport Impact note until driver 00175 lands; FFI is pinned separately
/// by the `ddb-core` unit test from task 4).
///
/// CREATE a type with no `unique_together`, apply a desired schema adding
/// one (the migration must be reported, not a `SCHEMA_UNSUPPORTED_CHANGE`),
/// re-apply the identical desired schema and assert convergence to a no-op,
/// then DROP the constraint (destructive re-apply of the original
/// no-constraint doc) -- the drop leg BLAKE's task-7 finding named as never
/// exercised. Every step runs through the shared CRUD driver surface (CLI's
/// `schema apply` / GraphQL's `applySchema` mutation) via
/// `StepOp::SchemaApply`, exercised on both drivers by the same fixture per
/// the repo's one-fixture-many-drivers pattern. `cross_driver_schema_apply.rs`
/// asserts `applied`/op-kind value parity per step (not just variant-level
/// Ok/Err) and probes duplicate enforcement before and after the drop.
pub fn index_evolution_migration() -> WorkflowFixture {
    const NO_CONSTRAINT: &str = "\
types:
  - name: ie_widget
    columns:
      - name: ie_label
        data_type: VARCHAR(255)
        zone: frontmatter
        required: true
";
    const WITH_CONSTRAINT: &str = "\
types:
  - name: ie_widget
    columns:
      - name: ie_label
        data_type: VARCHAR(255)
        zone: frontmatter
        required: true
    unique_together: [[ie_label]]
";
    WorkflowFixture {
        id: "index_evolution_migration".into(),
        title: "Adding unique_together to an existing type migrates, re-apply converges".into(),
        setup: SetupExpectation {
            auth_mode: AuthMode::None,
            timeout_ms: 30_000,
            setup_steps: vec![],
        },
        steps: vec![
            Step {
                op: StepOp::SchemaApply,
                args: serde_json::json!({"schema": NO_CONSTRAINT}),
            },
            Step {
                op: StepOp::SchemaApply,
                args: serde_json::json!({"schema": WITH_CONSTRAINT}),
            },
            Step {
                op: StepOp::SchemaApply,
                args: serde_json::json!({"schema": WITH_CONSTRAINT}),
            },
            Step {
                op: StepOp::SchemaApply,
                args: serde_json::json!({"schema": NO_CONSTRAINT, "allow_destructive": true}),
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

/// PRD 00184 task 7 (BLAKE, BOB): a sibling of `index_evolution_migration`
/// that probes what that fixture's "every step is Ok" assertion cannot --
/// duplicate enforcement is ACTUALLY live after the migration (step 3 must
/// error), and dropping the constraint ACTUALLY lifts it (step 5 must
/// succeed, not merely report `drop_unique_constraint` in the plan). Kept
/// separate from `index_evolution_migration` because its middle step is
/// expected to fail, which would break that fixture's own all-succeed
/// invariant.
pub fn index_evolution_duplicate_enforcement_and_drop() -> WorkflowFixture {
    const NO_CONSTRAINT: &str = "\
types:
  - name: ie_widget
    columns:
      - name: ie_label
        data_type: VARCHAR(255)
        zone: frontmatter
        required: true
";
    const WITH_CONSTRAINT: &str = "\
types:
  - name: ie_widget
    columns:
      - name: ie_label
        data_type: VARCHAR(255)
        zone: frontmatter
        required: true
    unique_together: [[ie_label]]
";
    WorkflowFixture {
        id: "index_evolution_duplicate_enforcement_and_drop".into(),
        title: "unique_together rejects duplicates post-migration, drop lifts it".into(),
        setup: SetupExpectation {
            auth_mode: AuthMode::None,
            timeout_ms: 30_000,
            setup_steps: vec![],
        },
        steps: vec![
            // 0: create the type with no constraint.
            Step {
                op: StepOp::SchemaApply,
                args: serde_json::json!({"schema": NO_CONSTRAINT}),
            },
            // 1: seed one row.
            Step {
                op: StepOp::ExecuteSql,
                args: serde_json::json!({
                    "sql": "INSERT INTO ie_widget (ie_label) VALUES ('dup-probe')"
                }),
            },
            // 2: migrate -- add unique_together over ie_label.
            Step {
                op: StepOp::SchemaApply,
                args: serde_json::json!({"schema": WITH_CONSTRAINT}),
            },
            // 3: a duplicate insert must now be REJECTED on both drivers.
            Step {
                op: StepOp::ExecuteSql,
                args: serde_json::json!({
                    "sql": "INSERT INTO ie_widget (ie_label) VALUES ('dup-probe')"
                }),
            },
            // 4: drop the constraint (destructive).
            Step {
                op: StepOp::SchemaApply,
                args: serde_json::json!({"schema": NO_CONSTRAINT, "allow_destructive": true}),
            },
            // 5: the same duplicate insert must now SUCCEED -- the drop
            // actually lifted enforcement, not just the typedef record.
            Step {
                op: StepOp::ExecuteSql,
                args: serde_json::json!({
                    "sql": "INSERT INTO ie_widget (ie_label) VALUES ('dup-probe')"
                }),
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

    #[test]
    fn steps_contains_live_search_probe() {
        let fixture = crud_baseline();
        let search_idx = fixture
            .steps
            .iter()
            .position(|s| s.op == StepOp::Search)
            .expect("crud_baseline has a Search step");
        let delete_idx = fixture
            .steps
            .iter()
            .position(|s| s.op == StepOp::DeleteDoogat)
            .expect("crud_baseline has a DeleteDoogat step");

        assert!(search_idx < delete_idx);
        assert_eq!(
            fixture.steps[search_idx]
                .args
                .get("query")
                .and_then(|v| v.as_str()),
            Some("Test")
        );
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
    fn validation_error_step_is_empty_search() {
        // The real trigger is an empty search query (rejected as BadRequest on
        // every interface); empty-title create is accepted, so it cannot be it.
        let fixture = validation_error();
        let step = &fixture.steps[0];
        assert_eq!(step.op, StepOp::Search);
        assert_eq!(step.args.get("query").and_then(|v| v.as_str()), Some(""));
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

    // PRD 00150 blind-review I-1: assert the warnings_shape_contract fixture
    // pins the WarningEntry shape on all three dimensions required by the
    // release-readiness §3.3 checklist: code, message, structured fields.

    #[test]
    fn warnings_shape_contract_declares_exactly_one_warning() {
        let fixture = warnings_shape_contract();
        assert_eq!(
            fixture.expected.warnings.len(),
            1,
            "shape contract pins a single WarningEntry exemplar"
        );
    }

    #[test]
    fn warnings_shape_contract_warning_has_non_empty_code() {
        let fixture = warnings_shape_contract();
        let warning = &fixture.expected.warnings[0];
        assert!(
            !warning.code.is_empty(),
            "WarningEntry must carry a non-empty stable code (release-readiness §3.3)"
        );
    }

    #[test]
    fn warnings_shape_contract_warning_has_non_empty_message() {
        let fixture = warnings_shape_contract();
        let warning = &fixture.expected.warnings[0];
        assert!(
            !warning.message.is_empty(),
            "WarningEntry must carry a non-empty human-readable message (release-readiness §3.3)"
        );
    }

    #[test]
    fn warnings_shape_contract_warning_has_non_empty_structured_fields() {
        let fixture = warnings_shape_contract();
        let warning = &fixture.expected.warnings[0];
        assert!(
            !warning.fields.is_empty(),
            "WarningEntry must carry structured fields (release-readiness §3.3); empty fields \
             would silently allow the shape to regress to code+message-only"
        );
    }

    #[test]
    fn warnings_shape_contract_targets_graphql_interface() {
        let fixture = warnings_shape_contract();
        assert!(
            fixture.interfaces.contains(&InterfaceId::Graphql),
            "release-readiness §3.3 names GraphQL extensions.warnings as the only Guaranteed \
             warning channel; the shape contract must target GraphQL"
        );
    }

    #[test]
    fn warnings_shape_contract_id_is_stable() {
        let fixture = warnings_shape_contract();
        assert_eq!(fixture.id, "warnings_shape_contract");
    }
}
