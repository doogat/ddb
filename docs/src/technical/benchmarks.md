# Benchmarks

Criterion benchmark suite measuring core operations at 1K doogats on NVMe SSD (macOS, warm cache).

## Running Benchmarks

```bash
cargo bench                  # all benchmarks
cargo bench --bench crud     # CRUD only
cargo bench --bench search   # search/index only
cargo bench --bench sync     # sync/compact only
```

Results are stored in `target/criterion/` with HTML reports.

## Baseline Results

Measured on Apple Silicon (M-series), macOS, release profile.

### CRUD Operations

| Operation | Median | Notes |
|-----------|--------|-------|
| create | 175 ms | single doogat commit (1K repo) |
| read | 231 µs | blob lookup from Git ODB |
| update | 168 ms | commit-durable write |
| delete | 165 ms | commit-durable delete |
| batch_commit_10 | 176 ms | 10 doogats in one commit |

Write operations are dominated by Git fsync. Batch commits amortize overhead — 10 writes cost roughly the same as 1.

> **Baselines vs targets**: Current CRUD baselines (165-175ms) exceed the per-operation commit-durable targets (30-50ms) in the table below. These are pre-optimization measurements at 1K doogats. The gap is almost entirely Git object hashing + fsync; the CRDT and indexer layers add <5ms. Planned optimizations: write-ahead batching (amortize fsync across multiple ops, as `batch_commit_10` already demonstrates), loose-object writes with deferred pack, and optional `core.fsyncObjectFiles=false` for write-acknowledged mode. No optimization work has been attempted yet — baselines exist to track improvement.

### Search & Index

| Operation | Median | Notes |
|-----------|--------|-------|
| FTS5 search | 563 µs | single keyword, 1K doogats |
| SQL SELECT | 6.3 µs | LIKE filter with LIMIT 10 |
| Full rebuild | 869 ms | reindex all 1K doogats |

Query latency is well under the NFR-01 target of <10 ms at 5K doogats.

### Sync & Compaction

| Operation | Median | Notes |
|-----------|--------|-------|
| fast_forward sync | 990 ms | fetch + merge 10 new doogats (1K repo) |
| compact | 287 ms | forced compact on 1K repo |

## NFR Targets (from spec)

| NFR | Metric | 5K target | 50K target |
|-----|--------|-----------|------------|
| NFR-01 | Query latency (indexed) | < 10 ms | < 50 ms |
| NFR-02 | Repository growth | < 50 MB/year | < 200 MB/year |
| NFR-03 | Sync time (batch) | < 2 seconds | < 10 seconds |

### Per-Operation Targets (desktop, SSD)

| Operation | Write-acknowledged | Commit-durable |
|-----------|-------------------|----------------|
| Create/update doogat | ~5 ms | ~30-50 ms |
| Read doogat | 1-3 ms | N/A |
| Git merge (no conflict) | N/A | 50-200 ms |
| CRDT resolve | N/A | 100-500 ms |
| Full sync (network) | N/A | 1-5 s |
| Compaction | N/A | 500-2000 ms |

### Mobile Targets

TBD after profiling on actual devices. Estimated 2-3x desktop latency for writes. Cold start (libgit2 + Automerge + SQLite): ~300-800 ms.

## Profiling

Build with tracing instrumentation for flamegraph generation:

```bash
cargo build -p ddb-core --features profiling
```

This adds `tracing::instrument` spans on hot paths. Use with `cargo flamegraph` or any `tracing`-compatible subscriber.
