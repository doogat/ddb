use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// Doogat content helpers (mirrors ddb-core bench helpers)
// ---------------------------------------------------------------------------

const DOOGAT_COUNT_DEFAULT: usize = 200;
const DOOGAT_COUNT_LARGE: usize = 5_000;

fn doogat_content(i: usize) -> String {
    let word = match i % 5 {
        0 => "architecture",
        1 => "refactoring",
        2 => "deployment",
        3 => "performance",
        _ => "documentation",
    };
    format!(
        "---\ntitle: Note about {word} {i}\ndate: 2026-01-01\ntags:\n  - bench\n  - {word}\n---\n\
         This doogat discusses {word} in the context of item {i}.\n\
         Some additional body text for search indexing.\n\
         ---\n- source:: bench-{i}"
    )
}

fn doogat_path(i: usize) -> String {
    format!("ddb/{:014}.md", 20260101000000u64 + i as u64)
}

// ---------------------------------------------------------------------------
// Lightweight server harness (mirrors tests/e2e/common.rs ServerGuard)
// ---------------------------------------------------------------------------

fn ddb_bin() -> PathBuf {
    let mut dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    dir.pop(); // ddb-server/ -> workspace root
    let target = std::env::var("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| dir.join("target"));
    let profile = if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    };
    target
        .join(profile)
        .join(format!("ddb{}", std::env::consts::EXE_SUFFIX))
}

static PORT_COUNTER: AtomicU16 = AtomicU16::new(18200);

struct BenchServer {
    child: Child,
    port: u16,
    token: String,
    _dir: TempDir,
    _remote_dir: TempDir,
}

impl BenchServer {
    fn start(dir: TempDir, remote_dir: TempDir) -> Self {
        let port = PORT_COUNTER.fetch_add(2, Ordering::SeqCst);
        let pg_port = port + 1;

        let mut child = std::process::Command::new(ddb_bin())
            .arg("--repo")
            .arg(dir.path())
            .arg("serve")
            .arg("--port")
            .arg(port.to_string())
            .arg("--pg-port")
            .arg(pg_port.to_string())
            .stderr(Stdio::piped())
            .spawn()
            .expect("failed to start ddb server (run `cargo build -p ddb-cli --release` first)");

        let stderr = child.stderr.take().unwrap();
        let reader = BufReader::new(stderr);
        let mut token = String::new();
        let mut http_ready = false;
        let mut pg_ready = false;

        for line in reader.lines() {
            let line = line.unwrap();
            if line.contains("auth token:") {
                if let Some(path) = line.split("auth token: ").nth(1) {
                    token = std::fs::read_to_string(path.trim())
                        .unwrap_or_default()
                        .trim()
                        .to_string();
                }
            }
            if line.contains("pgwire listening on") {
                pg_ready = true;
            } else if line.contains("listening on") {
                http_ready = true;
            }
            if http_ready && pg_ready {
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(50));

        Self {
            child,
            port,
            token,
            _dir: dir,
            _remote_dir: remote_dir,
        }
    }

    fn url(&self) -> String {
        format!("http://127.0.0.1:{}/graphql", self.port)
    }

    fn rest_url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}/rest{path}", self.port)
    }

    fn nosql_url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}/nosql{path}", self.port)
    }

    fn pg_port(&self) -> u16 {
        self.port + 1
    }
}

impl Drop for BenchServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

// ---------------------------------------------------------------------------
// Test repo seeding
// ---------------------------------------------------------------------------

fn seed_repo_n(dir: &Path, count: usize) {
    // Init via CLI
    let out = std::process::Command::new(ddb_bin())
        .arg("init")
        .arg(dir)
        .output()
        .expect("ddb init failed");
    assert!(
        out.status.success(),
        "ddb init: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    // Seed doogats by writing files + git commit
    for i in 0..count {
        let path = dir.join(doogat_path(i));
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&path, doogat_content(i)).unwrap();
    }
    let status = std::process::Command::new("git")
        .current_dir(dir)
        .args(["add", "."])
        .status()
        .unwrap();
    assert!(status.success());
    let status = std::process::Command::new("git")
        .current_dir(dir)
        .args(["commit", "-m", "seed"])
        .env("GIT_AUTHOR_NAME", "bench")
        .env("GIT_AUTHOR_EMAIL", "bench@test")
        .env("GIT_COMMITTER_NAME", "bench")
        .env("GIT_COMMITTER_EMAIL", "bench@test")
        .status()
        .unwrap();
    assert!(status.success());
}

