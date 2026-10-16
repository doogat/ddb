# Unify ID minting through one repo-aware path

**Priority:** P1 · **Roadmap:** Phase 1 (convergence) · **Depends on:** none (cross-references 00170, which owns the ID format/throughput ceiling — 00164 is the correctness fix only and must NOT change the 14-digit second-resolution public format)

> **Maintainer direction (2026-07-13, purpose-fit critique):** 00170's outcome is pre-decided as a format change (UTC mint + node discriminator; see the superseding note in 00170). 00164 stays format-preserving and lands first as planned; build the existence oracle so the later format change reuses it unchanged.

> **Scope addition (2026-07-16 assessment; path corrected at the backlog review):** while unifying minting, give `DoogatId` one shape validator (`is_valid_shape()`: 14 ASCII digits) and route the four divergent re-implementations through it — `ddb-core/src/attachments.rs:31` (`validate_doogat_id_format`), `ddb-server/src/lib.rs:205`, `consistency/mod.rs:335` (prefix variant), `parser/mod.rs:230` (no length check). Evidence: evolution-assessment-2026-07-16.md L8.

## Overview

### Problem Statement
DoogatIds are minted from three paths that disagree on what "already taken" means, so two mints in the same wall-clock second can silently overwrite each other on a single device:

- **Raw FFI create skips the repo check**: `create_doogat_raw` (`ddb-core/src/service/create.rs:421`) falls back to `parser::generate_id()` when the content has no `id` (`create.rs:429`), and `generate_id()` is `generate_unique_id(|_| false)` (`ddb-core/src/parser/mod.rs:927-929`) — the exists-closure always returns false, so nothing consults the working tree or index.
- **Bundled-type install skips the repo check**: `install_bundled_type` (`ddb-core/src/service/discovery.rs:89`) mints via the same `parser::generate_id()` (`|_| false`).
- **The generator only dedups within one process**: `generate_unique_id` (`parser/mod.rs:933-947`) dedups against a per-process `static LAST: Mutex<String>` and mints from `chrono::Local::now()`, spin-waiting 100 ms until the wall second advances. A second process — or any caller passing `|_| false` — shares no state with it.
- **Batch INSERT claims future seconds**: `unique_ids(count)` (`ddb-core/src/sql_engine/mod.rs:127-168`) takes a base second then allocates `+1 second` per additional row (`mod.rs:149`), checking each candidate only against the `doogats` index table (`mod.rs:154-158`), never the working tree. A 300-row INSERT reserves ~5 minutes of future ID space; a later single create that lands in that window overwrites a batch row.
- **The correct minter already exists**: `unique_id()` (`ddb-core/src/service/write_helpers.rs:242-258`) is repo-aware — its closure checks `ddb/{candidate}.md` and every type-folder subdirectory on disk. This is the target all mint paths should route through.

### Target Users
Downstream apps (jink) that mix `createMany` batch inserts with single `createDoogat` calls, and FFI/mobile consumers using `create_doogat_raw`; both currently risk silent same-second overwrite.

### Success Metrics
- Two different mint paths invoked in the same wall-clock second produce distinct IDs and never overwrite an existing doogat — pinned by a regression test that mints via the raw-create path and the batch path back-to-back and asserts distinct, non-colliding IDs.
- A batch INSERT of N rows does not reserve a future second that a subsequent single create then collides with — pinned by a test: INSERT N rows, then single-create, assert the single create's ID equals no batch row's ID and no batch row content was overwritten.
- Raw create, bundled-type install, and batch insert all consult real working-tree + index existence (no `|_| false`, no blind future-second walk).
- Public format unchanged: every minted ID is still a 14-digit `YYYYMMDDHHmmss` string; existing format/parse tests stay green.

## Functional Decomposition

### Capability: Repo-aware unique ID minting

