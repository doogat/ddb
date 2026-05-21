use ddb_core::types::ConflictAction;
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
        .create_doogat(None, None, vec![], None, BTreeMap::new(), ConflictAction::Error)
        .await
        .unwrap();
    // title: None maps to "" in CreateCommand
    assert_eq!(result.meta.title.as_deref(), Some(""));
}
