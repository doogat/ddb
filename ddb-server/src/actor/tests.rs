use super::*;
use ddb_core::error::codes;
use std::time::Duration;

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
