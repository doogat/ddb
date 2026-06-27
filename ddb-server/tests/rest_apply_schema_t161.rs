use std::sync::Arc;

use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use ddb_server::actor::ActorHandle;
use ddb_server::events::EventBus;
use ddb_server::read_pool::ReadPool;
use ddb_server::reload::SchemaReloader;
use tower::ServiceExt; // for `oneshot`

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

/// Second desired-schema YAML declaring a DIFFERENT type `widget`. Used to prove
/// the apply report is derived from the actual input doc, not a hardcoded name.
const WIDGET_YAML: &str = "types:\n  - name: widget\n    columns:\n      - name: label\n        data_type: VARCHAR(30)\n        zone: frontmatter\n";

/// Build the REST app under test with the actor injected via the `Extension`
/// layer the apply handler reads from.
fn app(actor: ActorHandle) -> axum::Router {
    ddb_server::rest::router().layer(axum::Extension(actor))
}

/// POST `body` to `/schema/apply` on a fresh clone of `app` (oneshot consumes
/// the service), returning the HTTP status and the parsed JSON body.
async fn post_apply(
    app: &axum::Router,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/schema/apply")
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    (status, json)
}

/// POST `body` to the production-mounted `/rest/schema/apply` path on a fresh
/// clone of `app` (oneshot consumes the service). Mirrors [`post_apply`] but
/// targets the real nested URL so the test exercises the `/rest` mount.
async fn post_rest_apply(
    app: &axum::Router,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/rest/schema/apply")
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    (status, json)
}

/// Assert that an `ops` JSON entry carries all five required keys with the
/// right scalar types, per the report op contract.
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

/// True if `op`'s `sql` field, lowercased, contains `needle`. Lets parse-binding
/// assertions check that the emitted SQL mentions the declared type name.
fn op_sql_contains(op: &serde_json::Value, needle: &str) -> bool {
    op.get("sql")
        .and_then(|v| v.as_str())
        .map(|s| s.to_lowercase().contains(needle))
        .unwrap_or(false)
}

/// PRD 00161 T7 (REST): `POST /schema/apply` with `dryRun: true` must return the
/// full report plan (with a `gizmo` op whose SQL names gizmo) WITHOUT mutating
/// the repo. `data.applied` stays false, and the `gizmo` table must NOT exist
/// afterward — proven by a SELECT that fails on a missing table.
#[tokio::test]
async fn dry_run_apply_returns_plan_without_mutating() {
    let tmp = tempfile::tempdir().unwrap();
    let (actor, _pool) = setup(tmp.path()).await;

    // Keep a handle for raw-SQL probes against the SAME repo; the actor is moved
    // into the router layer.
    let probe = actor.clone();
    let app = app(actor);

    let (status, json) =
        post_apply(&app, serde_json::json!({ "schema": GIZMO_YAML, "dryRun": true })).await;

    assert_eq!(status, StatusCode::OK, "dry-run must return 200; body: {json}");

    // The envelope: `warnings` is always an array, and `data` is the report.
    assert!(
        json.get("warnings").and_then(|v| v.as_array()).is_some(),
        "response envelope must carry a `warnings` array; got: {json}"
    );
    let report = &json["data"];

    assert_eq!(
        report.get("dry_run").and_then(|v| v.as_bool()),
        Some(true),
        "dry-run report must echo dry_run == true; got: {report}"
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
        ops.iter().any(|op| {
            op.get("table").and_then(|v| v.as_str()) == Some("gizmo")
                && op_sql_contains(op, "gizmo")
        }),
        "at least one planned op must target table == 'gizmo' with SQL naming gizmo; got: {report}"
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

    // PROVE no mutation by observation: a dry-run must NOT have created the
    // table, so selecting from it must fail (`no such table`).
    let probed = probe.execute_sql("SELECT * FROM gizmo".to_string()).await;
    assert!(
        probed.is_err(),
        "dry-run must NOT create the gizmo table; SELECT against it should fail but it succeeded"
    );
}

/// PRD 00161 T7 (REST): omitting `dryRun` defaults it to false, so the endpoint
/// actually creates the `gizmo` type. Real persistence is proven by selecting
/// the declared columns from the live table. A second identical call is
/// idempotent — the type already exists, so ops is now empty, `applied` is
/// false, and the table still exists.
#[tokio::test]
async fn real_apply_creates_type_and_is_idempotent() {
    let tmp = tempfile::tempdir().unwrap();
    let (actor, _pool) = setup(tmp.path()).await;

    let probe = actor.clone();
    let app = app(actor);

    // First call: real apply. `dryRun` is OMITTED — proving it defaults to false.
    let (status, first) = post_apply(&app, serde_json::json!({ "schema": GIZMO_YAML })).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "first apply must return 200; body: {first}"
    );
    let first_report = &first["data"];

    assert_eq!(
        first_report.get("dry_run").and_then(|v| v.as_bool()),
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

    // PROVE real persistence: the table with its declared columns must really
    // exist, so selecting `status` and `owner` from it must succeed.
    let after_first = probe
        .execute_sql("SELECT status, owner FROM gizmo".to_string())
        .await;
    assert!(
        after_first.is_ok(),
        "real apply must create the gizmo table with its declared columns; SELECT status, owner FROM gizmo failed"
    );

    // Second call: identical body. The type now exists, so there is nothing to
    // change — ops must be empty and applied must be false. This proves the first
    // call mutated and that re-apply is idempotent.
    let (status, second) = post_apply(&app, serde_json::json!({ "schema": GIZMO_YAML })).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "second apply must return 200; body: {second}"
    );
    let second_report = &second["data"];

    // An idempotent re-apply has an EMPTY plan, so `applied` is false: the core
    // contract defines `applied` as "ops were executed on this call", not "the
    // desired state holds". The real idempotency signal is the empty `ops` plus
    // the table still existing.
    assert_eq!(
        second_report.get("applied").and_then(|v| v.as_bool()),
        Some(false),
        "idempotent re-apply executes no ops, so applied == false; got: {second_report}"
    );
    let second_ops = second_report
        .get("ops")
        .and_then(|v| v.as_array())
        .unwrap_or_else(|| panic!("ops must be a JSON array; got: {second_report}"));
    assert!(
        second_ops.is_empty(),
        "re-applying an already-applied schema must produce zero ops; got: {second_report}"
    );

    // After the idempotent re-apply, the table must STILL exist with its columns.
    let after_second = probe
        .execute_sql("SELECT status, owner FROM gizmo".to_string())
        .await;
    assert!(
        after_second.is_ok(),
        "idempotent re-apply must leave the gizmo table intact; SELECT status, owner FROM gizmo failed afterward"
    );
}

