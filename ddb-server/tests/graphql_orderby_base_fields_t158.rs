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

/// Execute a GraphQL query, assert no errors, and return notes[].items[].subject.
async fn run_query(schema: &Schema, query: &str) -> Vec<String> {
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
/// deterministic pagination via id tiebreaker.
///
/// Fixture: insert n0..n3 with DISTINCT dates scrambled relative to insertion
/// order, so created_at ASC, created_at DESC, and id ASC all produce different
/// orderings, binding the sort direction under test.
///
///   subject | date       | insertion order (id ASC)
///   n0      | 2026-03-01 | 1st  (renamed to n0e after update)
///   n1      | 2026-01-01 | 2nd
///   n2      | 2026-04-01 | 3rd
///   n3      | 2026-02-01 | 4th
///
///   id ASC          = [n0e, n1, n2, n3]   (insertion order)
///   created_at ASC  = [n1, n3, n0e, n2]   (Jan < Feb < Mar < Apr)
///   created_at DESC = [n2, n0e, n3, n1]   (Apr > Mar > Feb > Jan)
///   updated_at DESC = [n0e, n3, n2, n1]   (n0e re-indexed last = newest)
#[tokio::test]
async fn orderby_base_fields_paginate_deterministically() {
    let tmp = tempfile::tempdir().unwrap();
    let (actor, pool) = setup(tmp.path()).await;

    // 1. Register typedef.
    actor
        .execute_sql("CREATE TABLE note (subject TEXT NOT NULL)".to_string())
        .await
        .expect("create table");

    // 2. Insert rows with explicit, scrambled dates so created_at order
    //    diverges from insertion (id) order. Small async sleeps ensure
    //    distinct updated_at values (set to chrono::Utc::now() at index time).
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

    // 3. Update n0 so its updated_at becomes the newest.
    tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
    actor
        .execute_sql("UPDATE note SET subject = 'n0e' WHERE subject = 'n0'".to_string())
        .await
        .expect("update n0");

    // 4. Build the schema (consumes actor + pool).
    let type_schemas = pool.get_type_schemas().await.expect("type schemas");
    let schema = ddb_server::schema::build_schema(actor, pool, type_schemas, None)
        .expect("schema must build");

    // A. id ASC: insertion order.
    let subjects_a = run_query(
        &schema,
        r#"{ notes(orderBy: { id: ASC }) { items { subject } } }"#,
    )
    .await;
    assert_eq!(
        subjects_a,
        vec!["n0e", "n1", "n2", "n3"],
        "id ASC order mismatch"
    );

    // B. created_at ASC: date order Jan < Feb < Mar < Apr.
    //    Differs from id ASC: [n1,n3,n0e,n2] != [n0e,n1,n2,n3].
    let subjects_b = run_query(
        &schema,
        r#"{ notes(orderBy: { created_at: ASC }) { items { subject } totalCount } }"#,
    )
    .await;
    assert_eq!(
        subjects_b,
        vec!["n1", "n3", "n0e", "n2"],
        "created_at ASC order mismatch"
    );

    // C. created_at DESC: date order Apr > Mar > Feb > Jan.
    //    Differs from ASC and from id ASC: proves direction matters.
    let subjects_c = run_query(
        &schema,
        r#"{ notes(orderBy: { created_at: DESC }) { items { subject } } }"#,
    )
    .await;
    assert_eq!(
        subjects_c,
        vec!["n2", "n0e", "n3", "n1"],
        "created_at DESC order mismatch (must differ from ASC)"
    );

    // D. updated_at DESC: n0e was re-indexed last (UPDATE), so it is newest.
    let subjects_d = run_query(
        &schema,
        r#"{ notes(orderBy: { updated_at: DESC }) { items { subject } } }"#,
    )
    .await;
    assert_eq!(
        subjects_d,
        vec!["n0e", "n3", "n2", "n1"],
        "updated_at DESC order mismatch"
    );

    // E. Deterministic pagination over created_at ASC with id tiebreaker.
    let page1 = run_query(
        &schema,
        r#"{ notes(orderBy: { created_at: ASC }, limit: 2, offset: 0) { items { subject } } }"#,
    )
    .await;
    let page2 = run_query(
        &schema,
        r#"{ notes(orderBy: { created_at: ASC }, limit: 2, offset: 2) { items { subject } } }"#,
    )
    .await;
    assert_eq!(page1, vec!["n1", "n3"], "pagination page 1 mismatch");
    assert_eq!(page2, vec!["n0e", "n2"], "pagination page 2 mismatch");
    let concatenated: Vec<String> = page1.into_iter().chain(page2).collect();
    assert_eq!(
        concatenated,
        vec!["n1", "n3", "n0e", "n2"],
        "page1 + page2 must equal full created_at ASC order; no gaps or dupes"
    );
}

/// PRD 00158 T2: updated_at ASC — the updated row (n0e) has the newest updated_at
/// and therefore appears LAST in ASC order. Ordering differs from both id ASC
/// and created_at ASC, binding the sort direction.
#[tokio::test]
async fn orderby_updated_at_asc() {
    let tmp = tempfile::tempdir().unwrap();
    let (actor, pool) = setup(tmp.path()).await;

    actor
        .execute_sql("CREATE TABLE note (subject TEXT NOT NULL)".to_string())
        .await
        .expect("create table");

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

    tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
    actor
        .execute_sql("UPDATE note SET subject = 'n0e' WHERE subject = 'n0'".to_string())
        .await
        .expect("update n0");

    let type_schemas = pool.get_type_schemas().await.expect("type schemas");
    let schema = ddb_server::schema::build_schema(actor, pool, type_schemas, None)
        .expect("schema must build");

    // updated_at ASC: n1 was indexed second (oldest remaining after n0 re-index),
    // n0e was re-indexed last (newest). Order: [n1, n2, n3, n0e].
    // Differs from id ASC [n0e,n1,n2,n3] and created_at ASC [n1,n3,n0e,n2].
    let subjects = run_query(
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