fn setup_server() -> BenchServer {
    setup_server_n(DOOGAT_COUNT_DEFAULT)
}

fn setup_server_n(count: usize) -> BenchServer {
    let dir = TempDir::new().unwrap();
    let remote_dir = TempDir::new().unwrap();
    seed_repo_n(dir.path(), count);

    let clone_out = std::process::Command::new("git")
        .args(["clone", "--bare"])
        .arg(dir.path())
        .arg(remote_dir.path())
        .output()
        .unwrap();
    assert!(
        clone_out.status.success(),
        "git clone --bare: {}",
        String::from_utf8_lossy(&clone_out.stderr)
    );

    let add_remote_out = std::process::Command::new("git")
        .current_dir(dir.path())
        .args(["remote", "add", "origin"])
        .arg(remote_dir.path())
        .output()
        .unwrap();
    assert!(
        add_remote_out.status.success(),
        "git remote add origin: {}",
        String::from_utf8_lossy(&add_remote_out.stderr)
    );

    BenchServer::start(dir, remote_dir)
}

// ---------------------------------------------------------------------------
// Async helpers
// ---------------------------------------------------------------------------

fn build_client(token: &str) -> reqwest::Client {
    reqwest::Client::builder()
        .default_headers({
            let mut h = reqwest::header::HeaderMap::new();
            h.insert("Authorization", format!("Bearer {token}").parse().unwrap());
            h
        })
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap()
}

async fn graphql_request(
    client: &reqwest::Client,
    url: &str,
    query: &str,
) -> reqwest::Result<serde_json::Value> {
    let body = serde_json::json!({ "query": query });
    client.post(url).json(&body).send().await?.json().await
}

/// Send `n` concurrent copies of `query` and return all results.
async fn concurrent_reads(
    client: &reqwest::Client,
    url: &str,
    query: &str,
    n: usize,
) -> Vec<serde_json::Value> {
    let mut handles = Vec::with_capacity(n);
    for _ in 0..n {
        let c = client.clone();
        let u = url.to_string();
        let q = query.to_string();
        handles.push(tokio::spawn(async move {
            graphql_request(&c, &u, &q).await.unwrap()
        }));
    }
    let mut results = Vec::with_capacity(n);
    for h in handles {
        results.push(h.await.unwrap());
    }
    results
}

// ---------------------------------------------------------------------------
// GraphQL queries
// ---------------------------------------------------------------------------

const Q_LIST: &str = "{ doogats(limit: 20) { id title } }";
const Q_SEARCH: &str =
    r#"{ search(query: "architecture", limit: 10) { items { id title } totalCount } }"#;

fn q_get(i: usize) -> String {
    format!(
        r#"{{ doogat(id: "{:014}") {{ id title body }} }}"#,
        20260101000000u64 + i as u64
    )
}

// ---------------------------------------------------------------------------
// Benchmarks
// ---------------------------------------------------------------------------

fn bench_single_reads(c: &mut Criterion) {
    let server = setup_server();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let client = build_client(&server.token);
    let url = server.url();

    let mut group = c.benchmark_group("server/single_read");
    group.sample_size(50);
    group.measurement_time(Duration::from_secs(10));

    group.bench_function("list_doogats", |b| {
        b.iter(|| {
            rt.block_on(graphql_request(&client, &url, Q_LIST)).unwrap();
        });
    });

    group.bench_function("get_doogat", |b| {
        b.iter(|| {
            rt.block_on(graphql_request(&client, &url, &q_get(0)))
                .unwrap();
        });
    });

    group.bench_function("search", |b| {
        b.iter(|| {
            rt.block_on(graphql_request(&client, &url, Q_SEARCH))
                .unwrap();
        });
    });

    group.finish();
}

