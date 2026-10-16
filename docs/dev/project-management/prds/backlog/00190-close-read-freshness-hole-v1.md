---
catchup: force
default_model: opus
model_tier_rationale: concurrency - staleness probe with a single rebuild owner and a debounced watcher
---

# Close the read-freshness hole (probe + watch mode + consistency contract)

**Priority:** P1 · **Roadmap:** Phase 3 (purpose gaps) · **Depends on:** 00169 (its merged serialized-cold-start-rebuild leg — the probe delegates rebuilds to one owner) · **Source:** maintainer decision 2026-07-13, `dev/local/audit-results/2026-07-12-purpose-fit-critique.md` §4

## Overview

### Problem Statement
The product premise is "the markdown files ARE the database", but the served read paths never re-check the files:

- **ReadPool readers skip staleness entirely.** Pool readers are built with `skip_stale_check=true` (`ddb-server/src/read_pool.rs`, `ddb-core/src/service/mod.rs:110-124`) and trust the actor to have rebuilt. Staleness detection itself is HEAD-oid comparison only (`ddb-core/src/indexer/rebuild.rs:12-24`). Net effect: a change **committed outside ddb** (editor + `git commit`, a cron job, another tool) stays invisible to GraphQL and PgWire SELECTs until some unrelated actor write happens to flip HEAD and trigger a rebuild. An **uncommitted** editor edit is invisible indefinitely.
- **Git sync refreshes the index but not the GraphQL schema.** The `Sync` actor handler runs `svc.sync` + `rebuild_if_stale` only (`ddb-server/src/actor/handlers.rs:155-161`); `SchemaReloader` triggers exist only on the applySchema surfaces (the GraphQL mutation, REST schema-apply, and PgWire DDL — all via `trigger_reload_and_wait`, `ddb-server/src/reload.rs`). A `_typedef` arriving via sync updates SQL tables while the GraphQL schema stays stale until a manual `applySchema` or restart; open subscriptions keep their pre-upgrade schema clone (`ws.rs:42`).
- **No written consistency contract.** No doc states what freshness each transport guarantees, so every downstream discovers the gaps by surprise.

> **Note (pre-00172 line references):** the actor citations above describe the twin-enum actor. 00172 deletes `actor/handlers.rs` (closure messages) and 00174 moves Sync/Compact onto a dedicated maintenance worker — both land well before this PRD. Read "the Sync actor handler" as "the sync execution path (a closure running on the 00174 maintenance worker)" and re-ground exact locations at execution time (frontmatter `catchup: force` is set for this reason).

Maintainer decision (2026-07-13): fix with a cheap read-path staleness probe (committed = visible, enforced) plus an **opt-in** watch mode that auto-commits external edits (files-are-the-database extends to live edits when the operator asks for it), plus the written contract.

### Target Users
Anyone who edits doogats with a text editor or external tool while `ddb serve` runs (the core markdown promise); downstream apps (jink) whose clients otherwise render stale data with no signal; multi-device users whose synced-in typedefs currently leave GraphQL schemas stale per device.

### Success Metrics
- A change committed outside ddb is visible to the next GraphQL/PgWire read without any intervening ddb write — pinned by a server test that commits externally, then queries.
- With `--watch` enabled, an external file save under `ddb/` becomes a commit within the debounce window and is then served — pinned by an integration scenario.
- A `_typedef` arriving via `sync` triggers a GraphQL schema rebuild in the same operation — pinned by a test asserting the schema version bumps after sync.
- `docs/src/` contains a per-transport consistency contract page; every freshness guarantee stated there is enforced by a test in this PRD.
- Probe overhead on the hot read path is one HEAD-oid read + one meta compare (microseconds); no full-rebuild work happens on fresh reads.

## Functional Decomposition

### Capability: Read-path staleness probe
Committed = visible, enforced on every served read.

#### Feature: HEAD-oid probe on pool reads
- **Description**: before executing a ReadPool read, compare repo HEAD to the index's stored head; on mismatch, get the index rebuilt before serving.
- **Inputs**: repo HEAD oid; `_ddb_meta` stored head.
- **Outputs**: a result at least as fresh as the captured HEAD, or READ_FRESHNESS_TIMEOUT through the sole core policy (Conflict category, HTTP 409, FFI Conflict hint, retryable string detail "true", fixed non-secret message). No stale successful result.
- **Behavior**: replace unconditional `skip_stale_check=true` with a cheap probe. On stale, delegate the rebuild to the single rebuild owner (actor / the 00169 rebuild lock, `locked_rebuild` at `indexer/rebuild.rs:538`) and wait bounded (a NEW 5 s constant — `ReadPool::acquire` is unbounded today, `read_pool.rs:234-243`; the only 5 s in that file is a test assertion at `:439`, grounded 2026-09-05); on timeout, return a structured retryable freshness failure with no stale success result. Never rebuild concurrently from pool readers.

