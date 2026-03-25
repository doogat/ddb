# Performance

Measured values for NFR/AC performance targets. All measurements taken on Darwin/arm64 (Apple Silicon) in release mode.

## Summary

| Req | Target | Measured | Status |
|-----|--------|----------|--------|
| NFR-01 / AC-06 | Query < 10ms (5K) | FTS ~3ms, SQL ~6µs | PASS |
| NFR-02 / AC-08 | Growth < 50MB/yr (5K) | < 50MB | PASS |
| NFR-03 / AC-02 | Sync < 2s (5K, LAN) | ~12.6s | FAIL |
| NFR-04 / AC-07 | Binary size profiled | 23.5MB | — |
| AC-19 | Query < 50ms (50K) | not yet measured | — |
| Mobile FFI | Baseline measured | init <5ms, search <1ms | — |

## Binary Size (NFR-04 / AC-07)

| Platform | Build | Size |
|----------|-------|------|
| Darwin/arm64 | release | 23.5MB |

Run `dev/bin/profile-binary-size` to measure on your platform.

## Query Latency (NFR-01 / AC-06 / AC-19)

| Scale | Operation | Target | Measured |
|-------|-----------|--------|----------|
| 5K doogats | FTS search | < 10ms | ~3.0ms |
| 5K doogats | SQL SELECT | < 10ms | ~6.1µs |
| 50K doogats | FTS search | < 50ms | not yet measured |
| 50K doogats | SQL SELECT | < 50ms | not yet measured |

Run 5K benchmarks: `cargo bench -p ddb-core --bench search -- "5k"`

Run 50K benchmarks: `cargo bench -p ddb-core --bench large_scale`

5K threshold tests: `cargo test --release -p ddb-core --test query_thresholds nfr01_`

The local release script (`dev/bin/release`) runs the 5K release-profile threshold tests before it bumps versions, creates a tag, or pushes.

50K threshold tests (slow): `cargo test --release -p ddb-core --test query_thresholds -- --ignored`

## Repo Growth (NFR-02 / AC-08)

| Scale | Target | Status |
|-------|--------|--------|
| 5K doogats, 365 days × 10 edits/day | < 50MB | PASS |

Benchmark: `cargo bench -p ddb-core --bench growth -- --test`

Release threshold test: `cargo test --release -p ddb-core --test growth_thresholds nfr02_`

The local release script (`dev/bin/release`) runs the repo-growth release threshold before it bumps versions, creates a tag, or pushes.

## Sync Latency (NFR-03 / AC-02)

| Scale | Target | Measured | Status |
|-------|--------|----------|--------|
| 5K doogats, localhost | < 2s | ~12.6s | FAIL |

Sync latency exceeds the NFR-03 target by ~6x. This needs optimization work (likely in `SyncManager::sync` or the underlying git fetch/merge path).

Run: `cargo bench -p ddb-core --bench sync -- "5k"`

Threshold test (ignored): `cargo test --release -p ddb-core --test sync_thresholds -- --ignored`

## Server Read-Path (NFR-01 under load)

| Workload | Latency | Status |
|----------|---------|--------|
| Single read (get doogat) | 60–276 µs | PASS |
| 16 concurrent readers | ~44 ms (list 20) | PASS |
| Reads during sustained writes | ~500 ms (list 20) | Degraded |

The actor serializes all operations. Reads meet NFR-01 targets under normal use but degrade 45x under sustained write load. Decision: keep single actor; see [Server Read-Path Decision](./server-read-path.md) for full analysis and operating envelope.

Run: `cargo build -p ddb-cli --release && cargo bench -p ddb-server`

## Mobile FFI Baseline (UniFFI / DoogatDriver)

Measured via platform-native timing on Darwin/arm64 (Apple Silicon), debug build.

| Metric | Swift (ContinuousClock) | Kotlin/JVM (measureTimeMillis) |
|--------|------------------------|-------------------------------|
| Cold start (DoogatDriver init) | 3.08 ms | 4 ms |
| Single doogat create | 971.97 ms | 966 ms |
| FTS search (100 doogats) | 0.83 ms | 1 ms |
| Reindex (100 doogats) | 47.72 ms | 41 ms |

**Notes:**

- Swift tests run via `swift test` (SPM, macOS host, debug build)
- Kotlin tests run via `./gradlew test` (JVM host, release native lib)
- Create latency is dominated by git commit per doogat (~1s each)
- Search and reindex are fast; both scale sub-linearly with doogat count
- Cold start is negligible (<5ms) — SQLite + git repo open

Run Swift: `cd tests/swift && swift test --filter testPerformanceMetrics`

Run Kotlin: `cd tests/kotlin && ./gradlew test --rerun --tests '*testPerformanceMetrics'`
