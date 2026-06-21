use async_graphql::dynamic::Schema;
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

/// Build a schema over a `note` table seeded so created_at, updated_at, and id
/// orderings are all distinct — every test case below fails if its sort
/// direction is dropped. Shared by both tests so the fixture lives in one place.
///
///   subject  | date       | insertion order (id ASC)
///   n0 -> n0e| 2026-03-01 | 1st  (re-indexed last via UPDATE -> newest updated_at)
///   n1       | 2026-01-01 | 2nd
///   n2       | 2026-04-01 | 3rd
///   n3       | 2026-02-01 | 4th
///
///   id ASC          = [n0e, n1, n2, n3]   (insertion order)
///   created_at ASC  = [n1, n3, n0e, n2]   (Jan < Feb < Mar < Apr)
///   created_at DESC = [n2, n0e, n3, n1]   (Apr > Mar > Feb > Jan)
///   updated_at ASC  = [n1, n2, n3, n0e]   (n0e re-indexed last = newest)
///   updated_at DESC = [n0e, n3, n2, n1]
async fn build_fixture_schema(dir: &std::path::Path) -> Schema {
    let (actor, pool) = setup(dir).await;
    actor
        .execute_sql("CREATE TABLE note (subject TEXT NOT NULL)".to_string())
        .await
        .expect("create table");

    // Insert rows with explicit, scrambled dates so created_at order diverges
    // from insertion (id) order. Small async sleeps ensure distinct updated_at
    // values (set to chrono::Utc::now() at index time).
    let fixtures = [
        ("n0", "2026-03-01"),
        ("n1", "2026-01-01"),
        ("n2", "2026-04-01"),
        ("n3", "2026-02-01"),
    ];
    for (subject, date) in fixtures {
        actor
            .execute_sql(format!(
                "INSERT INTO note (subject, date) VALUES ('{subject}', '{date}')"
            ))
            .await
            .unwrap_or_else(|e| panic!("insert {subject}: {e:?}"));
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
    }

    // Update n0 so its updated_at becomes the newest.
    tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
    actor
        .execute_sql("UPDATE note SET subject = 'n0e' WHERE subject = 'n0'".to_string())
        .await
        .expect("update n0");

    let type_schemas = pool.get_type_schemas().await.expect("type schemas");
    ddb_server::schema::build_schema(actor, pool, type_schemas, None).expect("schema must build")
}

/// Execute a GraphQL query against the `note` connection, assert no errors, and
/// return notes[].items[].subject. (Coupled to the `notes` query by design — the
/// only connection these tests exercise.)
async fn run_notes_query(schema: &Schema, query: &str) -> Vec<String> {
    let response = schema.execute(query).await;
    assert!(
        response.errors.is_empty(),
        "expected no GraphQL errors for query:\n{query}\nerrors: {:?}",
        response.errors
    );
    let data = response.data.clone().into_json().unwrap();
    let arr = data["notes"]["items"]
        .as_array()
        .expect("notes.items must be an array");
    arr.iter()
        .map(|item| item["subject"].as_str().unwrap_or("").to_string())
        .collect()
}