#### Feature: Actor-path staleness verification
- **Description**: verify (and pin with a test) that actor-routed reads (NoSQL API, non-SELECT paths) already run `ensure_fresh`, so the probe closes the last gap rather than adding a second mechanism.
- **Inputs**: existing `ensure_fresh` call sites (`service/mod.rs:116-124`).
- **Outputs**: a test proving actor reads see externally committed changes; a documented statement.
- **Behavior**: if verification finds an actor read path skipping `ensure_fresh`, route it through the same probe policy.

### Capability: Watch mode (opt-in)
External edits become commits without manual `git commit`.

#### Feature: Debounced auto-commit watcher
- **Description**: `ddb serve --watch` (and standalone `ddb watch`) watches `ddb/**` and `ddb/_typedef/**` markdown files and commits external changes after a debounce.
- **Inputs**: fs events (notify crate, a new dependency — `design-decisions.md:111` and the table row at `:121` already name fsmonitor/file-watcher support as planned); debounce interval (default 500 ms, flag-tunable).
- **Outputs**: one commit per debounce window batching all changed/deleted files, message `chore: absorb external edits`; index rebuild follows via the normal HEAD-change path.
- **Behavior**: events are collected per window; writes go through the existing cross-process write lock and `fresh_index()` commit path (same invariants as every git write); ddb's own writes are not re-absorbed (skip events while the write lock is held by this process, verify no feedback loop); non-doogat paths (`.ddb/`, `.crdt/`, `.nodes/`) are ignored; parse-invalid files are still committed (git is truth) but reported as structured warnings, consistent with the poison-file policy (00169).

### Capability: Schema reload on out-of-band typedef changes
The GraphQL schema follows git truth, not just `applySchema`.

#### Feature: Sync- and watch-triggered schema reload
- **Description**: any actor operation that changes HEAD from outside `applySchema` (sync, watch auto-commit, probe-triggered rebuild) triggers `SchemaReloader` when the change set touches `ddb/_typedef/`.
- **Inputs**: the changed-path set of the applied commits (sync merge result / watch commit / rebuild delta).
- **Outputs**: a rebuilt schema hot-swapped via the existing `ArcSwap` + version bump (`reload.rs:112-113`).
- **Behavior**: detect typedef paths in the delta; call `trigger_reload_and_wait` after the index rebuild completes; document (in the contract page) that open WebSocket subscriptions keep their captured schema until reconnect.

### Capability: Consistency contract
One page that says what each interface guarantees.

#### Feature: Per-transport consistency contract doc
- **Description**: a `docs/src/technical/consistency.md` page stating the freshness guarantee per transport.
- **Inputs**: the enforced behaviors above.
- **Outputs**: the page, linked from `SUMMARY.md`, `server.md`, and `indexer.md`.
- **Behavior**: states committed = visible (probe-enforced); uncommitted = not yet data unless watch mode absorbs it; schema follows typedef truth on sync/watch; subscription events are best-effort (broadcast capacity 256 drops for slow clients, existing known limit); CLI checks staleness per operation. A reader that cannot reach the HEAD snapshot captured by its probe within five seconds fails visibly and is safe to retry; no successful stale-read fallback exists. Every stated guarantee cites the test that pins it.

## Structural Decomposition

### Repository Structure

```
ddb-core/src/
├── indexer/rebuild.rs     # Maps to: HEAD-oid probe (staleness primitives reused, single rebuild owner)
├── service/mod.rs         # Maps to: HEAD-oid probe (skip_stale_check policy replaced by probe policy)
└── git_ops/               # Maps to: Watch mode (commit path reused; no new write path)
ddb-server/src/
├── read_pool.rs           # Maps to: HEAD-oid probe on pool reads
├── actor/                 # Maps to: Sync-triggered schema reload; actor-path verification (post-00172 closure dispatch; sync runs on the 00174 maintenance worker — handlers.rs no longer exists by this PRD)
├── watch.rs               # Maps to: Debounced auto-commit watcher (new)
├── reload.rs              # Maps to: Schema reload on out-of-band typedef changes
└── lib.rs                 # Maps to: --watch wiring
ddb-cli/src/
├── main.rs                # Maps to: --watch / ddb watch flags
└── commands/maintenance.rs # Maps to: ddb watch command handler
docs/src/technical/
└── consistency.md         # Maps to: Per-transport consistency contract doc (new)
```

### Module: read_pool (ddb-server/src/read_pool.rs, capabilities.rs; ddb-core/src/app_contract/error.rs)
- **Maps to capability**: Read-path staleness probe
- **Responsibility**: probe before execute; delegate rebuilds; never rebuild in-pool. Also own the read_freshness registry entry and the core timeout policy row.
- **Exports**: existing `execute_select`/read entry points, now probe-guarded.

