use ddb_core::error::{codes, DoogatError};
use ddb_core::types::{ConflictAction, Value};
use ddb_server::actor::ActorHandle;
use ddb_server::events::EventBus;
use std::collections::BTreeMap;

async fn spawn_actor(dir: &std::path::Path) -> ActorHandle {
    ddb_core::service::DoogatService::init(dir).expect("init repo");
    let event_bus = EventBus::new();
    ActorHandle::spawn(dir.to_path_buf(), event_bus).expect("spawn actor")
}

#[tokio::test]
async fn actor_create_routes_title_correctly() {
    let tmp = tempfile::tempdir().unwrap();
    let actor = spawn_actor(tmp.path()).await;
    let result = actor
        .create_doogat(
            Some("My Title".to_string()),
            None,
            vec![],
            None,
            BTreeMap::new(),
            ConflictAction::Error,
        )
        .await
        .unwrap();
    assert_eq!(result.meta.title.as_deref(), Some("My Title"));
}

#[tokio::test]
async fn actor_create_routes_body_correctly() {
    let tmp = tempfile::tempdir().unwrap();
    let actor = spawn_actor(tmp.path()).await;
    let result = actor
        .create_doogat(
            Some("Body Note".to_string()),
            Some("hello body".to_string()),
            vec![],
            None,
            BTreeMap::new(),
            ConflictAction::Error,
        )
        .await
        .unwrap();
    assert_eq!(result.body, "hello body");
}

#[tokio::test]
async fn actor_create_routes_tags_correctly() {
    let tmp = tempfile::tempdir().unwrap();
    let actor = spawn_actor(tmp.path()).await;
    let result = actor
        .create_doogat(
            Some("Tagged".to_string()),
            None,
            vec!["alpha".to_string(), "beta".to_string()],
            None,
            BTreeMap::new(),
            ConflictAction::Error,
        )
        .await
        .unwrap();
    assert_eq!(result.meta.tags, vec!["alpha", "beta"]);
}

#[tokio::test]
async fn actor_create_with_none_title_uses_empty_string() {
    let tmp = tempfile::tempdir().unwrap();
    let actor = spawn_actor(tmp.path()).await;
    let result = actor
        .create_doogat(
            None,
            None,
            vec![],
            None,
            BTreeMap::new(),
            ConflictAction::Error,
        )
        .await
        .unwrap();
    assert_eq!(result.meta.title.as_deref(), Some(""));
}

#[tokio::test]
async fn actor_create_routes_doogat_type_correctly() {
    let tmp = tempfile::tempdir().unwrap();
    let actor = spawn_actor(tmp.path()).await;
    actor
        .execute_sql("CREATE TABLE note (title TEXT)".to_string())
        .await
        .unwrap();
    let result = actor
        .create_doogat(
            Some("Typed Note".to_string()),
            None,
            vec![],
            Some("note".to_string()),
            BTreeMap::new(),
            ConflictAction::Error,
        )
        .await
        .unwrap();
    assert_eq!(result.meta.doogat_type.as_deref(), Some("note"));
}

#[tokio::test]
async fn actor_create_routes_extra_fields_correctly() {
    let tmp = tempfile::tempdir().unwrap();
    let actor = spawn_actor(tmp.path()).await;
    let mut fields = BTreeMap::new();
    fields.insert("priority".to_string(), Value::Number(5.0));
    let result = actor
        .create_doogat(
            Some("Fields Note".to_string()),
            None,
            vec![],
            None,
            fields,
            ConflictAction::Error,
        )
        .await
        .unwrap();
    assert_eq!(result.meta.extra.get("priority"), Some(&Value::Number(5.0)));
}

#[tokio::test]
async fn actor_create_conflict_ignore_now_errors_on_constraint_violation() {
    let tmp = tempfile::tempdir().unwrap();
    let actor = spawn_actor(tmp.path()).await;
    actor
        .execute_sql("CREATE TABLE cfg (theme TEXT) SINGLETON".to_string())
        .await
        .unwrap();
    actor
        .create_doogat(
            Some("Config".to_string()),
            None,
            vec![],
            Some("cfg".to_string()),
            BTreeMap::new(),
            ConflictAction::Ignore,
        )
        .await
        .unwrap();
    let result = actor
        .create_doogat(
            Some("Config2".to_string()),
            None,
            vec![],
            Some("cfg".to_string()),
            BTreeMap::new(),
            ConflictAction::Ignore,
        )
        .await;
    match result.expect_err("expected error on second SINGLETON create") {
        DoogatError::Structured {
            code: codes::SINGLETON_VIOLATION,
            ..
        } => {}
        other => panic!("expected SINGLETON_VIOLATION, got {other:?}"),
    }
}
