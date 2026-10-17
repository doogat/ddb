use super::*;
use crate::events::EventKind;
use crate::read_pool::ReadPool;
use crate::reload::SchemaReloader;
use ddb_core::app_contract::{ApplySchemaCommand, CreateCommand, UnregisteredTypePolicy};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::broadcast;

// ReloadIntent / ReloadSignal: the reload check `call` runs after a verb.
//
// A real `SchemaReloader` is registered on the actor and observed through its
// public `version()`, which grows by exactly one per completed reload. Every
// reload assertion waits (bounded) for the expected count, then lets the system
// settle so a stray extra or late reload is still caught.

/// Build a real `SchemaReloader` over `actor` and register it via `set_reloader`.
fn register_reloader(dir: &std::path::Path, actor: &ActorHandle) -> Arc<SchemaReloader> {
    let pool = ReadPool::new(dir.to_path_buf(), 1).unwrap();
    let (reloader, _shared) = SchemaReloader::new(actor.clone(), pool);
    actor.set_reloader(reloader.clone());
    reloader
}

/// Assert exactly `expected` reloads happened since `before` was sampled.
async fn assert_reloads(reloader: &SchemaReloader, before: u64, expected: u64) {
    let target = before + expected;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    while reloader.version() < target && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        reloader.version(),
        target,
        "expected exactly {expected} schema reload(s)"
    );
}

type ApplyResult = ActorResult<AppOutput<SchemaApplyReport>>;

fn apply(
    schema: &str,
    dry_run: bool,
) -> impl FnOnce(&mut DoogatService) -> ApplyResult + Send + 'static {
    let schema_doc = schema.to_string();
    move |svc| {
        svc.apply_schema(ApplySchemaCommand {
            schema_doc,
            dry_run,
            allow_destructive: false,
        })
    }
}

fn create_cmd(title: &str) -> CreateCommand {
    CreateCommand {
        title: Some(title.into()),
        body: Some("body".into()),
        tags: vec![],
        doogat_type: None,
        fields: std::collections::BTreeMap::new(),
        on_conflict: ConflictAction::default(),
        unregistered_type_policy: UnregisteredTypePolicy::Strict,
    }
}

fn failing_read() -> ActorResult<Vec<TableSchema>> {
    Err(DoogatError::Validation("closure failed".into()))
}

fn failing_sql() -> ActorResult<SqlResult> {
    Err(DoogatError::Validation("closure failed".into()))
}

fn has_item(schemas: &[TableSchema]) -> bool {
    schemas.iter().any(|s| s.table_name == "item")
}

#[tokio::test]
async fn none_intent_read_returns_value_without_event_or_reload() {
    tokio::time::timeout(Duration::from_secs(30), async {
        let tmp = tempfile::tempdir().unwrap();
        let bus = EventBus::new();
        let actor = spawn_actor(tmp.path(), bus.clone());
        let setup = actor.call(EventIntent::None, ReloadIntent::None, apply(ITEM_SCHEMA, false));
        assert!(setup.await.unwrap().value.applied);
        let reloader = register_reloader(tmp.path(), &actor);
        let before = reloader.version();
        let mut rx = bus.subscribe();

        let schemas = actor
            .call(EventIntent::None, ReloadIntent::None, |svc| svc.list_type_schemas())
            .await
            .unwrap();

        assert!(has_item(&schemas), "read must return the closure's schema list");
        assert!(matches!(rx.try_recv(), Err(broadcast::error::TryRecvError::Empty)));
        assert_reloads(&reloader, before, 0).await;
    })
    .await
    .expect("none-intent read test timed out");
}

#[tokio::test]
async fn none_intent_mutation_returns_value_and_event_without_reload() {
    tokio::time::timeout(Duration::from_secs(30), async {
        let tmp = tempfile::tempdir().unwrap();
        let bus = EventBus::new();
        let actor = spawn_actor(tmp.path(), bus.clone());
        let reloader = register_reloader(tmp.path(), &actor);
        let before = reloader.version();
        let mut rx = bus.subscribe();

        let output = actor
            .call(EventIntent::Created, ReloadIntent::None, |svc| {
                svc.create(create_cmd("no reload"))
            })
            .await
            .unwrap();

        assert_eq!(output.value.meta.title.as_deref(), Some("no reload"));
        let created_id = output.value.meta.id.as_ref().map(ToString::to_string);
        let event = rx.recv().await.expect("expected one Created event");
        assert_eq!(event.kind, EventKind::Created);
        assert_eq!(Some(event.doogat_id), created_id);
        assert!(matches!(rx.try_recv(), Err(broadcast::error::TryRecvError::Empty)));
        assert_reloads(&reloader, before, 0).await;
    })
    .await
    .expect("none-intent mutation test timed out");
}

