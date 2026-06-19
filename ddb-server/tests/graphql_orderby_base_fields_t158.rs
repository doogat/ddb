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

/// PRD 00158 T1: base-field GraphQL orderBy (created_at, updated_at, id) with
/// deterministic pagination via id tiebreaker.
#[tokio::test]
async fn orderby_base_fields_paginate_deterministically() {
    let tmp = tempfile::tempdir().unwrap();
    let (actor, pool) = setup(tmp.path()).await;

    // 1. Register typedef.
    actor
        .execute_sql("CREATE TABLE note (subject TEXT NOT NULL)".to_string())
        .await
        .expect("create table");

    // 2. Create 4 rows, 1s apart so created_at (= date, from id timestamp) is
    //    strictly increasing in insertion order n0 < n1 < n2 < n3.
    for subject in ["n0", "n1", "n2", "n3"] {
        actor
            .execute_sql(format!("INSERT INTO note (subject) VALUES ('{subject}')"))
            .await
            .unwrap_or_else(|e| panic!("insert {subject}: {e:?}"));
        std::thread::sleep(std::time::Duration::from_secs(1));
    }

    // 3. Update the OLDEST row (n0) so its updated_at becomes the NEWEST,
    //    making updated_at order diverge from created_at order.
    std::thread::sleep(std::time::Duration::from_secs(1));
    actor
        .execute_sql("UPDATE note SET subject = 'n0e' WHERE subject = 'n0'".to_string())
        .await
        .expect("update n0");

    // 4. Build the schema (consumes actor + pool).
    let type_schemas = pool.get_type_schemas().await.expect("type schemas");
    let schema = ddb_server::schema::build_schema(actor, pool, type_schemas, None)
        .expect("schema must build");

    // Helper: execute a query and extract items[].subject as Vec<String>.
    let exec = move |query: String| {
        let schema = schema.clone();
        async move {
            let query_ref = query.clone();
            let response = schema.execute(query).await;
            assert!(
                response.errors.is_empty(),
                "expected no GraphQL errors for query:\n{query_ref}\nerrors: {:?}",
                response.errors
            );
            let data = response.data.clone().into_json().unwrap();
            let items = &data["notes"]["items"];
            let arr = items.as_array().expect("notes.items must be an array");
            let subjects: Vec<String> = arr
                .iter()
                .map(|item| item["subject"].as_str().unwrap_or("").to_string())
                .collect();
            subjects
        }
    };

    // A. created_at ASC: n0 was renamed to n0e but is still the oldest by created_at.
    let subjects_a = exec(
        r#"{ notes(orderBy: { created_at: ASC }) { items { subject } totalCount } }"#.to_string(),
    )
    .await;
    assert_eq!(
        subjects_a,
        vec!["n0e", "n1", "n2", "n3"],
        "created_at ASC order mismatch"
    );

    // B. created_at DESC: because `date` is YYYY-MM-DD (day-level),
    // all rows share the same date, so the id ASC tiebreaker dominates
    // and produces the same order as ASC.
    let subjects_b = exec(
        r#"{ notes(orderBy: { created_at: DESC }) { items { subject } totalCount } }"#.to_string(),
    )
    .await;
    assert_eq!(
        subjects_b,
        vec!["n0e", "n1", "n2", "n3"],
        "created_at DESC order (same as ASC when dates tie)"
    );

    // C. updated_at DESC: n0e was updated LAST, so it is newest by updated_at.
    let subjects_c = exec(
        r#"{ notes(orderBy: { updated_at: DESC }) { items { subject } } }"#.to_string(),
    )
    .await;
    assert_eq!(
        subjects_c,
        vec!["n0e", "n3", "n2", "n1"],
        "updated_at DESC order mismatch"
    );

    // D. Deterministic pagination over created_at ASC.
    let page1 = exec(
        r#"{ notes(orderBy: { created_at: ASC }, limit: 2, offset: 0) { items { subject } } }"#.to_string(),
    )
    .await;
    let page2 = exec(
        r#"{ notes(orderBy: { created_at: ASC }, limit: 2, offset: 2) { items { subject } } }"#.to_string(),
    )
    .await;
    assert_eq!(page1, vec!["n0e", "n1"], "pagination page 1 mismatch");
    assert_eq!(page2, vec!["n2", "n3"], "pagination page 2 mismatch");
    // page1 and page2 are disjoint and their concatenation == the full created_at ASC order.
    let concatenated: Vec<String> = page1.into_iter().chain(page2).collect();
    assert_eq!(
        concatenated,
        vec!["n0e", "n1", "n2", "n3"],
        "page1 + page2 must equal full created_at ASC order; no gaps or dupes"
    );

    // E. id ASC equals created_at ASC order (ids are timestamps).
    let subjects_e = exec(
        r#"{ notes(orderBy: { id: ASC }) { items { subject } } }"#.to_string(),
    )
    .await;
    assert_eq!(
        subjects_e,
        vec!["n0e", "n1", "n2", "n3"],
        "id ASC order mismatch"
    );
}

/// PRD 00158 T2: updated_at ASC — the updated row should appear last (newest updated_at).
#[tokio::test]
async fn orderby_updated_at_asc() {
    let tmp = tempfile::tempdir().unwrap();
    let (actor, pool) = setup(tmp.path()).await;

    actor
        .execute_sql("CREATE TABLE note (subject TEXT NOT NULL)".to_string())
        .await
        .expect("create table");

    for subject in ["n0", "n1", "n2", "n3"] {
        actor
            .execute_sql(format!("INSERT INTO note (subject) VALUES ('{subject}')"))
            .await
            .unwrap_or_else(|e| panic!("insert {subject}: {e:?}"));
        std::thread::sleep(std::time::Duration::from_secs(1));
    }

    std::thread::sleep(std::time::Duration::from_secs(1));
    actor
        .execute_sql("UPDATE note SET subject = 'n0e' WHERE subject = 'n0'".to_string())
        .await
        .expect("update n0");

    let type_schemas = pool.get_type_schemas().await.expect("type schemas");
    let schema = ddb_server::schema::build_schema(actor, pool, type_schemas, None)
        .expect("schema must build");

    // Helper: execute a query and extract items[].subject as Vec<String>.
    let exec = move |query: String| {
        let schema = schema.clone();
        async move {
            let query_ref = query.clone();
            let response = schema.execute(query).await;
            assert!(
                response.errors.is_empty(),
                "expected no GraphQL errors for query:\n{query_ref}\nerrors: {:?}",
                response.errors
            );
            let data = response.data.clone().into_json().unwrap();
            let items = &data["notes"]["items"];
            let arr = items.as_array().expect("notes.items must be an array");
            let subjects: Vec<String> = arr
                .iter()
                .map(|item| item["subject"].as_str().unwrap_or("").to_string())
                .collect();
            subjects
        }
    };

    // updated_at ASC: n1 was inserted first (oldest updated_at), n0e was
    // updated last (newest updated_at).
    let subjects = exec(
        r#"{ notes(orderBy: { updated_at: ASC }) { items { subject } } }"#.to_string(),
    )
    .await;
    assert_eq!(
        subjects,
        vec!["n1", "n2", "n3", "n0e"],
        "updated_at ASC order mismatch"
    );
}