### Module: watch (ddb-server/src/watch.rs, new)
- **Maps to capability**: Watch mode
- **Responsibility**: fs events → debounced batch → locked commit → normal rebuild/reload flow.
- **Exports**: `spawn_watcher(...)` used by `serve --watch` and `ddb watch`.

### Module: reload + sync path (ddb-server/src/reload.rs, the post-00172/00174 sync execution path)
- **Maps to capability**: Schema reload on out-of-band typedef changes
- **Responsibility**: typedef-delta detection and reload trigger after sync/watch/rebuild.
- **Exports**: `trigger_reload_and_wait` (existing), typedef-delta helper (new).

### Module: docs/consistency (docs/src/technical/consistency.md, new)
- **Maps to capability**: Consistency contract
- **Responsibility**: the single page stating per-transport freshness guarantees, each citing its pinning test.
- **Exports**: the contract page.

### Module: core policy and watch wiring (ddb-core/src/service/mod.rs, indexer/rebuild.rs, git_ops/; ddb-server/src/actor/, lib.rs; ddb-cli/src/main.rs, commands/maintenance.rs)
- **Maps to capability**: Read-path staleness probe; Watch mode (opt-in); Schema reload on out-of-band typedef changes
- **Responsibility**: expose one freshness policy, preserve single-owner rebuild and repo locks, wire opt-in watch, and reload after actual typedef changes.
- **Exports**: freshness results/errors, watch CLI flags, and existing actor/service adapters.


## Dependency Graph

### Foundation Layer (Phase 0)
No dependencies - built first.

- **Probe policy in core**: staleness probe replacing blanket `skip_stale_check`, rebuild delegation (coordinates with the 00169 single-owner rebuild lock).

### Core Layer (Phase 1)
- **read_pool probe wiring**: Depends on [probe policy].
- **typedef-delta reload trigger (sync path)**: Depends on [probe policy] (shares the HEAD-delta plumbing).
- **watch module**: Depends on [probe policy] (its commits are picked up by the same probe/rebuild flow).

### Integration Layer (Phase 2)
- **contract doc + scenarios + e2e + walkthrough**: Depends on [read_pool probe, reload trigger, watch module].

## Implementation Phases

### Phase 0: Foundation
**Goal**: one probe policy, one rebuild owner.

**Tasks**:
- [ ] Implement the probe policy in core: cheap HEAD-vs-meta check exposed to read paths, rebuild delegated to the single owner with a bounded wait (5 s; a new constant — no production ReadPool queue timeout exists) and READ_FRESHNESS_TIMEOUT registered once in 00173's core policy (Conflict/409/FFI Conflict; retryable detail "true") (depends on: 00173) - Acceptance: unit tests cover fresh (no rebuild work), stale (rebuild then serve), and timeout (structured retryable failure, no stale data returned) paths.
- [ ] Verify actor-routed reads already `ensure_fresh` (NoSQL API included); pin with a test; route any gap through the probe policy (no deps) - Acceptance: a test proves an externally committed change is visible to an actor-routed read with no intervening ddb write.

**Exit Criteria**: probe policy exists, tested; exactly one rebuild owner.

### Phase 1: Core
**Goal**: served reads honor git truth; schema follows synced typedefs; watch mode lands.

**Tasks**:
- [ ] Wire the probe into ReadPool reads, replacing unconditional `skip_stale_check=true` (depends on: Phase 0) - Acceptance: a server test commits a change externally, then a GraphQL query and a PgWire SELECT both return it with no intervening mutation; hot-path overhead is probe-only on fresh reads; timeout fixtures pin structured retryable failures on GraphQL, REST, PgWire, and actor/NoSQL reads with no stale payload.
- [ ] Trigger `SchemaReloader` from the sync execution path (the sync closure on the 00174 maintenance worker) when the synced delta touches `ddb/_typedef/` (depends on: Phase 0) - Acceptance: a test syncs in a typedef and asserts the schema version bumps and a new-schema query succeeds without `applySchema`.
- [ ] Implement watch mode (`ddb serve --watch`, `ddb watch`): notify-based watcher, 500 ms debounce (tunable), one batched commit per window via the write lock, deletions staged, self-write feedback excluded, typedef changes trigger reload (depends on: Phase 0) - Acceptance: integration test saves a file externally, sees one commit within the window, then reads it via GraphQL; a ddb-initiated write produces no absorb commit.

**Exit Criteria**: externally committed edits are served; synced typedefs reload the schema; watch mode absorbs live edits.

### Phase 2: Integration
**Goal**: contract published, scenarios authored, deliverables complete.