#### Feature: Single repo-aware existence oracle
- **Description**: one existence check (working tree + index) that every mint path shares, generalizing `write_helpers::unique_id`.
- **Inputs**: the service repo path + index handle; optional type/folder for path computation.
- **Outputs**: a closure/`fn` that returns true when a candidate ID already exists anywhere durable.
- **Behavior**: extend the `unique_id` filesystem closure to also consult the `doogats` index, and expose it so create/discovery/batch reuse it instead of `|_| false`.

#### Feature: Repo-aware raw-create and bundled-install minting
- **Description**: `create_doogat_raw` and `install_bundled_type` mint through the shared oracle.
- **Inputs**: parsed content (may carry an `id`); type/folder.
- **Outputs**: a fresh ID absent from working tree and index.
- **Behavior**: replace the `parser::generate_id()` fallback at `create.rs:429` and `discovery.rs:89` with the repo-aware minter; an author-supplied `id` still wins unchanged.

#### Feature: Non-future-dating batch minting
- **Description**: `unique_ids(count)` allocates N IDs by check-and-advance against the repo, not by pre-claiming a fixed future window.
- **Inputs**: count; the shared existence oracle.
- **Outputs**: N distinct IDs; the reserved window is contiguous from now and consistent with what single creates observe.
- **Behavior**: mint each candidate and advance only past genuinely-occupied seconds (checking working tree + index), so a later single create routed through the same oracle sees the batch's IDs. Eliminating future seconds entirely for large bursts is the format/throughput ceiling owned by 00170; this PRD makes the reservation consistent, not sub-second.

## Structural Decomposition

### Repository Structure

```
ddb-core/src/service/
├── write_helpers.rs   # Maps to: Single repo-aware existence oracle (generalize unique_id to check index + working tree; shared entry point)
├── create.rs          # Maps to: Repo-aware raw-create minting (create_doogat_raw:429 routes through the oracle)
├── discovery.rs       # Maps to: Repo-aware bundled-install minting (install_bundled_type:89 routes through the oracle)
ddb-core/src/sql_engine/
└── mod.rs             # Maps to: Non-future-dating batch minting (unique_ids:127-168 check-and-advance against the oracle)
ddb-core/src/parser/
└── mod.rs             # Maps to: Single repo-aware existence oracle (generate_unique_id stays the low-level primitive; callers stop passing |_| false)
```

### Module: write_helpers (ddb-core/src/service/write_helpers.rs)
- **Maps to capability**: Repo-aware unique ID minting
- **Responsibility**: own the shared existence oracle (working tree + index) and the service-level minter.
- **Exports**:
  - `unique_id(...)` - now checks index in addition to the filesystem
  - the existence closure - reused by create/discovery/sql_engine

### Module: create + discovery (ddb-core/src/service/create.rs, discovery.rs)
- **Maps to capability**: Repo-aware unique ID minting
- **Responsibility**: route raw create and bundled-type install through the shared minter.
- **Exports**: `create_doogat_raw`, `install_bundled_type` - repo-aware minting

### Module: sql_engine (ddb-core/src/sql_engine/mod.rs)
- **Maps to capability**: Repo-aware unique ID minting
- **Responsibility**: allocate batch IDs by check-and-advance against the shared oracle instead of pre-claiming a future window.
- **Exports**: `unique_ids(count)`, `unique_id()` - consistent with single-create minting

## Dependency Graph

### Foundation Layer (Phase 0)
No dependencies - built first.

- **write_helpers existence oracle**: the shared repo-aware check every path will reuse.

### Core Layer (Phase 1)
- **create + discovery routing**: Depends on [write_helpers oracle] - swap the `|_| false` fallbacks.
- **sql_engine batch minting**: Depends on [write_helpers oracle] - inject/reuse the oracle; drop blind future-dating.

### Integration Layer (Phase 2)
- **tests + docs + scenarios**: Depends on [create, discovery, sql_engine].

## Implementation Phases

### Phase 0: Foundation
**Goal**: one repo-aware existence oracle.

