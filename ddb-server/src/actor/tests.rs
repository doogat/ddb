use super::*;
use ddb_core::app_contract::{CreateCommand, UnregisteredTypePolicy};
use ddb_core::error::codes;
use std::time::Duration;
use tokio::sync::broadcast;

#[tokio::test]
async fn shared_actor_preserves_schema_applys_owned_transaction() {
    tokio::time::timeout(Duration::from_secs(30), async {
        let tmp = tempfile::tempdir().unwrap();
        DoogatService::init(tmp.path()).unwrap();
        let actor = ActorHandle::spawn(tmp.path().to_path_buf(), EventBus::new()).unwrap();
        let schema = "types:\n  - name: item\n    columns:\n      - name: label\n        data_type: TEXT\n        zone: frontmatter\n";
        actor.apply_schema(schema.into(), false, false).await.unwrap();
        actor.execute_sql("INSERT INTO item (label) VALUES ('committed')".into()).await.unwrap();
        let result = actor.execute_sql("SELECT label FROM item".into()).await.unwrap();
        assert!(matches!(result, SqlResult::Rows { rows, .. } if rows == vec![vec!["committed".to_string()]]));
    }).await.expect("schema apply transaction test timed out");
}

/// Exercises the real actor constructor: reverting its Shared scope to the
/// default Exclusive scope must fail this test, even if core guard tests pass.
#[tokio::test]
async fn actor_service_uses_shared_transaction_scope() {
    tokio::time::timeout(Duration::from_secs(30), async {
        let tmp = tempfile::tempdir().unwrap();
        DoogatService::init(tmp.path()).unwrap();
        let actor = ActorHandle::spawn(tmp.path().to_path_buf(), EventBus::new()).unwrap();
        for sql in ["BEGIN", "COMMIT", "ROLLBACK", "SELECT 1; BEGIN"] {
            let err = actor.execute_sql(sql.into()).await.unwrap_err();
            assert!(
                matches!(
                    err,
                    DoogatError::Structured {
                        code: codes::TRANSACTION_NOT_SUPPORTED,
                        ..
                    }
                ),
                "unexpected error for {sql}: {err}"
            );
        }
        let err = actor.execute_batch(vec!["BEGIN".into()]).await.unwrap_err();
        assert!(matches!(
            err,
            DoogatError::Structured {
                code: codes::TRANSACTION_NOT_SUPPORTED,
                ..
            }
        ));
        // A later client can execute a complete transaction normally.
        actor
            .execute_batch(vec!["BEGIN".into(), "SELECT 1".into(), "COMMIT".into()])
            .await
            .unwrap();
    })
    .await
    .expect("actor transaction scope test timed out");
}

// ---------------------------------------------------------------------------
// Phase 0 acceptance tests (Task 1.1, TDD) for the closure transport.
//
// These pin the behavior the generic `ActorHandle::call<R, F>` +
// `EventIntent` transport (Task 1.2) must reproduce, and they compile and pass
// WHILE the twin enums (`ActorCommand`/`ActorReply`) and `handle_command` still
// exist — proving the closure work is purely additive and independently
// reversible (design.md "Reversibility strategy", requirements 1.1/1.2/1.3).
//
// Acceptance behavior being pinned:
//   * a READ (EventIntent::None) returns its value and emits NO `DoogatEvent`
//     (Req 3.3);
//   * a MUTATION (EventIntent::Created) returns its value AND emits exactly one
//     matching `DoogatEvent` on the `EventBus` subscribed before the call
//     (Req 3.4).
//
// Task 1.1 runs one wave BEFORE Task 1.2 adds `call`/`EventIntent`. To keep
// this task's Tier 1 gate green (`cargo build`, clippy, `cargo test-ci`), the
// executable assertions below drive the SAME two verbs through the CURRENT
// verb-method path (`get_type_schemas`, `create_doogat`), which the closure
// transport must preserve byte-for-byte. The exact target closure form each
// test must be re-expressed against once 1.2 lands is recorded verbatim in the
// doc comment on each test so the intent is not lost.

/// Read acceptance: a non-mutating verb returns its value and emits no event.
///
/// Routed through the closure transport (Task 1.2) with `EventIntent::None`
/// (Req 1.1, 1.2, 1.3, 3.3).
#[tokio::test]
async fn read_returns_value_and_emits_no_event() {
    tokio::time::timeout(Duration::from_secs(30), async {
        let tmp = tempfile::tempdir().unwrap();
        DoogatService::init(tmp.path()).unwrap();
        let bus = EventBus::new();
        let actor = ActorHandle::spawn(tmp.path().to_path_buf(), bus.clone()).unwrap();

        // Subscribe BEFORE the call so any emitted event would be observed.
        let mut rx = bus.subscribe();

        // Drive one read through the generic closure entry point. `EventIntent::None`
        // means no event may be emitted; the read returns the current schema set.
        let schemas = actor
            .call(EventIntent::None, |svc| svc.list_type_schemas())
            .await
            .unwrap();
        // Every returned schema is a well-formed row (named table).
        assert!(
            schemas.iter().all(|s| !s.table_name.is_empty()),
            "every returned schema must carry a table name"
        );

        // Emits NO event.
        assert!(
            matches!(rx.try_recv(), Err(broadcast::error::TryRecvError::Empty)),
            "a read must not emit any DoogatEvent"
        );
    })
    .await
    .expect("read acceptance test timed out");
}

/// Mutation acceptance: a mutating verb returns its value AND emits exactly one
/// matching `DoogatEvent`.
///
/// Routed through the closure transport (Task 1.2) with `EventIntent::Created`
/// (Req 1.1, 1.2, 1.3, 3.4). Emission happens at the closure boundary from the
/// actual `Ok` result, so exactly one `Created` event carrying the created doogat's
/// id/type reaches the bus.
#[tokio::test]
async fn mutation_returns_value_and_emits_created_event() {
    tokio::time::timeout(Duration::from_secs(30), async {
        let tmp = tempfile::tempdir().unwrap();
        DoogatService::init(tmp.path()).unwrap();
        let bus = EventBus::new();
        let actor = ActorHandle::spawn(tmp.path().to_path_buf(), bus.clone()).unwrap();

        // Subscribe BEFORE the call so the emitted event is observed.
        let mut rx = bus.subscribe();

        // Drive one create mutation through the generic closure entry point,
        // declaring `EventIntent::Created`. The closure calls the SAME
        // `DoogatService::create` the legacy `create_doogat` verb routes to.
        let output = actor
            .call(EventIntent::Created, |svc| {
                svc.create(CreateCommand {
                    title: Some("acceptance".into()),
                    body: Some("body".into()),
                    tags: vec![],
                    doogat_type: None,
                    fields: std::collections::BTreeMap::new(),
                    on_conflict: ConflictAction::default(),
                    unregistered_type_policy: UnregisteredTypePolicy::Strict,
                })
            })
            .await
            .unwrap();

        // Returns the value.
        let created_id = output
            .value
            .meta
            .id
            .as_ref()
            .map(ToString::to_string)
            .expect("created doogat must have an id");

        // Emits exactly one Created event carrying the created doogat's id/type.
        let event = rx.recv().await.expect("expected one Created event");
        assert_eq!(event.kind, EventKind::Created);
        assert_eq!(event.doogat_id, created_id);
        assert_eq!(event.doogat_type, output.value.meta.doogat_type);
        assert!(
            matches!(rx.try_recv(), Err(broadcast::error::TryRecvError::Empty)),
            "a single create must emit exactly one event"
        );
    })
    .await
    .expect("mutation acceptance test timed out");
}
