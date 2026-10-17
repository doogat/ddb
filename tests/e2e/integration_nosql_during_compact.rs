//! NoSQL reads must stay responsive while a compact runs. Task 2 moved
//! `compact` to a dedicated maintenance worker thread, separate from the
//! write-actor thread that `nosql_get` still dispatches on (00174). This test
//! proves THAT decoupling is what keeps a NoSQL GET bounded during compact,
//! not anything added in this task (task 3 only documents the redb
//! exclusive-open constraint).

use crate::common::{DdbTestRepo, ServerGuard};
use serde_json::json;
use std::time::{Duration, Instant};

fn create(server: &ServerGuard, title: &str) -> String {
    let result = server.graphql_with_vars(
        r#"mutation($input: CreateDoogatInput!) { createDoogat(input: $input) { id } }"#,
        json!({ "input": { "title": title } }),
    );
    assert!(result.get("errors").is_none(), "create failed: {result}");
    result["data"]["createDoogat"]["id"]
        .as_str()
        .expect("create missing id")
        .to_string()
}

fn nosql_get(server: &ServerGuard, id: &str, timeout: Duration) -> reqwest::blocking::Response {
    reqwest::blocking::Client::new()
        .get(format!("http://127.0.0.1:{}/nosql/{id}", server.port))
        .header("Authorization", format!("Bearer {}", server.token))
        .timeout(timeout)
        .send()
        .expect("nosql GET request failed")
}

#[test]
fn integration_nosql_get_stays_bounded_while_compact_runs() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    // The doogat the NoSQL GET will read throughout the scenario.
    let seed = create(&server, "Seed");

    // Give compact enough garbage to chew on: several doogats, each edited
    // a few times, so git gc + CRDT compaction has real work to do and
    // overlaps the GET below rather than finishing before it starts.
    for i in 0..20 {
        let id = create(&server, &format!("Garbage {i}"));
        for rev in 0..3 {
            let result = server.graphql_with_vars(
                r#"mutation($input: UpdateDoogatInput!) {
                    updateDoogat(input: $input) { id }
                }"#,
                json!({ "input": { "id": id, "title": format!("Garbage {i} rev {rev}") } }),
            );
            assert!(result.get("errors").is_none(), "update failed: {result}");
        }
    }

    let compact = std::thread::scope(|s| {
        let compact = s.spawn(|| {
            reqwest::blocking::Client::new()
                .post(server.url())
                .header("Authorization", format!("Bearer {}", server.token))
                .json(&json!({
                    "query": "mutation { compact(force: true, noBackup: true) { filesRemoved gcSuccess } }"
                }))
                .timeout(Duration::from_secs(60))
                .send()
                .expect("compact request failed")
                .json::<serde_json::Value>()
                .expect("invalid compact json")
        });

        // Bounded wait for the GET itself: if nosql_get were queued behind
        // compact on a shared thread, a hung/slow compact would starve it
        // past this timeout and the request would error out instead of
        // completing quickly.
        let start = Instant::now();
        let get_resp = nosql_get(&server, &seed, Duration::from_secs(5));
        let elapsed = start.elapsed();

        assert!(
            get_resp.status().is_success(),
            "nosql GET failed with status {}",
            get_resp.status()
        );
        assert!(
            elapsed < Duration::from_secs(5),
            "nosql GET for {seed} took {elapsed:?} while compact was running; \
             expected it to stay bounded because reads and maintenance run on \
             separate actor threads"
        );

        compact.join().unwrap()
    });

    assert!(
        compact.get("errors").is_none(),
        "compact failed: {compact}"
    );
}