**Tasks**:
- [ ] Generalize the `write_helpers::unique_id` existence closure to also consult the `doogats` index, and expose it for reuse (no deps) - Acceptance: a unit test proves the oracle reports an on-disk-only ID and an index-only ID both as taken.

**Exit Criteria**: a single oracle answers "taken?" across working tree + index.

### Phase 1: Core
**Goal**: every mint path uses the oracle; batch stops future-dating blindly.

**Tasks**:
- [ ] Route `create_doogat_raw` (`create.rs:429`) and `install_bundled_type` (`discovery.rs:89`) through the repo-aware minter (depends on: Phase 0) - Acceptance: neither path passes `|_| false`; a test mints via raw create twice against a repo already holding the current-second ID and gets a fresh, non-colliding ID.
- [ ] Convert `unique_ids` to check-and-advance against the oracle; drop the fixed `+1s` index-only walk (depends on: Phase 0) - Acceptance: a batch of N rows plus an immediate single create yield N+1 distinct IDs with no overwritten content; the reservation window is contiguous from now.

**Exit Criteria**: all mint paths agree on existence; no silent same-second overwrite.

### Phase 2: Integration
**Goal**: lock the behavior and document it.

**Tasks**:
- [ ] Add core regression tests: (a) two mint paths in one second never overwrite; (b) batch-then-single never collides (depends on: Phase 1) - Acceptance: both fail against the old code and pass after.
- [ ] Add an `integration.sh`/`.ps1` scenario exercising `createMany` followed by a single create + assert distinct IDs; update `docs/src/technical/data-model.md` (ID minting) + `docs/src/technical/sql-engine.md` (batch INSERT minting) + CHANGELOG (depends on: Phase 1) - Acceptance: scenario green; docs describe unified repo-aware minting. No new CLI command or user-visible behavior, so no smoke command scenario and no walkthrough are required.

**Exit Criteria**: regression tests + integration scenario + docs landed.

## Test Strategy

### Critical Scenarios
- **Happy path**: create N via batch INSERT, then one single create → Expected: N+1 distinct IDs, every batch row intact.
- **Edge case**: two `create_doogat_raw` calls inside the same wall-clock second against a repo that already holds that second's ID → Expected: the minter advances; distinct IDs, no overwrite.
- **Error case**: a batch that would previously have claimed a future second later hit by a single create → Expected: the single create observes the batch's IDs and advances; no content loss.

## Risks

- **SqlEngine repo access**: `unique_ids` currently sees only the index. Mitigation: inject the shared existence oracle (closure) rather than a new adapter construction inside the service; keep the domain boundary intact.
- **Throughput at second resolution**: large bursts still advance into near-future seconds because the public format is second-resolution. Mitigation: scope 00164 to consistency only; the sub-second/throughput ceiling is 00170. Document the boundary.
- **Fourth mint path already repo-aware**: `reassign_single_loser` (`sync_manager/mod.rs:676-711`) mints via `generate_unique_id` with a repo-aware closure — correct for existence but wall-clock for the timestamp. Out of scope here; determinism of that path is 00167. Noted so it is not mistaken for a regression.

## Transport Impact

- **CLI**: Guaranteed — `ddb create` / batch INSERT route through the unified minter; no surface change, internal correctness.
- **GraphQL**: Guaranteed — `createDoogat` / `createMany` mint through the unified path.
- **REST**: Guaranteed — REST create/batch passthrough uses the same core path.
- **PgWire**: Guaranteed — `INSERT` uses `unique_ids`, now check-and-advance.
- **FFI**: Guaranteed — `create_doogat_raw` becomes repo-aware (the primary fix site).
- **NoSQL HTTP**: Intentionally absent — read-only document mirror; no create surface, so no minting.
- **Cross-interface conformance plan**: a core regression test proves two mint paths in one second never overwrite; a batch-then-single test proves no future-second collision; the `integration.sh`/`.ps1` scenario exercises CLI `createMany` + single create across the same repo. No public response/error shape changes; no new transport surface.
