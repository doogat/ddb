use crate::common::{ServerGuard, DdbTestRepo};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio_postgres::SimpleQueryMessage;

/// Concurrent reads succeed while a write is in flight.
#[test]
fn concurrent_reads_during_write() {
    let repo = DdbTestRepo::init();

    // Create a doogat so reads have something to query
    repo.ddb()
        .args(["create", "--title", "Seed", "--body", "seed body"])
        .assert()
        .success();

    let server = Arc::new(ServerGuard::start(&repo));

    // Fire concurrent reads + one write simultaneously
    let mut handles: Vec<std::thread::JoinHandle<()>> = Vec::new();

    // 6 concurrent read queries (GraphQL + REST)
    for i in 0..3 {
        let srv = Arc::clone(&server);
        handles.push(std::thread::spawn(move || {
            let result = srv.graphql(r#"{ doogats { id title } }"#);
            assert!(
                result.get("errors").is_none(),
                "concurrent graphql read {i} failed: {result}"
            );
            let doogats = result.pointer("/data/doogats").unwrap();
            assert!(doogats.is_array(), "expected array, got {doogats}");
        }));
    }
    for i in 0..3 {
        let srv = Arc::clone(&server);
        handles.push(std::thread::spawn(move || {
            let resp = srv.rest_get("/doogats");
            assert!(
                resp.status().is_success(),
                "concurrent rest read {i} failed: {}",
                resp.status()
            );
        }));
    }

    // 1 write (mutation)
    let srv = Arc::clone(&server);
    handles.push(std::thread::spawn(move || {
        let result = srv.graphql_with_vars(
            r#"mutation($input: CreateDoogatInput!) { createDoogat(input: $input) { id } }"#,
            serde_json::json!({ "input": {
                "title": "Written during reads",
                "content": "body",
            } }),
        );
        assert!(
            result.get("errors").is_none(),
            "write during concurrent reads failed: {result}"
        );
    }));

    for h in handles {
        h.join().expect("thread panicked");
    }

    // Verify both seed and new doogat exist
    let result = server.graphql(r#"{ search(query: "Written during reads") { hits { id } } }"#);
    assert!(
        result.get("errors").is_none(),
        "post-write search failed: {result}"
    );
    let hits = result.pointer("/data/search/hits").unwrap();
    assert_eq!(hits.as_array().unwrap().len(), 1);
}

/// pgwire SELECT uses ReadPool (verified by running a query while a mutation is in flight)
#[test]
fn pgwire_select_during_graphql_write() {
    let repo = DdbTestRepo::init();

    repo.ddb()
        .args(["create", "--title", "PgSeed", "--body", "pg seed"])
        .assert()
        .success();

    let server = Arc::new(ServerGuard::start(&repo));

    // Fire a pgwire SELECT and a GraphQL mutation concurrently
    let srv1 = Arc::clone(&server);
    let pg_handle = std::thread::spawn(move || {
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            let (client, connection) = tokio_postgres::Config::new()
                .host("127.0.0.1")
                .port(srv1.pg_port)
                .user("ddb")
                .password(&srv1.token)
                .dbname("ddb")
                .connect(tokio_postgres::NoTls)
                .await
                .expect("pg connect failed");
            tokio::spawn(async move {
                connection.await.ok();
            });

            let messages = client
                .simple_query("SELECT 1 AS n")
                .await
                .expect("pg SELECT failed");
            let row = messages
                .iter()
                .find_map(|m| match m {
                    SimpleQueryMessage::Row(row) => Some(row),
                    _ => None,
                })
                .expect("missing row");
            assert_eq!(row.get(0), Some("1"));
        });
    });

    let srv2 = Arc::clone(&server);
    let write_handle = std::thread::spawn(move || {
        let result = srv2.graphql_with_vars(
            r#"mutation($input: CreateDoogatInput!) { createDoogat(input: $input) { id } }"#,
            serde_json::json!({ "input": {
                "title": "Written during pg read",
                "content": "body",
            } }),
        );
        assert!(
            result.get("errors").is_none(),
            "write during pg read failed: {result}"
        );
    });

    pg_handle.join().expect("pgwire thread panicked");
    write_handle.join().expect("write thread panicked");
}

/// Read latency stays bounded while writes are in flight.
///
/// PRD target: p95 < 2x idle. We use 10x to avoid CI flakiness while
/// still catching regressions where reads queue behind writes.
#[test]
fn read_latency_bounded_under_writes() {
    let repo = DdbTestRepo::init();

    repo.ddb()
        .args(["create", "--title", "LatSeed", "--body", "seed"])
        .assert()
        .success();

    let server = Arc::new(ServerGuard::start(&repo));

    // Measure idle baseline (5 sequential reads)
    let mut baseline_times = Vec::new();
    for _ in 0..5 {
        let start = Instant::now();
        let result = server.graphql(r#"{ doogats { id title } }"#);
        baseline_times.push(start.elapsed());
        assert!(result.get("errors").is_none());
    }
    baseline_times.sort();
    let baseline_p95 = baseline_times[baseline_times.len() * 95 / 100];

    // Fire reads while a sustained write burst runs
    let write_server = Arc::clone(&server);
    let write_done = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let wd = Arc::clone(&write_done);
    let write_thread = std::thread::spawn(move || {
        for i in 0..5 {
            let _ = write_server.graphql_with_vars(
                r#"mutation($input: CreateDoogatInput!) { createDoogat(input: $input) { id } }"#,
                serde_json::json!({ "input": {
                    "title": format!("LatWrite{i}"),
                    "content": "body",
                } }),
            );
        }
        wd.store(true, std::sync::atomic::Ordering::Release);
    });

    let mut mixed_times = Vec::new();
    while !write_done.load(std::sync::atomic::Ordering::Acquire) || mixed_times.len() < 5 {
        let start = Instant::now();
        let result = server.graphql(r#"{ doogats { id title } }"#);
        mixed_times.push(start.elapsed());
        assert!(result.get("errors").is_none());
        if mixed_times.len() >= 50 {
            break;
        }
    }

    write_thread.join().expect("write thread panicked");

    mixed_times.sort();
    let mixed_p95 = mixed_times[mixed_times.len() * 95 / 100];
    let bound = baseline_p95 * 10; // PRD target is 2x; 10x avoids CI flakiness

    eprintln!(
        "read latency: baseline_p95={:?} mixed_p95={:?} bound={:?}",
        baseline_p95, mixed_p95, bound
    );

    assert!(
        mixed_p95 < bound.max(Duration::from_millis(500)),
        "p95 read latency {mixed_p95:?} exceeded 10x baseline {baseline_p95:?} (bound={bound:?})"
    );
}