**Tasks**:
- [ ] Write `docs/src/technical/consistency.md` (per-transport guarantees, each citing its pinning test; subscription drop caveat; uncommitted-edit stance); link from `SUMMARY.md`, `server.md`, `indexer.md` (depends on: Phase 1) - Acceptance: page exists; every stated guarantee names a test landed in this PRD.
- [ ] Author the scenarios in the Rust e2e harness (`tests/e2e/`; the shell harness was removed by 00171): external commit visible; watch absorb; sync typedef reload; plus the `--watch` e2e, a showboat walkthrough for watch mode in `dev/local/walkthroughs/`, and a CHANGELOG entry (depends on: Phase 1) - Acceptance: e2e scenarios authored; walkthrough executes; CHANGELOG updated. Include the `read_freshness` registry addition and update exact-set plus health/GraphQL agreement fixtures; Acceptance also requires the flag to match compiled availability and the implemented workflow.

**Exit Criteria**: contract page + scenarios + e2e + walkthrough + CHANGELOG landed; push `master`.

## Test Strategy

**Validation:** per-task local gates are `cargo build`, `cargo clippy --workspace --all-targets`, and `cargo test-ci`, including colocated unit regressions. Author required e2e/integration/conformance scenarios and executable walkthroughs; their heavy execution belongs to Tier 2 CI, not the per-task gate. Retain only CLAUDE.md's conditional e2e run for Claude-routed tasks deleting/replacing code paths. Walkthrough creation uses showboat; any verification uses `dev/bin/safe-showboat-verify`, never the project root. This backlog assumes 00171 has completed the shell-to-Rust harness migration.

### Critical Scenarios
- **Happy path**: external `git commit` of an edited doogat, then a GraphQL query → Expected: the edit is returned, no ddb write needed in between.
- **Edge case**: rapid successive saves within one debounce window under `--watch` → Expected: exactly one absorb commit containing the final content; index and schema (if typedef) follow.
- **Error case**: probe finds the index stale but the rebuild owner is busy past the bounded wait → Expected: the read returns a structured retryable freshness failure, with no stale result and no concurrent rebuild from the pool.

## Risks

- **Probe latency on hot reads**: mitigated by probing oids only (no tree walk); benchmark before/after on the search path; the fresh path does zero rebuild work.
- **Rebuild races (00169 rebuild lock)**: the probe must delegate, never rebuild in-pool. Mitigation: depend on 00169's single-owner rebuild lock; bounded timeout failures avoid pile-ups.
- **Watcher feedback loop** (ddb write → fs event → absorb commit): mitigation is explicit self-write exclusion plus a regression test asserting no absorb commit follows a ddb-initiated write.
- **Partial saves / editor temp files**: debounce plus markdown-only path filter; parse-invalid content still commits (git is truth) but warns per the poison-file policy; document the behavior.
- **Platform fs-event differences (macOS/Linux/Windows)**: use the notify crate's cross-platform backends; integration scenarios run in CI's cross-platform matrix (Tier 2).
- **Typedef catalog lag for write-side validators (deferred from FT-5 review, 2026-09-14, FT5-BOB-01 / Victor LOW)**: typedef registration commits to Git then indexes as a separate unlocked step (`sql_engine/ddl.rs` `handle_create_table`: `commit_file` then `index_doogat`). `DoogatService::list_type_schemas` discovers typedefs through `SELECT path FROM doogats WHERE type = '_typedef'`, so a writer whose `ensure_fresh` ran before that commit can validate against a catalog missing a just-committed SINGLETON/UNIQUE typedef. Pre-existing on every create/update validator; FT-5 narrowed, not widened, it. The consistency contract page must state this window, and closing it means making typedef registration atomic with indexing or discovering typedefs from the Git tree; neither fits a write-path item's allowlist.

**Capability registry (depends on 00186):** add `read_freshness` to the existing sorted registry only with this implemented surface; document the applicable interface/workflow and compile-time availability. Internal refactors and tests are not separate flags.

## Transport Impact

- **CLI**: Guaranteed — already staleness-checked per operation; gains `ddb watch` / `serve --watch`.
- **GraphQL**: Guaranteed — pool reads probe-guarded; schema follows synced/watched typedefs.
- **REST**: Guaranteed — served from the same actor/pool paths; inherits the probe.
- **PgWire**: Guaranteed — SELECTs run on the probe-guarded pool; DML already actor-fresh.
- **FFI**: Guaranteed — in-process calls run `ensure_fresh` (verified in Phase 0); the contract page states the FFI freshness rule explicitly.
- **NoSQL HTTP**: Guaranteed — actor-routed reads verified fresh in Phase 0.
- **Cross-interface conformance plan**: one external-commit-visibility test each for GraphQL, PgWire SELECT, REST, and NoSQL (actor path); the contract page maps each guarantee to its test; sync-typedef reload pinned by schema-version test. jink impact: clients stop rendering stale data after out-of-band edits, with a documented contract to build against.