/// PRD 00158 T1: base-field GraphQL orderBy (created_at, updated_at, id) with
/// deterministic pagination via the id tiebreaker. Each case below yields an
/// ordering distinct from the others (see `build_fixture_schema`), so a dropped
/// sort direction would change the result and fail the test.
#[tokio::test]
async fn orderby_base_fields_paginate_deterministically() {
    let tmp = tempfile::tempdir().unwrap();
    let schema = build_fixture_schema(tmp.path()).await;

    let cases = [
        (
            r#"{ notes(orderBy: { id: ASC }) { items { subject } } }"#,
            vec!["n0e", "n1", "n2", "n3"],
        ),
        (
            r#"{ notes(orderBy: { created_at: ASC }) { items { subject } totalCount } }"#,
            vec!["n1", "n3", "n0e", "n2"],
        ),
        (
            r#"{ notes(orderBy: { created_at: DESC }) { items { subject } } }"#,
            vec!["n2", "n0e", "n3", "n1"],
        ),
        (
            r#"{ notes(orderBy: { updated_at: DESC }) { items { subject } } }"#,
            vec!["n0e", "n3", "n2", "n1"],
        ),
    ];
    for (query, expected) in cases {
        assert_eq!(
            run_notes_query(&schema, query).await,
            expected,
            "order mismatch for query:\n{query}"
        );
    }

    // Deterministic pagination over created_at ASC with the id tiebreaker.
    let page1 = run_notes_query(
        &schema,
        r#"{ notes(orderBy: { created_at: ASC }, limit: 2, offset: 0) { items { subject } } }"#,
    )
    .await;
    let page2 = run_notes_query(
        &schema,
        r#"{ notes(orderBy: { created_at: ASC }, limit: 2, offset: 2) { items { subject } } }"#,
    )
    .await;
    assert_eq!(page1, vec!["n1", "n3"], "pagination page 1 mismatch");
    assert_eq!(page2, vec!["n0e", "n2"], "pagination page 2 mismatch");
    let full: Vec<String> = page1.into_iter().chain(page2).collect();
    assert_eq!(
        full,
        vec!["n1", "n3", "n0e", "n2"],
        "page1 + page2 must equal full created_at ASC order; no gaps or dupes"
    );
}

/// PRD 00158 T2: updated_at ASC — the updated row (n0e) has the newest
/// updated_at and therefore appears LAST in ASC order. Ordering differs from
/// both id ASC [n0e,n1,n2,n3] and created_at ASC [n1,n3,n0e,n2], binding the
/// sort direction.
#[tokio::test]
async fn orderby_updated_at_asc() {
    let tmp = tempfile::tempdir().unwrap();
    let schema = build_fixture_schema(tmp.path()).await;

    let subjects = run_notes_query(
        &schema,
        r#"{ notes(orderBy: { updated_at: ASC }) { items { subject } } }"#,
    )
    .await;
    assert_eq!(
        subjects,
        vec!["n1", "n2", "n3", "n0e"],
        "updated_at ASC order mismatch"
    );
}

/// PRD 00158 doubt-review: within a tied `date`, the appended `id` tiebreaker
/// must mirror the primary sort direction. `date` is day-level, so same-day
/// ties are the common case for the headline `created_at` reverse-chron use —
/// `created_at DESC` must keep the newer-id row first (not oldest-first), and
/// `created_at ASC` the older-id row first. Fails if the tiebreaker is a fixed
/// `id ASC` regardless of direction.
#[tokio::test]
async fn orderby_created_at_tiebreaker_follows_sort_direction() {
    let tmp = tempfile::tempdir().unwrap();
    let (actor, pool) = setup(tmp.path()).await;
    actor
        .execute_sql("CREATE TABLE note (subject TEXT NOT NULL)".to_string())
        .await
        .expect("create table");

    // Two rows share one date; insertion order fixes their ids (a < b).
    for subject in ["a", "b"] {
        actor
            .execute_sql(format!(
                "INSERT INTO note (subject, date) VALUES ('{subject}', '2026-05-01')"
            ))
            .await
            .unwrap_or_else(|e| panic!("insert {subject}: {e:?}"));
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
    }

    let type_schemas = pool.get_type_schemas().await.expect("type schemas");
    let schema = ddb_server::schema::build_schema(actor, pool, type_schemas, None)
        .expect("schema must build");

    // DESC: within the tied date, the newer id (b) comes first.
    assert_eq!(
        run_notes_query(
            &schema,
            r#"{ notes(orderBy: { created_at: DESC }) { items { subject } } }"#,
        )
        .await,
        vec!["b", "a"],
        "created_at DESC must keep the newest-id row first within a tied date"
    );
    // ASC: within the tied date, the older id (a) comes first.
    assert_eq!(
        run_notes_query(
            &schema,
            r#"{ notes(orderBy: { created_at: ASC }) { items { subject } } }"#,
        )
        .await,
        vec!["a", "b"],
        "created_at ASC must keep the oldest-id row first within a tied date"
    );
}