fn bench_concurrent_reads(c: &mut Criterion) {
    let server = setup_server();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let client = build_client(&server.token);
    let url = server.url();

    let mut group = c.benchmark_group("server/concurrent_reads");
    group.sample_size(30);
    group.measurement_time(Duration::from_secs(15));

    for concurrency in [1, 4, 8, 16] {
        group.throughput(Throughput::Elements(concurrency as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(concurrency),
            &concurrency,
            |b, &n| {
                b.iter(|| {
                    rt.block_on(concurrent_reads(&client, &url, Q_LIST, n));
                });
            },
        );
    }

    group.finish();
}

fn bench_concurrent_search(c: &mut Criterion) {
    let server = setup_server();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let client = build_client(&server.token);
    let url = server.url();

    let mut group = c.benchmark_group("server/concurrent_search");
    group.sample_size(30);
    group.measurement_time(Duration::from_secs(15));

    for concurrency in [1, 4, 8, 16] {
        group.throughput(Throughput::Elements(concurrency as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(concurrency),
            &concurrency,
            |b, &n| {
                b.iter(|| {
                    rt.block_on(concurrent_reads(&client, &url, Q_SEARCH, n));
                });
            },
        );
    }

    group.finish();
}

// ---------------------------------------------------------------------------
// REST / NoSQL / pgwire helpers
// ---------------------------------------------------------------------------

async fn rest_request(client: &reqwest::Client, url: &str) -> reqwest::Result<serde_json::Value> {
    client.get(url).send().await?.json().await
}

async fn pgwire_query_reuse(client: &tokio_postgres::Client, sql: &str) {
    client.simple_query(sql).await.unwrap();
}

// ---------------------------------------------------------------------------
// Mixed-load benchmark: reads during concurrent writes
// ---------------------------------------------------------------------------

fn q_create(seq: usize) -> String {
    format!(
        r#"mutation {{ createDoogat(input: {{ title: "bench-write-{seq}", content: "load test body {seq}", tags: ["bench"] }}) {{ id }} }}"#
    )
}

fn q_update(id: &str, seq: usize) -> String {
    format!(
        r#"mutation {{ updateDoogat(input: {{ id: "{id}", title: "bench-updated-{seq}" }}) {{ id }} }}"#
    )
}

const Q_SYNC: &str =
    r#"mutation { sync { direction commitsTransferred conflictsResolved resurrected } }"#;

/// Spawn a background write loop that creates, updates, and syncs doogats until `stop` is set.
/// Returns a JoinHandle and an Arc tracking how many writes completed.
fn spawn_write_load(
    rt: &tokio::runtime::Runtime,
    client: &reqwest::Client,
    url: &str,
    stop: Arc<AtomicBool>,
) -> (tokio::task::JoinHandle<()>, Arc<AtomicUsize>) {
    let c = client.clone();
    let u = url.to_string();
    let write_count = Arc::new(AtomicUsize::new(0));
    let wc = write_count.clone();
    let handle = rt.spawn(async move {
        let mut seq = 0usize;
        let mut created_ids: Vec<String> = Vec::new();
        while !stop.load(Ordering::Relaxed) {
            // Alternate: create a new doogat, then update one we created earlier
            let resp = graphql_request(&c, &u, &q_create(seq)).await.unwrap();
            wc.fetch_add(1, Ordering::Relaxed);
            if let Some(id) = resp
                .pointer("/data/createDoogat/id")
                .and_then(|v| v.as_str())
            {
                created_ids.push(id.to_string());
            }
            if let Some(id) = created_ids.get(seq % created_ids.len().max(1)) {
                let _ = graphql_request(&c, &u, &q_update(id, seq)).await;
                wc.fetch_add(1, Ordering::Relaxed);
            }
            // Sync every 3rd iteration to exercise fetch/merge under mixed load.
            if seq % 3 == 2 {
                let sync_resp = graphql_request(&c, &u, Q_SYNC).await.unwrap();
                assert!(
                    sync_resp.get("errors").is_none(),
                    "sync mutation returned errors: {sync_resp}"
                );
                wc.fetch_add(1, Ordering::Relaxed);
            }
            seq += 1;
            // Small yield to avoid starving the read benchmarks
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    });
    (handle, write_count)
}

fn bench_mixed_load(c: &mut Criterion) {
    let server = setup_server();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let client = build_client(&server.token);
    let url = server.url();

    let mut group = c.benchmark_group("server/mixed_load");
    group.sample_size(30);
    group.measurement_time(Duration::from_secs(15));

    // Baseline: read-only (no background writes)
    group.bench_function("reads_only", |b| {
        b.iter(|| {
            rt.block_on(concurrent_reads(&client, &url, Q_LIST, 4));
        });
    });

    // Mixed: reads with background writes
    let stop = Arc::new(AtomicBool::new(false));
    let (write_handle, write_count) = spawn_write_load(&rt, &client, &url, stop.clone());

    group.bench_function("reads_during_writes", |b| {
        b.iter(|| {
            rt.block_on(concurrent_reads(&client, &url, Q_LIST, 4));
        });
    });

    // Mixed: search with background writes
    group.bench_function("search_during_writes", |b| {
        b.iter(|| {
            rt.block_on(concurrent_reads(&client, &url, Q_SEARCH, 4));
        });
    });

    stop.store(true, Ordering::Relaxed);
    rt.block_on(write_handle).unwrap();
    eprintln!(
        "mixed_load: background writes completed = {}",
        write_count.load(Ordering::Relaxed)
    );

    group.finish();
}

// ---------------------------------------------------------------------------
// Cross-protocol comparison: same "list doogats" across all protocols
// ---------------------------------------------------------------------------

fn bench_protocol_comparison(c: &mut Criterion) {
    let server = setup_server();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let client = build_client(&server.token);
    let graphql_url = server.url();
    let first_id = format!("{:014}", 20260101000000u64);
    let rest_url = server.rest_url(&format!("/doogats/{first_id}"));
    let nosql_url = server.nosql_url(&format!("/{:014}", 20260101000000u64));
    let pg_port = server.pg_port();
    let token = server.token.clone();

    // Pre-connect a persistent pgwire client for fair comparison
    let pg_client: tokio_postgres::Client = rt.block_on(async {
        let (client, conn) = tokio_postgres::Config::new()
            .host("127.0.0.1")
            .port(pg_port)
            .user("ddb")
            .password(&token)
            .dbname("ddb")
            .connect(tokio_postgres::NoTls)
            .await
            .unwrap();
        tokio::spawn(async move {
            conn.await.ok();
        });
        client
    });

    let mut group = c.benchmark_group("server/protocol_comparison");
    group.sample_size(50);
    group.measurement_time(Duration::from_secs(10));

    let gql_get = q_get(0);
    group.bench_function("graphql", |b| {
        b.iter(|| {
            rt.block_on(graphql_request(&client, &graphql_url, &gql_get))
                .unwrap();
        });
    });

    group.bench_function("rest", |b| {
        b.iter(|| {
            rt.block_on(rest_request(&client, &rest_url)).unwrap();
        });
    });

    group.bench_function("nosql", |b| {
        b.iter(|| {
            rt.block_on(rest_request(&client, &nosql_url)).unwrap();
        });
    });

    group.bench_function("pgwire", |b| {
        b.iter(|| {
            rt.block_on(pgwire_query_reuse(
                &pg_client,
                &format!("SELECT id, title, body FROM doogats WHERE id = '{first_id}'"),
            ));
        });
    });

    group.finish();
}

// ---------------------------------------------------------------------------
// 5K-doogat benchmarks: validates NFR-01 at target scale
// ---------------------------------------------------------------------------

fn bench_single_reads_5k(c: &mut Criterion) {
    let server = setup_server_n(DOOGAT_COUNT_LARGE);
    let rt = tokio::runtime::Runtime::new().unwrap();
    let client = build_client(&server.token);
    let url = server.url();

    let mut group = c.benchmark_group("server_5k/single_read");
    group.sample_size(30);
    group.measurement_time(Duration::from_secs(10));

    group.bench_function("list_doogats", |b| {
        b.iter(|| {
            rt.block_on(graphql_request(&client, &url, Q_LIST)).unwrap();
        });
    });

    group.bench_function("get_doogat", |b| {
        b.iter(|| {
            rt.block_on(graphql_request(&client, &url, &q_get(0)))
                .unwrap();
        });
    });

    group.bench_function("search", |b| {
        b.iter(|| {
            rt.block_on(graphql_request(&client, &url, Q_SEARCH))
                .unwrap();
        });
    });

    group.finish();
}

fn bench_protocol_comparison_5k(c: &mut Criterion) {
    let server = setup_server_n(DOOGAT_COUNT_LARGE);
    let rt = tokio::runtime::Runtime::new().unwrap();
    let client = build_client(&server.token);
    let graphql_url = server.url();
    let first_id = format!("{:014}", 20260101000000u64);
    let rest_url = server.rest_url(&format!("/doogats/{first_id}"));
    let nosql_url = server.nosql_url(&format!("/{first_id}"));
    let pg_port = server.pg_port();
    let token = server.token.clone();

    let pg_client: tokio_postgres::Client = rt.block_on(async {
        let (client, conn) = tokio_postgres::Config::new()
            .host("127.0.0.1")
            .port(pg_port)
            .user("ddb")
            .password(&token)
            .dbname("ddb")
            .connect(tokio_postgres::NoTls)
            .await
            .unwrap();
        tokio::spawn(async move {
            conn.await.ok();
        });
        client
    });

    let mut group = c.benchmark_group("server_5k/protocol_comparison");
    group.sample_size(30);
    group.measurement_time(Duration::from_secs(10));

    let gql_get = q_get(0);
    group.bench_function("graphql", |b| {
        b.iter(|| {
            rt.block_on(graphql_request(&client, &graphql_url, &gql_get))
                .unwrap();
        });
    });

    group.bench_function("rest", |b| {
        b.iter(|| {
            rt.block_on(rest_request(&client, &rest_url)).unwrap();
        });
    });

    group.bench_function("nosql", |b| {
        b.iter(|| {
            rt.block_on(rest_request(&client, &nosql_url)).unwrap();
        });
    });

    group.bench_function("pgwire", |b| {
        b.iter(|| {
            rt.block_on(pgwire_query_reuse(
                &pg_client,
                &format!("SELECT id, title, body FROM doogats WHERE id = '{first_id}'"),
            ));
        });
    });

    group.finish();
}

fn bench_concurrent_reads_5k(c: &mut Criterion) {
    let server = setup_server_n(DOOGAT_COUNT_LARGE);
    let rt = tokio::runtime::Runtime::new().unwrap();
    let client = build_client(&server.token);
    let url = server.url();

    let mut group = c.benchmark_group("server_5k/concurrent_reads");
    group.sample_size(30);
    group.measurement_time(Duration::from_secs(15));

    for concurrency in [1, 4, 8, 16] {
        group.throughput(Throughput::Elements(concurrency as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(concurrency),
            &concurrency,
            |b, &n| {
                b.iter(|| {
                    rt.block_on(concurrent_reads(&client, &url, Q_LIST, n));
                });
            },
        );
    }

    group.finish();
}

fn bench_mixed_load_5k(c: &mut Criterion) {
    let server = setup_server_n(DOOGAT_COUNT_LARGE);
    let rt = tokio::runtime::Runtime::new().unwrap();
    let client = build_client(&server.token);
    let url = server.url();

    let mut group = c.benchmark_group("server_5k/mixed_load");
    group.sample_size(30);
    group.measurement_time(Duration::from_secs(15));

    // Baseline: read-only (no background writes)
    group.bench_function("reads_only", |b| {
        b.iter(|| {
            rt.block_on(concurrent_reads(&client, &url, Q_LIST, 4));
        });
    });

    // Mixed: reads with background writes
    let stop = Arc::new(AtomicBool::new(false));
    let (write_handle, write_count) = spawn_write_load(&rt, &client, &url, stop.clone());

    group.bench_function("reads_during_writes", |b| {
        b.iter(|| {
            rt.block_on(concurrent_reads(&client, &url, Q_LIST, 4));
        });
    });

    // Mixed: search with background writes
    group.bench_function("search_during_writes", |b| {
        b.iter(|| {
            rt.block_on(concurrent_reads(&client, &url, Q_SEARCH, 4));
        });
    });

    stop.store(true, Ordering::Relaxed);
    rt.block_on(write_handle).unwrap();
    eprintln!(
        "mixed_load_5k: background writes completed = {}",
        write_count.load(Ordering::Relaxed)
    );

    group.finish();
}

criterion_group!(
    benches,
    bench_single_reads,
    bench_concurrent_reads,
    bench_concurrent_search,
    bench_mixed_load,
    bench_protocol_comparison
);
criterion_group!(
    benches_5k,
    bench_single_reads_5k,
    bench_concurrent_reads_5k,
    bench_mixed_load_5k,
    bench_protocol_comparison_5k
);
criterion_main!(benches, benches_5k);
