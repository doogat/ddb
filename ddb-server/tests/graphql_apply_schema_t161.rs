use std::sync::Arc;

use ddb_server::actor::ActorHandle;
use ddb_server::events::EventBus;
use ddb_server::read_pool::ReadPool;
use ddb_server::reload::SchemaReloader;

async fn setup(dir: &std::path::Path) -> (ActorHandle, ReadPool) {
    ddb_core::service::DoogatService::init(dir).expect("init repo");
    let event_bus = EventBus::new();
    let actor = ActorHandle::spawn(
        dir.to_path_buf(),
        event_bus,
        ddb_core::git_ops::DEFAULT_REMOTE_TIMEOUT,
    )
    .expect("spawn actor");
    let pool = ReadPool::new(dir.to_path_buf(), 1).expect("read pool");
    (actor, pool)
}

/// Desired-schema YAML declaring a brand-new `gizmo` type that does not exist
/// in a freshly-init'd repo, so applying it is a CREATE.
const GIZMO_YAML: &str = "types:\n  - name: gizmo\n    columns:\n      - name: status\n        data_type: VARCHAR(50)\n        zone: frontmatter\n      - name: owner\n        data_type: VARCHAR(100)\n    search_key: status\n";

/// Second desired-schema YAML declaring a DIFFERENT type `widget`. Used to prove
/// the apply report is derived from the actual input doc, not a hardcoded name.
const WIDGET_YAML: &str = "types:\n  - name: widget\n    columns:\n      - name: label\n        data_type: VARCHAR(30)\n        zone: frontmatter\n";

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

/// True if `op`'s `sql` field, lowercased, contains `needle`. Lets parse-binding
/// assertions check that the emitted SQL mentions the declared type name.
fn op_sql_contains(op: &serde_json::Value, needle: &str) -> bool {
    op.get("sql")
        .and_then(|v| v.as_str())
        .map(|s| s.to_lowercase().contains(needle))
        .unwrap_or(false)
}

/// PRD 00161 T6: `applySchema(..., dryRun: true)` must return the full
/// SchemaApplyReport plan (with a `gizmo` op whose SQL names gizmo) WITHOUT
/// mutating the repo. `applied` stays false, and the `gizmo` table must NOT
/// exist afterward — proven by a SELECT that fails on a missing table.
#[tokio::test]
async fn dry_run_apply_returns_plan_without_mutating() {
    let tmp = tempfile::tempdir().unwrap();
    let (actor, pool) = setup(tmp.path()).await;

    // Keep a handle for raw-SQL probes against the SAME repo before build_schema
    // moves the actor.
    let probe = actor.clone();
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
    let probed = probe
        .execute_sql("SELECT * FROM gizmo".to_string())
        .await;
    assert!(
        probed.is_err(),
        "dry-run must NOT create the gizmo table; SELECT against it should fail but it succeeded"
    );
}