/// PRD 00161 T7 (REST): the apply report must be derived from the actual input
/// doc, not a hardcoded type name. Applying a doc that declares `widget` must
/// produce an op targeting `widget` (NOT `gizmo`) whose SQL names widget —
/// killing any hardcoded-"gizmo" resolver.
#[tokio::test]
async fn apply_report_reflects_declared_type_name() {
    let tmp = tempfile::tempdir().unwrap();
    let (actor, _pool) = setup(tmp.path()).await;
    let app = app(actor);

    let (status, json) =
        post_apply(&app, serde_json::json!({ "schema": WIDGET_YAML, "dryRun": true })).await;

    assert_eq!(status, StatusCode::OK, "dry-run must return 200; body: {json}");
    let report = &json["data"];

    let ops = report
        .get("ops")
        .and_then(|v| v.as_array())
        .unwrap_or_else(|| panic!("ops must be a JSON array; got: {report}"));
    assert!(
        !ops.is_empty(),
        "creating a brand-new widget type must plan at least one op; got: {report}"
    );
    assert!(
        ops.iter().any(|op| {
            op.get("table").and_then(|v| v.as_str()) == Some("widget")
                && op_sql_contains(op, "widget")
        }),
        "at least one planned op must target table == 'widget' with SQL naming widget — report must reflect the input doc, not a hardcoded name; got: {report}"
    );
    assert!(
        !ops.iter()
            .any(|op| op.get("table").and_then(|v| v.as_str()) == Some("gizmo")),
        "no op may target 'gizmo' for a widget-only doc; got: {report}"
    );
}

/// PRD 00161 (REST, Contracts A+B): a real (non-dry-run) `POST /rest/schema/apply`
/// that actually applies (`applied == true`) MUST trigger the dynamic-schema
/// reload — exactly as the GraphQL `applySchema` mutation does — so a newly
/// declared type surfaces in this same server instance without a restart. A
/// dry-run apply mutates nothing and must NOT reload. The reload is observed via
/// `SchemaReloader::version()`, which increments only on a completed reload. The
/// REST router is nested under `/rest`, so this also pins the production mount
/// path `/rest/schema/apply` (Contract B).
#[tokio::test]
async fn real_apply_triggers_schema_reload_dry_run_does_not() {
    let tmp = tempfile::tempdir().unwrap();
    let (actor, pool) = setup(tmp.path()).await;

    // The reloader owns its own actor/pool clones for the background reload loop;
    // the router registers it as an Extension so the apply handler can find it.
    let (reloader, _shared) = SchemaReloader::new(actor.clone(), pool.clone());
    let app = axum::Router::new()
        .nest("/rest", ddb_server::rest::router())
        .layer(axum::Extension(actor))
        .layer(axum::Extension(Arc::clone(&reloader)));

    // Dry-run: plan only, no mutation, so NO reload may fire.
    let version_before_dry = reloader.version();
    let (dry_status, dry_json) =
        post_rest_apply(&app, serde_json::json!({ "schema": GIZMO_YAML, "dryRun": true })).await;
    assert_eq!(
        dry_status,
        StatusCode::OK,
        "dry-run apply must return 200; body: {dry_json}"
    );
    assert_eq!(
        reloader.version(),
        version_before_dry,
        "a dry-run apply mutates nothing and must NOT trigger a schema reload"
    );

    // Real apply: creating `gizmo` must trigger a reload so the type becomes
    // queryable through the dynamic schema in this same server instance.
    let version_before_real = reloader.version();
    let (real_status, real_json) =
        post_rest_apply(&app, serde_json::json!({ "schema": GIZMO_YAML })).await;
    assert_eq!(
        real_status,
        StatusCode::OK,
        "real apply must return 200; body: {real_json}"
    );
    assert_eq!(
        real_json["data"].get("applied").and_then(|v| v.as_bool()),
        Some(true),
        "real apply must set applied == true; got: {real_json}"
    );
    assert!(
        reloader.version() > version_before_real,
        "a real (non-dry-run) apply that creates a typedef must trigger a schema reload \
         (version must increment); before={version_before_real} after={}",
        reloader.version()
    );
}
