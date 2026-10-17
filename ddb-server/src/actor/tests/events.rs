use super::*;
use crate::events::{DoogatEvent, EventKind};
use std::time::Duration;
use tokio::sync::broadcast;

#[tokio::test]
async fn dead_actor_yields_structured_error_not_panic() {
    tokio::time::timeout(Duration::from_secs(30), async {
        let tmp = tempfile::tempdir().unwrap();
        let actor = spawn_actor(tmp.path(), EventBus::new());

        // The closure panics on the actor thread, dropping the reply sender.
        let dropped = actor
            .call(
                EventIntent::None,
                ReloadIntent::Explicit(true),
                |_svc| -> ActorResult<Vec<TableSchema>> { panic!("verb crashed") },
            )
            .await;
        assert!(
            matches!(&dropped, Err(DoogatError::Validation(m)) if m == "actor dropped reply"),
            "dropped reply must map to the structured FromActorError value"
        );

        // The actor thread is gone: later calls still get a structured error.
        let after = actor
            .call(EventIntent::None, ReloadIntent::Explicit(true), |svc| {
                svc.list_type_schemas()
            })
            .await;
        assert!(
            matches!(&after, Err(DoogatError::Validation(m))
                if m == "actor stopped" || m == "actor dropped reply"),
            "a dead actor must yield a structured error"
        );
    })
    .await
    .expect("dead-actor test timed out");
}

// ---------------------------------------------------------------------------
// Event-bus emission pins for the `ActorHandle` verbs (closure + `EventIntent` +
// `IntentEmit`): each verb emits the expected events only on `Ok`. Events are compared as
// `(kind, doogat_id, doogat_type)`; timestamps are `Utc::now()` and are not
// part of the contract.

type Ev = (EventKind, String, Option<String>);

fn drain(rx: &mut broadcast::Receiver<DoogatEvent>) -> Vec<Ev> {
    let mut out = Vec::new();
    while let Ok(e) = rx.try_recv() {
        out.push((e.kind, e.doogat_id, e.doogat_type));
    }
    out
}

/// Kind and type only, for paths that necessarily touch different ids.
fn shape(events: &[Ev]) -> Vec<(EventKind, Option<String>)> {
    events.iter().map(|(k, _, t)| (k.clone(), t.clone())).collect()
}

fn id_of(z: &ParsedDoogat) -> String {
    z.meta.id.as_ref().map(ToString::to_string).unwrap()
}

fn events_for(kind: EventKind, doogats: &[ParsedDoogat]) -> Vec<Ev> {
    doogats
        .iter()
        .map(|z| (kind.clone(), id_of(z), z.meta.doogat_type.clone()))
        .collect()
}

fn item() -> Option<String> {
    Some("item".into())
}

/// Fresh actor with the `item` type registered.
async fn parity_actor(dir: &std::path::Path, bus: EventBus) -> ActorHandle {
    let actor = spawn_actor(dir, bus);
    assert!(actor.apply_schema(ITEM_SCHEMA.into(), false, false).await.unwrap().value.applied);
    actor
}

async fn new_item(actor: &ActorHandle, title: &str) -> String {
    let out = actor
        .create_doogat(Some(title.into()), Some("body".into()), vec![], item(), Default::default(), ConflictAction::default())
        .await
        .unwrap();
    id_of(&out.value)
}

fn update_input(id: &str, title: &str) -> BatchUpdateInput {
    BatchUpdateInput {
        id: id.into(),
        title: Some(title.into()),
        body: None,
        tags: None,
        doogat_type: None,
        fields: None,
        unset_fields: None,
    }
}

fn create_input(title: &str) -> BatchCreateInput {
    BatchCreateInput {
        title: Some(title.into()),
        body: Some("body".into()),
        tags: vec![],
        doogat_type: item(),
        fields: Default::default(),
        on_conflict: ConflictAction::default(),
    }
}

#[tokio::test]
async fn create_doogat_emits_one_created_event() {
    tokio::time::timeout(Duration::from_secs(60), async {
        let tmp = tempfile::tempdir().unwrap();
        let bus = EventBus::new();
        let actor = parity_actor(tmp.path(), bus.clone()).await;
        let mut rx = bus.subscribe();

        let value = actor
            .create_doogat(Some("intent".into()), Some("body".into()), vec![], item(), Default::default(), ConflictAction::default())
            .await
            .unwrap()
            .value;
        let events = drain(&mut rx);

        assert_eq!(events, events_for(EventKind::Created, &[value]));
        assert_eq!(shape(&events), vec![(EventKind::Created, item())]);
    })
    .await
    .expect("create event test timed out");
}