/// PRD 00161 T6: omitting `dryRun` defaults it to false, so `applySchema`
/// actually creates the `gizmo` type. Real persistence is proven by selecting
/// the declared columns from the live table. A second identical call is
/// idempotent — the type already exists, so ops is now empty, and the table
/// still exists.
#[tokio::test]
async fn real_apply_creates_type_and_is_idempotent() {
    let tmp = tempfile::tempdir().unwrap();
    let (actor, pool) = setup(tmp.path()).await;

    let probe = actor.clone();
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

    // PROVE real persistence: the table with its declared columns must really
    // exist, so selecting `status` and `owner` from it must succeed.
    let after_first = probe
        .execute_sql("SELECT status, owner FROM gizmo".to_string())
        .await;
    assert!(
        after_first.is_ok(),
        "real apply must create the gizmo table with its declared columns; SELECT status, owner FROM gizmo failed"
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

    // An idempotent re-apply has an EMPTY plan, so `applied` is false: the core
    // contract defines `applied` as "ops were executed on this call", not "the
    // desired state holds" (see ddb-core apply_schema: empty plan => applied=false,
    // pinned by its own `reapply_same_doc_is_idempotent_noop` test). The real
    // idempotency signal is the empty `ops` below plus the table still existing.
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

/// PRD 00161 T6: the apply report must be derived from the actual input doc, not
/// a hardcoded type name. Applying a doc that declares `widget` must produce an
/// op targeting `widget` (NOT `gizmo`) whose SQL names widget — killing any
/// hardcoded-"gizmo" resolver.
#[tokio::test]
async fn apply_report_reflects_declared_type_name() {
    let tmp = tempfile::tempdir().unwrap();
    let (actor, pool) = setup(tmp.path()).await;

    let schema =
        ddb_server::schema::build_schema(actor, pool, vec![], None).expect("schema must build");

    let schema_lit = serde_json::to_string(WIDGET_YAML).unwrap();
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
}

/// PRD 00161 blind-review IMPORTANT-1: a real (non-dry-run) `applySchema` that
/// creates a typedef MUST trigger a dynamic-GraphQL-schema reload — exactly as
/// `executeSql`/`executeBatch` do after DDL — so the newly declared type surfaces
/// in the dynamic schema without a restart. A `dryRun` apply mutates nothing and
/// must NOT reload. The reload is observed via `SchemaReloader::version()`, which
/// increments only on a completed reload.
#[tokio::test]
async fn real_apply_triggers_schema_reload_dry_run_does_not() {
    let tmp = tempfile::tempdir().unwrap();
    let (actor, pool) = setup(tmp.path()).await;

    // The reloader owns its own actor/pool clones for the background reload loop.
    // The reload decision now lives inside ActorHandle::call (PRD 00172 task 4),
    // so the actor itself must be wired with the reloader, matching lib.rs.
    let (reloader, _shared) = SchemaReloader::new(actor.clone(), pool.clone());
    actor.set_reloader(Arc::clone(&reloader));
    let schema = ddb_server::schema::build_schema(actor, pool, vec![], Some(Arc::clone(&reloader)))
        .expect("schema must build");

    let schema_lit = serde_json::to_string(GIZMO_YAML).unwrap();

    // Dry-run: plan only, no mutation, so NO reload may fire.
    let dry_query =
        format!(r#"mutation {{ applySchema(schema: {schema_lit}, dryRun: true) {{ applied }} }}"#);
    let version_before_dry = reloader.version();
    let dry = schema.execute(dry_query).await;
    assert!(
        dry.errors.is_empty(),
        "dry-run apply: expected no GraphQL errors, got: {:?}",
        dry.errors
    );
    assert_eq!(
        reloader.version(),
        version_before_dry,
        "a dry-run applySchema mutates nothing and must NOT trigger a schema reload"
    );

    // Real apply: creating `gizmo` must trigger a reload so the type is queryable
    // through the dynamic schema in this same server instance.
    let real_query = format!(r#"mutation {{ applySchema(schema: {schema_lit}) {{ applied }} }}"#);
    let version_before_real = reloader.version();
    let real = schema.execute(real_query).await;
    assert!(
        real.errors.is_empty(),
        "real apply: expected no GraphQL errors, got: {:?}",
        real.errors
    );
    assert_eq!(
        real.data.clone().into_json().unwrap()["applySchema"]["applied"].as_bool(),
        Some(true),
        "real apply must set applied == true"
    );
    assert!(
        reloader.version() > version_before_real,
        "a real (non-dry-run) applySchema that creates a typedef must trigger a schema reload \
         (version must increment); before={version_before_real} after={}",
        reloader.version()
    );
}

/// PRD 00172 task 4: `operations.rs`'s own hand-wired reload trigger after
/// `executeSql` was deleted — the reload decision now lives inside
/// `ActorHandle::call` (via `ReloadIntent::Explicit`, pre-classified from the
/// SQL text). A DDL statement must still trigger exactly one reload; a non-DDL
/// statement must trigger none.
#[tokio::test]
async fn execute_sql_ddl_triggers_schema_reload_dml_does_not() {
    let tmp = tempfile::tempdir().unwrap();
    let (actor, pool) = setup(tmp.path()).await;

    let (reloader, _shared) = SchemaReloader::new(actor.clone(), pool.clone());
    actor.set_reloader(Arc::clone(&reloader));
    let schema = ddb_server::schema::build_schema(actor, pool, vec![], Some(Arc::clone(&reloader)))
        .expect("schema must build");

    let ddl = r#"mutation { executeSql(sql: "CREATE TABLE sqlddltest (label TEXT)") { message } }"#;
    let version_before_ddl = reloader.version();
    let result = schema.execute(ddl).await;
    assert!(
        result.errors.is_empty(),
        "executeSql DDL: expected no GraphQL errors, got: {:?}",
        result.errors
    );
    assert!(
        reloader.version() > version_before_ddl,
        "a CREATE TABLE via executeSql must trigger a schema reload (version must increment); \
         before={version_before_ddl} after={}",
        reloader.version()
    );

    let dml = r#"mutation { executeSql(sql: "INSERT INTO sqlddltest (label) VALUES ('x')") { message affected } }"#;
    let version_before_dml = reloader.version();
    let result = schema.execute(dml).await;
    assert!(
        result.errors.is_empty(),
        "executeSql DML: expected no GraphQL errors, got: {:?}",
        result.errors
    );
    assert_eq!(
        reloader.version(),
        version_before_dml,
        "an INSERT via executeSql must NOT trigger a schema reload"
    );
}

/// PRD 00172 task 4: same guard for `executeBatch`, whose reload trigger is
/// classified across every statement in the batch (`.any(requires_schema_reload)`)
/// rather than a single statement.
#[tokio::test]
async fn execute_batch_with_ddl_triggers_schema_reload_pure_dml_does_not() {
    let tmp = tempfile::tempdir().unwrap();
    let (actor, pool) = setup(tmp.path()).await;

    let (reloader, _shared) = SchemaReloader::new(actor.clone(), pool.clone());
    actor.set_reloader(Arc::clone(&reloader));
    let schema = ddb_server::schema::build_schema(actor, pool, vec![], Some(Arc::clone(&reloader)))
        .expect("schema must build");

    let batch_with_ddl = r#"mutation { executeBatch(statements: [
        "CREATE TABLE batchddltest (label TEXT)",
        "INSERT INTO batchddltest (label) VALUES ('x')"
    ]) { message } }"#;
    let version_before_ddl = reloader.version();
    let result = schema.execute(batch_with_ddl).await;
    assert!(
        result.errors.is_empty(),
        "executeBatch with DDL: expected no GraphQL errors, got: {:?}",
        result.errors
    );
    assert!(
        reloader.version() > version_before_ddl,
        "a batch containing CREATE TABLE must trigger a schema reload (version must increment); \
         before={version_before_ddl} after={}",
        reloader.version()
    );

    let pure_dml_batch = r#"mutation { executeBatch(statements: [
        "INSERT INTO batchddltest (label) VALUES ('y')",
        "INSERT INTO batchddltest (label) VALUES ('z')"
    ]) { message } }"#;
    let version_before_dml = reloader.version();
    let result = schema.execute(pure_dml_batch).await;
    assert!(
        result.errors.is_empty(),
        "executeBatch pure DML: expected no GraphQL errors, got: {:?}",
        result.errors
    );
    assert_eq!(
        reloader.version(),
        version_before_dml,
        "a batch with no DDL must NOT trigger a schema reload"
    );
}
