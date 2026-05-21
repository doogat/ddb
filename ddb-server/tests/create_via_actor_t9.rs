use ddb_core::app_contract::AppOutput;
use ddb_core::error::{codes, DoogatError};
use ddb_core::types::{ConflictAction, ParsedDoogat, Value};
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
    assert_eq!(result.value.meta.title.as_deref(), Some("My Title"));
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
    assert_eq!(result.value.body, "hello body");
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
    assert_eq!(result.value.meta.tags, vec!["alpha", "beta"]);
}

#[tokio::test]
async fn actor_create_none_title_renders_title_template() {
    let tmp = tempfile::tempdir().unwrap();
    let actor = spawn_actor(tmp.path()).await;
    actor
        .execute_sql("CREATE TABLE bookmark (url TEXT NOT NULL)".to_string())
        .await
        .unwrap();
    actor
        .execute_sql("ALTER TABLE bookmark SET TITLE TEMPLATE '{url}'".to_string())
        .await
        .unwrap();
    let mut fields = BTreeMap::new();
    fields.insert(
        "url".to_string(),
        Value::String("https://example.com".to_string()),
    );
    let result = actor
        .create_doogat(
            None,
            None,
            vec![],
            Some("bookmark".to_string()),
            fields,
            ConflictAction::Error,
        )
        .await
        .unwrap();
    let title = result.value.meta.title.as_deref().unwrap_or("");
    assert!(
        !title.is_empty(),
        "expected title rendered from title_template, got empty string"
    );
    assert!(
        title.contains("example.com"),
        "expected title to contain template value, got {title:?}"
    );
}

#[tokio::test]
async fn actor_create_none_title_untyped_returns_not_null_error() {
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
        .await;
    match result {
        Err(DoogatError::Structured {
            code: codes::NOT_NULL_VIOLATION,
            ..
        }) => {}
        Err(other) => panic!("expected NOT_NULL_VIOLATION, got {other:?}"),
        Ok(d) => panic!(
            "expected error for untyped title: None, got doogat with title {:?}",
            d.value.meta.title
        ),
    }
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
    assert_eq!(result.value.meta.doogat_type.as_deref(), Some("note"));
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
    assert_eq!(
        result.value.meta.extra.get("priority"),
        Some(&Value::Number(5.0))
    );
}

#[tokio::test]
async fn actor_create_conflict_ignore_returns_existing_singleton() {
    let tmp = tempfile::tempdir().unwrap();
    let actor = spawn_actor(tmp.path()).await;
    actor
        .execute_sql("CREATE TABLE cfg (theme TEXT) SINGLETON".to_string())
        .await
        .unwrap();
    let first = actor
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
    let first_id = first.value.meta.id.as_ref().unwrap().0.clone();

    let second = actor
        .create_doogat(
            Some("Config2".to_string()),
            None,
            vec![],
            Some("cfg".to_string()),
            BTreeMap::new(),
            ConflictAction::Ignore,
        )
        .await
        .expect("second SINGLETON create with Ignore must not error");

    let second_id = second.value.meta.id.as_ref().unwrap().0.clone();
    assert_eq!(
        first_id, second_id,
        "Ignore on SINGLETON must return the existing row, not create a new one"
    );
}

#[tokio::test]
async fn actor_create_returns_app_output_envelope() {
    let tmp = tempfile::tempdir().unwrap();
    let actor = spawn_actor(tmp.path()).await;
    let output: AppOutput<ParsedDoogat> = actor
        .create_doogat(
            Some("Warning Test".to_string()),
            None,
            vec![],
            None,
            BTreeMap::new(),
            ConflictAction::Error,
        )
        .await
        .unwrap();
    assert!(output.warnings.is_empty());
    assert!(
        output.value.meta.id.is_some(),
        "created doogat must have an id"
    );
}

#[tokio::test]
async fn actor_create_with_unregistered_type_returns_type_not_registered() {
    let tmp = tempfile::tempdir().unwrap();
    let actor = spawn_actor(tmp.path()).await;
    let result = actor
        .create_doogat(
            Some("Ghost".to_string()),
            None,
            vec![],
            Some("nonexistent_type".to_string()),
            BTreeMap::new(),
            ConflictAction::Error,
        )
        .await;
    match result {
        Err(DoogatError::Structured {
            code: codes::TYPE_NOT_REGISTERED,
            ..
        }) => {}
        Err(other) => panic!("expected TYPE_NOT_REGISTERED, got {other:?}"),
        Ok(_) => panic!("expected error for unregistered type, got Ok"),
    }
}