#[tokio::test]
async fn update_doogat_emits_one_updated_event() {
    tokio::time::timeout(Duration::from_secs(60), async {
        let tmp = tempfile::tempdir().unwrap();
        let bus = EventBus::new();
        let actor = parity_actor(tmp.path(), bus.clone()).await;
        let id = new_item(&actor, "target").await;
        let mut rx = bus.subscribe();

        actor
            .update_doogat(UpdateDoogatParams {
                id: id.clone(),
                title: Some("intent".into()),
                body: None,
                tags: None,
                doogat_type: None,
                fields: Default::default(),
                unset_fields: vec![],
            })
            .await
            .unwrap();
        let events = drain(&mut rx);

        assert_eq!(events, vec![(EventKind::Updated, id, item())]);
    })
    .await
    .expect("update event test timed out");
}

#[tokio::test]
async fn delete_doogat_emits_one_deleted_event_failed_delete_emits_none() {
    tokio::time::timeout(Duration::from_secs(60), async {
        let tmp = tempfile::tempdir().unwrap();
        let bus = EventBus::new();
        let actor = parity_actor(tmp.path(), bus.clone()).await;
        let id = new_item(&actor, "intent").await;
        let mut rx = bus.subscribe();

        actor.delete_doogat(id.clone()).await.unwrap();
        let events = drain(&mut rx);
        assert_eq!(events, vec![(EventKind::Deleted, id.clone(), item())]);

        // A failed delete (doogat already gone) emits nothing.
        assert!(actor.delete_doogat(id).await.is_err());
        assert!(drain(&mut rx).is_empty(), "a failed delete must emit no event");
    })
    .await
    .expect("delete event test timed out");
}

/// Regression pin for the shared-cell mechanism: the closure resolves the type
/// before deleting (the row is gone afterward), and emission reads it back.
#[tokio::test]
async fn delete_emits_exactly_one_deleted_event_with_resolved_type() {
    tokio::time::timeout(Duration::from_secs(60), async {
        let tmp = tempfile::tempdir().unwrap();
        let bus = EventBus::new();
        let actor = parity_actor(tmp.path(), bus.clone()).await;
        let id = new_item(&actor, "doomed").await;
        let resolved = actor.get_doogat(id.clone()).await.unwrap().meta.doogat_type;
        assert_eq!(resolved, item());
        let mut rx = bus.subscribe();

        actor.delete_doogat(id.clone()).await.unwrap();

        assert_eq!(drain(&mut rx), vec![(EventKind::Deleted, id, resolved)]);
    })
    .await
    .expect("delete shared-cell test timed out");
}

#[tokio::test]
async fn batch_update_emits_one_updated_event_per_doogat() {
    tokio::time::timeout(Duration::from_secs(60), async {
        let tmp = tempfile::tempdir().unwrap();
        let bus = EventBus::new();
        let actor = parity_actor(tmp.path(), bus.clone()).await;
        let a = new_item(&actor, "a").await;
        let b = new_item(&actor, "b").await;
        let mut rx = bus.subscribe();

        let updates = vec![update_input(&a, "intent"), update_input(&b, "intent")];
        let list = actor.batch_update(updates).await.unwrap();
        let events = drain(&mut rx);

        assert_eq!(events, events_for(EventKind::Updated, &list));
        assert_eq!(events.len(), 2);
    })
    .await
    .expect("batch-update event test timed out");
}

#[tokio::test]
async fn create_many_emits_one_created_event_per_doogat() {
    tokio::time::timeout(Duration::from_secs(60), async {
        let tmp = tempfile::tempdir().unwrap();
        let bus = EventBus::new();
        let actor = parity_actor(tmp.path(), bus.clone()).await;
        let mut rx = bus.subscribe();

        let inputs = vec![create_input("intent 1"), create_input("intent 2")];
        let list = actor.create_many(inputs).await.unwrap();
        let events = drain(&mut rx);

        assert_eq!(events, events_for(EventKind::Created, &list));
        assert_eq!(shape(&events), vec![(EventKind::Created, item()); 2]);
    })
    .await
    .expect("create-many event test timed out");
}

/// Fresh actor with `item` registered as a SINGLETON typedef.
async fn singleton_actor(dir: &std::path::Path, bus: EventBus) -> ActorHandle {
    let actor = parity_actor(dir, bus).await;
    let schema = format!("{ITEM_SCHEMA}    singleton: true\n");
    assert!(actor.apply_schema(schema, false, false).await.unwrap().value.applied);
    actor
}

