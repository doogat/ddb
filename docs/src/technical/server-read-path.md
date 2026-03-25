# Server Read-Path Decision

Status: **accepted** | Date: 2026-03-07

## Context

The server serializes reads and writes through a single actor thread. Before broader adoption, we need to decide whether to keep this model or introduce a read fast path that bypasses the actor for non-mutating queries.

## Benchmark Results

All measurements on macOS, release build. Full benchmark suite: `cargo bench -p ddb-server` (requires `cargo build -p ddb-cli --release` first — benchmarks launch the `ddb` binary).

### Single-request latency (no contention)

| Protocol | Operation | 200 doogats | 5K doogats |
|----------|-----------|-------------|------------|
| GraphQL | get doogat | 276 µs | 1.04 ms |
| GraphQL | list 20 | 2.9 ms | 18.3 ms |
| GraphQL | FTS search | 65 µs | 65 µs |
| REST | get doogat | 211 µs | 1.05 ms |
| NoSQL | get doogat | 60 µs | 64 µs |
| pgwire | SELECT by id | 62 µs | 69 µs |

Get and search stay well under 10 ms at 5K. List (20 doogats) exceeds 10 ms at 5K (18.3 ms) due to index scan overhead on the larger dataset — this is the actor round-trip plus SQLite query time, not a protocol bottleneck.

### Concurrent reads (list 20 doogats, GraphQL)

| Readers | Batch time | Throughput |
|---------|-----------|------------|
| 1 | 2.9 ms | 342 req/s |
| 4 | 11.1 ms | 360 req/s |
| 8 | 22.1 ms | 362 req/s |
| 16 | 44.0 ms | 364 req/s |

Throughput is flat — time scales linearly with concurrency. The actor serializes requests; adding readers adds latency without increasing throughput.

FTS search scales better (15K → 60K elem/s from 1 → 16 readers) because the per-query cost is low enough that actor overhead is proportionally smaller.

### Mixed load (4 concurrent reads + background writes)

| Workload | Latency (4 readers) | Delta |
|----------|-------------------|-------|
| Reads only | 11 ms | baseline |
| Reads during writes | 500 ms | **45x** |
| Search during writes | 130 µs | ~1x |

Write operations (create + git commit + reindex) dominate actor time. List queries queue behind them; search queries are fast enough to squeeze through gaps.

This is the critical finding: **writes degrade read latency for expensive queries by 45x**.

### Protocol overhead (get-by-id)

| Protocol | 200 doogats | 5K doogats | Overhead vs NoSQL (5K) |
|----------|------------|------------|----------------------|
| NoSQL | 60 µs | 64 µs | baseline |
| pgwire | 62 µs | 69 µs | +5 µs |
| REST | 211 µs | 1.05 ms | +986 µs |
| GraphQL | 276 µs | 1.07 ms | +1.0 ms |

At 5K, REST and GraphQL show higher absolute overhead (schema resolution scales with type count). NoSQL and pgwire remain fast. Note: pgwire benchmarks cover get-by-id only; search and list via pgwire are not yet benchmarked.

## Options

### A. Keep single actor (chosen)

All reads and writes continue through the actor's mpsc channel.

**Pros:**
- Zero additional complexity
- Single-writer guarantee is trivially maintained
- No risk of stale reads or read/write races
- Current latency meets NFR-01 targets

**Cons:**
- Write activity degrades read latency for heavy queries (45x under sustained writes)
- Throughput ceiling of ~360 req/s for list-style queries
- Linear latency growth with concurrent readers

**When to revisit:** If multiple clients poll the server concurrently while writes are active, and the degraded latency is unacceptable.

### B. Read fast path (deferred)

Non-mutating queries bypass the actor and read SQLite directly via a shared read-only connection pool.

**Pros:**
- Reads unaffected by write activity
- True concurrent read throughput

**Cons:**
- SQLite WAL mode needed; read connections may see slightly stale data
- Must classify every query as read-only (risk: mutation disguised as read)
- Shared connection pool adds locking complexity
- More surface area for consistency bugs

**When to adopt:** Benchmarks show the actor is the bottleneck *and* real usage patterns involve concurrent read+write traffic.

## Decision

**Keep single actor.** The current model meets NFR-01 latency targets for get and search at 5K doogats. List queries exceed 10 ms at 5K (18.3 ms) but remain acceptable for the expected access pattern. The 45x degradation under mixed load is real but only manifests when writes are sustained — in practice, Doogat writes are infrequent (human-speed note-taking), so reads rarely contend.

### What scales

- Single-reader query latency: excellent (60 µs to 2.9 ms)
- FTS search under contention: good (130 µs with 4 readers + writes)
- Protocol diversity: all four protocols functional with similar actor-path cost

### What does not scale

- Concurrent list/get throughput: capped at ~360 req/s (actor-serialized)
- Read latency during sustained writes: 45x degradation for expensive queries
- More than ~16 concurrent readers: latency grows linearly

### Risks

- **Premature complexity**: Adding a read fast path now would create consistency surface area for no demonstrated user need.
- **Misleading benchmarks**: Sustained background writes are worst-case; real usage will show lower contention.

## Operating Envelope

These are the supported performance boundaries for the single-actor design.

### Supported

| Parameter | Limit | Notes |
|-----------|-------|-------|
| Concurrent readers (no writes) | Up to 16 | Latency grows linearly; p99 stays under 50 ms for list queries |
| Single-request latency | < 1.1 ms (get), < 70 µs (search), ~18 ms (list) | Measured at 5K doogats |
| Throughput (read-only) | ~360 req/s (list), ~15K req/s (search) | Actor-serialized ceiling |
| Write frequency | Human-speed (< 1 write/sec) | Reads remain responsive at this rate |

### Degraded

| Condition | Impact | Mitigation |
|-----------|--------|------------|
| Sustained writes (> 1/sec) | List latency degrades 10-45x | Batch writes; use search instead of list |
| > 16 concurrent readers | Latency > 50 ms per request | Rate-limit clients or add read fast path |
| Sync during reads | Similar to write degradation | Schedule sync during idle periods |

### Revisit triggers

Introduce a read fast path if any of these become true:

1. Multiple clients poll the server concurrently (e.g., mobile + desktop + web)
2. Background sync runs frequently enough to cause sustained write contention
3. Query latency under mixed load exceeds NFR-01's 10 ms target in production

## References

- NFR-01 / AC-06: Query latency < 10 ms at 5K doogats
- NFR-03 / AC-02: Sync should not degrade reads beyond accepted bounds
- Spec §5: Single-writer semantics preserved (actor enforces this)
