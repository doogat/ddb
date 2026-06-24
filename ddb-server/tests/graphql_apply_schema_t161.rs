use ddb_server::actor::ActorHandle;
use ddb_server::events::EventBus;
use ddb_server::read_pool::ReadPool;

async fn setup(dir: &std::path::Path) -> (ActorHandle, ReadPool) {
    ddb_core::service::DoogatService::init(dir).expect("init repo");
    let event_bus = EventBus::new();
    let actor = ActorHandle::spawn(dir.to_path_buf(), event_bus).expect("spawn actor");
    let pool = ReadPool::new(dir.to_path_buf(), 1).expect("read pool");
    (actor, pool)
}

/// Desired-schema YAML declaring a brand-new `gizmo` type that does not exist
/// in a freshly-init'd repo, so applying it is a CREATE.
const GIZMO_YAML: &str = "types:\n  - name: gizmo\n    columns:\n      - name: status\n        data_type: VARCHAR(50)\n        zone: frontmatter\n      - name: owner\n        data_type: VARCHAR(100)\n    search_key: status\n";

/// Assert that an `ops` JSON entry carries all five required keys with the
/// right scalar types, per the PlanOpReport contract.
fn assert_op_shape(op: &serde_json::Value) {
    assert!(
        op.get("kind").and_then(|v| v.as_str()).is_some(),
        "op must carry a string `kind`; got: {op}"
    );
    assert!(
        op.get("table").and_then(|v| v.as_str()).is_some(),
        "op must carry a string `table`; got: {op}"
    );
    assert!(
        op.get("detail").and_then(|v| v.as_str()).is_some(),
        "op must carry a string `detail`; got: {op}"
    );
    assert!(
        op.get("destructive").and_then(|v| v.as_bool()).is_some(),
        "op must carry a boolean `destructive`; got: {op}"
    );
    assert!(
        op.get("sql").and_then(|v| v.as_str()).is_some(),
        "op must carry a string `sql`; got: {op}"
    );
}

/// PRD 00161 T6: `applySchema(..., dryRun: true)` must return the full
/// SchemaApplyReport plan (with a `gizmo` op) WITHOUT mutating the repo —
/// `applied` stays false even though a CREATE is planned.
#[tokio::test]
async fn dry_run_apply_returns_plan_without_mutating() {
    let tmp = tempfile::tempdir().unwrap();
    let (actor, pool) = setup(tmp.path()).await;

    let schema =
        ddb_server::schema::build_schema(actor, pool, vec![], None).expect("schema must build");

    // Embed the multi-line YAML as a properly-escaped GraphQL String literal.
    let schema_lit = serde_json::to_string(GIZMO_YAML).unwrap();
    let query = format!(
        r#"mutation {{ applySchema(schema: {schema_lit}, dryRun: true) {{ dryRun applied ops {{ kind table detail destructive sql }} unsupported }} }}"#
    );

    let response = schema.execute(query).await;

    assert!(
        response.errors.is_empty(),
        "expected no GraphQL errors, got: {:?}",
        response.errors
    );

    let data = response.data.clone().into_json().unwrap();
    let report = &data["applySchema"];

    assert_eq!(
        report.get("dryRun").and_then(|v| v.as_bool()),
        Some(true),
        "dry-run report must echo dryRun == true; got: {report}"
    );
    assert_eq!(
        report.get("applied").and_then(|v| v.as_bool()),
        Some(false),
        "dry-run must NOT apply — applied must be false; got: {report}"
    );

    let ops = report
        .get("ops")
        .and_then(|v| v.as_array())
        .unwrap_or_else(|| panic!("ops must be a JSON array; got: {report}"));
    assert!(
        !ops.is_empty(),
        "creating a brand-new gizmo type must plan at least one op; got: {report}"
    );
    assert!(
        ops.iter()
            .any(|op| op.get("table").and_then(|v| v.as_str()) == Some("gizmo")),
        "at least one planned op must target table == 'gizmo'; got: {report}"
    );
    for op in ops {
        assert_op_shape(op);
    }

    assert!(
        report
            .get("unsupported")
            .and_then(|v| v.as_array())
            .is_some(),
        "unsupported must be a JSON array (possibly empty); got: {report}"
    );
}

/// PRD 00161 T6: omitting `dryRun` defaults it to false, so `applySchema`
/// actually creates the `gizmo` type (applied == true, ops non-empty). A second
/// identical call is idempotent — the type already exists, so ops is now empty,
/// which proves the first call really mutated.
#[tokio::test]
async fn real_apply_creates_type_and_is_idempotent() {
    let tmp = tempfile::tempdir().unwrap();
    let (actor, pool) = setup(tmp.path()).await;

    let schema =
        ddb_server::schema::build_schema(actor, pool, vec![], None).expect("schema must build");

    let schema_lit = serde_json::to_string(GIZMO_YAML).unwrap();
    // dryRun is OMITTED — proving it defaults to false.
    let query = format!(
        r#"mutation {{ applySchema(schema: {schema_lit}) {{ dryRun applied ops {{ kind table detail destructive sql }} unsupported }} }}"#
    );

    // First call: real apply.
    let first = schema.execute(query.clone()).await;
    assert!(
        first.errors.is_empty(),
        "first apply: expected no GraphQL errors, got: {:?}",
        first.errors
    );
    let first_data = first.data.clone().into_json().unwrap();
    let first_report = &first_data["applySchema"];

    assert_eq!(
        first_report.get("dryRun").and_then(|v| v.as_bool()),
        Some(false),
        "omitting dryRun must default it to false; got: {first_report}"
    );
    assert_eq!(
        first_report.get("applied").and_then(|v| v.as_bool()),
        Some(true),
        "real apply must set applied == true; got: {first_report}"
    );
    let first_ops = first_report
        .get("ops")
        .and_then(|v| v.as_array())
        .unwrap_or_else(|| panic!("ops must be a JSON array; got: {first_report}"));
    assert!(
        !first_ops.is_empty(),
        "creating gizmo for the first time must produce ops; got: {first_report}"
    );

    // Second call: identical mutation. The type now exists, so there is nothing
    // to change — ops must be empty. This proves the first call mutated and that
    // re-apply is idempotent.
    let second = schema.execute(query).await;
    assert!(
        second.errors.is_empty(),
        "second apply: expected no GraphQL errors, got: {:?}",
        second.errors
    );
    let second_data = second.data.clone().into_json().unwrap();
    let second_report = &second_data["applySchema"];

    assert_eq!(
        second_report.get("applied").and_then(|v| v.as_bool()),
        Some(true),
        "idempotent re-apply must still report applied == true; got: {second_report}"
    );
    let second_ops = second_report
        .get("ops")
        .and_then(|v| v.as_array())
        .unwrap_or_else(|| panic!("ops must be a JSON array; got: {second_report}"));
    assert!(
        second_ops.is_empty(),
        "re-applying an already-applied schema must produce zero ops; got: {second_report}"
    );
}