#[tokio::test]
async fn explicit_true_reloads_once_on_ok() {
    tokio::time::timeout(Duration::from_secs(30), async {
        let tmp = tempfile::tempdir().unwrap();
        let actor = spawn_actor(tmp.path(), EventBus::new());
        let reloader = register_reloader(tmp.path(), &actor);
        let before = reloader.version();

        // Explicit(true) has teeth only for SqlResult/Vec<SqlResult> (the
        // caller classified the SQL text itself); every other result type
        // never reloads under this intent, whatever its value.
        let result = actor
            .call(EventIntent::None, ReloadIntent::Explicit(true), |svc| {
                svc.execute_sql("SELECT 1")
            })
            .await
            .unwrap();

        assert_eq!(reloader.version(), before + 1, "call must await its reload");
        assert!(matches!(result, SqlResult::Rows { .. }), "call must return the closure's value");
        assert_reloads(&reloader, before, 1).await;
    })
    .await
    .expect("explicit(true) ok test timed out");
}

#[tokio::test]
async fn explicit_true_skips_reload_on_err() {
    tokio::time::timeout(Duration::from_secs(30), async {
        let tmp = tempfile::tempdir().unwrap();
        let actor = spawn_actor(tmp.path(), EventBus::new());
        let reloader = register_reloader(tmp.path(), &actor);
        let before = reloader.version();

        let result = actor
            .call(EventIntent::None, ReloadIntent::Explicit(true), |_svc| failing_sql())
            .await;

        assert!(
            matches!(&result, Err(DoogatError::Validation(m)) if m == "closure failed"),
            "call must return the closure's own error"
        );
        assert_reloads(&reloader, before, 0).await;
    })
    .await
    .expect("explicit(true) err test timed out");
}

#[tokio::test]
async fn explicit_false_never_reloads() {
    tokio::time::timeout(Duration::from_secs(30), async {
        let tmp = tempfile::tempdir().unwrap();
        let actor = spawn_actor(tmp.path(), EventBus::new());
        let setup = actor.call(EventIntent::None, ReloadIntent::None, apply(ITEM_SCHEMA, false));
        assert!(setup.await.unwrap().value.applied);
        let reloader = register_reloader(tmp.path(), &actor);
        let before = reloader.version();

        let ok = actor
            .call(EventIntent::None, ReloadIntent::Explicit(false), |svc| {
                svc.list_type_schemas()
            })
            .await
            .unwrap();
        assert!(has_item(&ok));
        let err = actor
            .call(EventIntent::None, ReloadIntent::Explicit(false), |_svc| failing_sql())
            .await;
        assert!(err.is_err());
        let err = actor
            .call(EventIntent::None, ReloadIntent::Explicit(false), |_svc| failing_read())
            .await;
        assert!(err.is_err());

        assert_reloads(&reloader, before, 0).await;
    })
    .await
    .expect("explicit(false) test timed out");
}

#[tokio::test]
async fn schema_apply_reloads_once_when_applied() {
    tokio::time::timeout(Duration::from_secs(30), async {
        let tmp = tempfile::tempdir().unwrap();
        let actor = spawn_actor(tmp.path(), EventBus::new());
        let reloader = register_reloader(tmp.path(), &actor);
        let before = reloader.version();

        let output = actor
            .call(
                EventIntent::None,
                ReloadIntent::SchemaApply { dry_run: false },
                apply(ITEM_SCHEMA, false),
            )
            .await
            .unwrap();

        assert_eq!(reloader.version(), before + 1, "call must await its reload");
        assert!(output.value.applied, "a new type must be applied");
        assert!(!output.value.ops.is_empty());
        assert_reloads(&reloader, before, 1).await;
    })
    .await
    .expect("schema-apply applied test timed out");
}

#[tokio::test]
async fn schema_apply_skips_reload_when_not_applied_or_err() {
    tokio::time::timeout(Duration::from_secs(30), async {
        let tmp = tempfile::tempdir().unwrap();
        let actor = spawn_actor(tmp.path(), EventBus::new());
        let setup = actor.call(EventIntent::None, ReloadIntent::None, apply(ITEM_SCHEMA, false));
        assert!(setup.await.unwrap().value.applied);
        let reloader = register_reloader(tmp.path(), &actor);
        let before = reloader.version();
        let intent = || ReloadIntent::SchemaApply { dry_run: false };

        // Re-applying the live schema is an empty plan: Ok, but applied == false.
        let noop = actor
            .call(EventIntent::None, intent(), apply(ITEM_SCHEMA, false))
            .await
            .unwrap();
        assert!(!noop.value.applied);
        let err = actor
            .call(EventIntent::None, intent(), apply("types: [", false))
            .await;
        assert!(err.is_err(), "malformed schema doc must fail");

        assert_reloads(&reloader, before, 0).await;
    })
    .await
    .expect("schema-apply not-applied test timed out");
}

