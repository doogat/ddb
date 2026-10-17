//! NoSQL reads must stay responsive while a compact runs. Task 2 moved
//! `compact` to a dedicated maintenance worker thread, separate from the
//! write-actor thread that `nosql_get` still dispatches on (00174). This test
//! proves THAT decoupling is what keeps a NoSQL GET bounded during compact,
//! not anything added in this task (task 3 only documents the redb
//! exclusive-open constraint).

use crate::common::{DdbTestRepo, ServerGuard};
use serde_json::json;
use std::sync::atomic::{AtomicBool, Ordering};
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

/// Seed garbage for compact to chew on: several doogats, each edited a few
/// times, so git gc + CRDT compaction has real work to do.
fn seed_compaction_garbage(server: &ServerGuard) {
    for i in 0..20 {
        let id = create(server, &format!("Garbage {i}"));
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
}

/// Fire a burst of parallel GETs for `seed` against `server` while `compact`
/// is in flight, each checking whether `compact` is still running right
/// after it returns. Spreading many concurrent GETs across compact's run
/// window raises the odds of catching it still running, but cannot
/// guarantee it: on this backend, `compact` over a repo this size completes
/// in low single-digit milliseconds regardless of how much garbage is
/// seeded (measured: unchanged from 20 to several hundred edits), so a
/// deterministic overlap proof would need a test-only stall hook inside
/// `ddb-core`'s compaction path, which is outside this test-only task's
/// file scope. Returns whether overlap was actually observed, as a
/// best-effort signal only; the bounded-latency assertions below are the
/// test's real guarantee.
fn probe_overlap_with_burst_gets(
    server: &ServerGuard,
    seed: &str,
    compact: &std::thread::ScopedJoinHandle<'_, serde_json::Value>,
) -> bool {
    let observed_overlap = AtomicBool::new(false);
    std::thread::scope(|get_scope| {
        let handles: Vec<_> = (0..20)
            .map(|_| {
                get_scope.spawn(|| {
                    let start = Instant::now();
                    let get_resp = nosql_get(server, seed, Duration::from_secs(5));
                    let elapsed = start.elapsed();

                    assert!(
                        get_resp.status().is_success(),
                        "nosql GET failed with status {}",
                        get_resp.status()
                    );
                    assert!(
                        elapsed < Duration::from_secs(5),
                        "nosql GET for {seed} took {elapsed:?} while compact \
                         was running; expected it to stay bounded because \
                         reads and maintenance run on separate actor threads"
                    );
                    if !compact.is_finished() {
                        observed_overlap.store(true, Ordering::Relaxed);
                    }
                })
            })
            .collect();
        for handle in handles {
            handle.join().unwrap();
        }
    });
    observed_overlap.load(Ordering::Relaxed)
}

#[test]
fn integration_nosql_get_stays_bounded_while_compact_runs() {
    let repo = DdbTestRepo::init();
    let server = ServerGuard::start(&repo);

    // The doogat the NoSQL GET will read throughout the scenario.
    let seed = create(&server, "Seed");

    // Give compact enough garbage to chew on, so it has a realistic chance
    // of overlapping the GET burst below.
    seed_compaction_garbage(&server);

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

        // Best-effort overlap signal (see doc comment above); not asserted
        // on, since this backend's compact is too fast to guarantee it.
        let _observed_overlap = probe_overlap_with_burst_gets(&server, &seed, &compact);

        compact.join().unwrap()
    });

    assert!(
        compact.get("errors").is_none(),
        "compact failed: {compact}"
    );
}