/// Upsert creates then updates the same singleton: first call creates, second
/// call updates the same row.
#[tokio::test]
async fn upsert_singleton_emits_created_then_updated_event() {
    tokio::time::timeout(Duration::from_secs(60), async {
        let tmp = tempfile::tempdir().unwrap();
        let bus = EventBus::new();
        let actor = singleton_actor(tmp.path(), bus.clone()).await;
        let mut rx = bus.subscribe();
        let mut ids = Vec::new();
        for _ in 0..2 {
            let outcome = actor.upsert_singleton("item".into(), Default::default()).await.unwrap();
            ids.push(outcome.id);
        }
        let events = drain(&mut rx);

        assert_eq!(
            events,
            vec![
                (EventKind::Created, ids[0].clone(), item()),
                (EventKind::Updated, ids[1].clone(), item()),
            ]
        );
    })
    .await
    .expect("upsert event test timed out");
}

// ---------------------------------------------------------------------------
// Dropped reply per return-type family: the structured FromActorError value
// reaches the caller over the oneshot, never a panic or an "unexpected reply".

const DROPPED: &[&str] = &["actor dropped reply"];
const GONE: &[&str] = &["actor stopped", "actor dropped reply"];

fn assert_actor_error<T>(result: &ActorResult<T>, allowed: &[&str]) {
    assert!(
        matches!(result, Err(DoogatError::Validation(m)) if allowed.contains(&m.as_str())),
        "expected one of {allowed:?}"
    );
}

/// Kill the actor thread mid-verb with a closure returning `ActorResult<T>`.
async fn crash_verb<T>(actor: &ActorHandle, intent: EventIntent) -> ActorResult<T>
where
    T: Send + 'static,
    ActorResult<T>: IntentEmit + ReloadSignal,
{
    actor
        .call(intent, ReloadIntent::None, |_svc| -> ActorResult<T> { panic!("verb crashed") })
        .await
}

#[tokio::test]
async fn read_family_dropped_reply_yields_structured_error() {
    tokio::time::timeout(Duration::from_secs(30), async {
        let tmp = tempfile::tempdir().unwrap();
        let actor = spawn_actor(tmp.path(), EventBus::new());
        assert_actor_error(&crash_verb::<ParsedDoogat>(&actor, EventIntent::None).await, DROPPED);
        assert_actor_error(&actor.get_doogat("20250101000000".into()).await, GONE);
    })
    .await
    .expect("read-family dropped-reply test timed out");
}

#[tokio::test]
async fn create_family_dropped_reply_yields_structured_error_and_no_event() {
    tokio::time::timeout(Duration::from_secs(30), async {
        let tmp = tempfile::tempdir().unwrap();
        let bus = EventBus::new();
        let actor = spawn_actor(tmp.path(), bus.clone());
        let mut rx = bus.subscribe();
        let dropped = crash_verb::<AppOutput<ParsedDoogat>>(&actor, EventIntent::Created).await;
        assert_actor_error(&dropped, DROPPED);
        let after = actor
            .create_doogat(Some("t".into()), None, vec![], None, Default::default(), ConflictAction::default())
            .await;
        assert_actor_error(&after, GONE);
        assert!(drain(&mut rx).is_empty(), "a crashed create must emit no event");
    })
    .await
    .expect("create-family dropped-reply test timed out");
}

#[tokio::test]
async fn list_family_dropped_reply_yields_structured_error_and_no_event() {
    tokio::time::timeout(Duration::from_secs(30), async {
        let tmp = tempfile::tempdir().unwrap();
        let bus = EventBus::new();
        let actor = spawn_actor(tmp.path(), bus.clone());
        let mut rx = bus.subscribe();
        let dropped = crash_verb::<Vec<ParsedDoogat>>(&actor, EventIntent::CreateMany).await;
        assert_actor_error(&dropped, DROPPED);
        assert_actor_error(&actor.create_many(vec![create_input("t")]).await, GONE);
        assert!(drain(&mut rx).is_empty(), "a crashed create-many must emit no event");
    })
    .await
    .expect("list-family dropped-reply test timed out");
}

#[tokio::test]
async fn sql_family_dropped_reply_yields_structured_error() {
    tokio::time::timeout(Duration::from_secs(30), async {
        let tmp = tempfile::tempdir().unwrap();
        let actor = spawn_actor(tmp.path(), EventBus::new());
        assert_actor_error(&crash_verb::<SqlResult>(&actor, EventIntent::None).await, DROPPED);
        assert_actor_error(&actor.execute_sql("SELECT 1".into()).await, GONE);
    })
    .await
    .expect("sql-family dropped-reply test timed out");
}