#[tokio::test]
async fn schema_apply_preview_with_pending_ops_does_not_reload() {
    tokio::time::timeout(Duration::from_secs(30), async {
        let tmp = tempfile::tempdir().unwrap();
        let actor = spawn_actor(tmp.path(), EventBus::new());
        let reloader = register_reloader(tmp.path(), &actor);
        let before = reloader.version();

        // Intent claims a real apply, but the closure only previews: ops pending, nothing applied.
        let preview = actor
            .call(
                EventIntent::None,
                ReloadIntent::SchemaApply { dry_run: false },
                apply(ITEM_SCHEMA, true),
            )
            .await
            .unwrap();
        assert!(!preview.value.applied);
        assert!(!preview.value.ops.is_empty(), "a new type must plan ops");

        assert_reloads(&reloader, before, 0).await;
    })
    .await
    .expect("schema-apply preview test timed out");
}

#[tokio::test]
async fn explicit_true_ignored_for_schema_apply_result() {
    tokio::time::timeout(Duration::from_secs(30), async {
        let tmp = tempfile::tempdir().unwrap();
        let actor = spawn_actor(tmp.path(), EventBus::new());
        let reloader = register_reloader(tmp.path(), &actor);
        let before = reloader.version();

        // Explicit is the caller's own classification of SQL text; it has no
        // meaning for a schema-apply result, which reloads only under
        // ReloadIntent::SchemaApply.
        let output = actor
            .call(EventIntent::None, ReloadIntent::Explicit(true), apply(ITEM_SCHEMA, false))
            .await
            .unwrap();

        assert!(output.value.applied);
        assert_reloads(&reloader, before, 0).await;
    })
    .await
    .expect("explicit(true) schema-apply test timed out");
}

#[tokio::test]
async fn schema_apply_intent_on_non_schema_apply_result_does_not_reload() {
    tokio::time::timeout(Duration::from_secs(30), async {
        let tmp = tempfile::tempdir().unwrap();
        let actor = spawn_actor(tmp.path(), EventBus::new());
        let setup = actor.call(EventIntent::None, ReloadIntent::None, apply(ITEM_SCHEMA, false));
        assert!(setup.await.unwrap().value.applied);
        let reloader = register_reloader(tmp.path(), &actor);
        let before = reloader.version();

        let schemas = actor
            .call(
                EventIntent::None,
                ReloadIntent::SchemaApply { dry_run: false },
                |svc| svc.list_type_schemas(),
            )
            .await
            .unwrap();

        assert!(has_item(&schemas));
        assert_reloads(&reloader, before, 0).await;
    })
    .await
    .expect("schema-apply intent on read test timed out");
}

#[tokio::test]
async fn schema_apply_dry_run_never_reloads() {
    tokio::time::timeout(Duration::from_secs(30), async {
        let tmp = tempfile::tempdir().unwrap();
        let actor = spawn_actor(tmp.path(), EventBus::new());
        let reloader = register_reloader(tmp.path(), &actor);
        let before = reloader.version();
        let intent = || ReloadIntent::SchemaApply { dry_run: true };

        // The intent's dry_run wins even when the closure really applied.
        let applied = actor
            .call(EventIntent::None, intent(), apply(ITEM_SCHEMA, false))
            .await
            .unwrap();
        assert!(applied.value.applied);
        let preview = actor
            .call(EventIntent::None, intent(), apply(ITEM_SCHEMA, true))
            .await
            .unwrap();
        assert!(!preview.value.applied);
        let err = actor
            .call(EventIntent::None, intent(), apply("types: [", false))
            .await;
        assert!(err.is_err());

        assert_reloads(&reloader, before, 0).await;
    })
    .await
    .expect("schema-apply dry-run test timed out");
}

#[tokio::test]
async fn reload_intents_without_registered_reloader_return_values() {
    tokio::time::timeout(Duration::from_secs(30), async {
        let tmp = tempfile::tempdir().unwrap();
        let actor = spawn_actor(tmp.path(), EventBus::new());

        let output = actor
            .call(
                EventIntent::None,
                ReloadIntent::SchemaApply { dry_run: false },
                apply(ITEM_SCHEMA, false),
            )
            .await
            .unwrap();
        assert!(output.value.applied);

        let schemas = actor
            .call(EventIntent::None, ReloadIntent::Explicit(true), |svc| {
                svc.list_type_schemas()
            })
            .await
            .unwrap();
        assert!(has_item(&schemas), "actor must keep serving after a no-op reload check");
    })
    .await
    .expect("no-reloader test timed out");
}
