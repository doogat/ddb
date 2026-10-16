---
default_model: opus
model_tier_rationale: cross-cutting shape change - a new read verb and record on CLI, GraphQL, REST, and FFI
---

# Index-health quarantine listing surface

**Priority:** P2 · **Roadmap:** Phase 2 (app contract) · **Depends on:** 00169 (lenient skip + `ConsistencyWarning::UnreadableFile` + the lenient `Index::parallel_parse` scan it relies on — done 2026-08-16)

## Overview

### Problem Statement
After 00169 (done 2026-08-16) incremental reindex is lenient: a poison file is *skipped*, a summary warning rides the triggering verb, `ddb reindex` prints one line per skipped path (`ddb-cli/src/commands/maintenance.rs:65-69` via `describe_consistency_warning`, commit `b3a5263`), and the FFI `RebuildReport.warnings` carries per-file `{code, message}` entries (`ffi/records.rs:173-185`, commit `48b9925`). What is still missing (grounded 2026-09-05): every one of those surfaces enumerates only by RUNNING A REBUILD (`svc.reindex()`/`reindex_strict()`); no side-effect-free listing exists on any transport; GraphQL and REST have no index-health read at all; and FFI fuses path and reason into `message` instead of exposing a structured `path`. An operator who sees "1 file skipped" still has no `ddb doctor` / `indexHealth` to consult without triggering a rebuild.

This PRD adds the net-new read surface that lists quarantined/skipped files (path + reason) with cross-interface parity, so an operator can find and fix the bad file without a manual full rebuild to discover it.

### Target Users
Operators and downstream apps (jink) who need to see which file was quarantined and why, on whichever interface they already use.

### Success Metrics
- A doctor/index-health read surface enumerates the quarantined file(s) (path + reason) without a full-rebuild side effect.
- Parity across CLI, GraphQL, REST, and FFI: the same repo with the same poison file returns the same quarantined path(s) + reason on each.
- `ddb doctor` lists the quarantined paths without a rebuild side effect; the FFI record exposes structured `path` + `reason` fields (not a fused `message`). (`ddb reindex` already names each path — shipped by 00169, out of scope here.)
- No regression: 00169's skip/`--strict` behavior and existing reindex tests stay green.

## Functional Decomposition

### Capability: Quarantine listing
#### Feature: Index-health read surface
- **Description**: a read verb enumerates the currently-unindexable files (path + reason) without a full rebuild side effect.
- **Inputs**: the repository's doogats.
- **Outputs**: a list of `{path, reason}` quarantine entries.
- **Behavior**: reuse the lenient `Index::parallel_parse` scan (`ddb-core/src/indexer/rebuild.rs:397`, returns `(Vec<ParsedDoogat>, Vec<ConsistencyWarning>)`) over all doogats and return its `ConsistencyWarning`s (including 00169's `UnreadableFile`, `types/schema.rs:129`) as structured entries; do not mutate the index.

#### Feature: Cross-interface exposure
- **Description**: the listing is exposed on CLI, GraphQL, REST, and FFI as a side-effect-free read.
- **Inputs**: the `list_index_warnings()` service verb result.
- **Outputs**: `ddb doctor` output, a GraphQL `indexHealth` query, a REST index-health read, and an FFI driver method returning a record with structured `path` + `reason` fields.
- **Behavior**: each transport serializes the same structured `{path, reason}` entries; none triggers a rebuild (`ddb reindex`'s per-path lines, shipped by 00169, are unchanged).

## Structural Decomposition

### Repository Structure

```
ddb-core/src/service/
└── utility.rs             # Maps to: IndexHealthCommand → AppOutput of core path/reason records (drives the lenient scan, read-only)
ddb-core/src/indexer/
└── rebuild.rs             # Maps to: Index::parallel_parse (:397) — the lenient scan the verb reuses (existing, unchanged)
ddb-cli/src/commands/
└── maintenance.rs         # Maps to: `ddb doctor` listing (reindex's per-path lines at :65-69 already exist, unchanged)
ddb-server/src/schema/
└── discovery_queries.rs   # Maps to: GraphQL indexHealth query
ddb-server/src/
└── rest.rs                # Maps to: REST index-health read
ddb-core/src/ffi/
├── driver.rs              # Maps to: FFI index-health driver method
└── records.rs             # Maps to: structured {path, reason} FFI record (RebuildReport.warnings stays as shipped)
```

### Module: service (ddb-core/src/service/utility.rs)
- **Maps to capability**: Quarantine listing
- **Responsibility**: expose the index-health read as a structured, side-effect-free verb.
- **Exports**: list_index_warnings(IndexHealthCommand) -> Result<AppOutput<Vec<IndexHealthEntry>>>; the core entry has path and reason, mirrored losslessly by adapters.

### Module: cli maintenance (ddb-cli/src/commands/maintenance.rs)
- **Maps to capability**: Quarantine listing
- **Responsibility**: `ddb doctor` prints the listing without a rebuild.
- **Exports**: `doctor` command handler

### Module: health contract and adapters (ddb-core/src/app_contract/commands.rs, results.rs; indexer/rebuild.rs; ffi/driver.rs, records.rs; ddb-server/src/schema/discovery_queries.rs, rest.rs)
- **Maps to capability**: Quarantine listing
- **Responsibility**: own one read-only health command/result, reuse lenient parsing, and render structured paths/reasons without per-transport policy.
- **Exports**: IndexHealthCommand, core health result, indexHealth/REST/FFI adapters.

## Dependency Graph

### Foundation Layer (Phase 0)
Depends on 00169 (lenient scan + `UnreadableFile` variant, landed) - built first.

- **list_index_warnings service verb**: reuse the lenient scan behind IndexHealthCommand and return the core entries in AppOutput.

### Core Layer (Phase 1)
- **CLI/GraphQL/REST/FFI exposure**: Depends on [list_index_warnings].

### Integration Layer (Phase 2)
- **parity tests + docs + scenarios + walkthrough**: Depends on [exposure].

## Implementation Phases

### Phase 0: Foundation
**Goal**: a side-effect-free index-health read verb.

**Tasks**:
- [ ] Define IndexHealthCommand and core IndexHealthEntry { path, reason } in app_contract, then implement list_index_warnings through DoogatService → AppOutput<Vec<IndexHealthEntry>> using the read-only lenient scan (depends on: 00169) - Acceptance: a poison file yields its path/reason, _ddb_meta HEAD is unchanged, and core tests pin the command/result contract; 00193's disposition registry must inventory this new command.

**Exit Criteria**: the service verb returns quarantine entries without side effects.

### Phase 1: Core
**Goal**: expose the listing on every interface.

**Tasks**:
- [ ] Decode IndexHealthCommand and render its AppOutput on CLI (`ddb doctor`), GraphQL (`indexHealth`), REST, and FFI (a new record with structured `path` + `reason`; `RebuildReport.warnings` stays as shipped) (depends on: Phase 0) - Acceptance: with a poison file present, each surface returns/prints the quarantined path + reason without running a rebuild (the index's `_ddb_meta` head is unchanged after the call).

**Exit Criteria**: the listing is available on CLI/GraphQL/REST/FFI, side-effect-free.

### Phase 2: Integration
**Goal**: parity, docs, scenarios.

**Tasks**:
- [ ] Add a cross-interface parity test asserting CLI + GraphQL (and REST/FFI where wired) return the same quarantined path(s) + reason; add the scenarios to the Rust e2e harness (`tests/e2e/`; the shell harness is removed by 00171): the `ddb doctor` CLI listing and GraphQL `indexHealth` over a synced poison file (depends on: Phase 1) - Acceptance: parity asserted on CLI + GraphQL; e2e scenarios authored and authored for Tier 2 CI.
- [ ] Update `docs/src/technical/indexer.md` (index-health/doctor surface) + `docs/src/technical/server.md` (indexHealth query) + CHANGELOG; add a showboat walkthrough for `ddb doctor` (new user-facing command) (depends on: Phase 1) - Acceptance: docs + walkthrough landed.

**Exit Criteria**: quarantine listing shipped with parity; docs + scenarios + walkthrough landed.

## Test Strategy

**Validation:** per-task local gates are `cargo build`, `cargo clippy --workspace --all-targets`, and `cargo test-ci`, including colocated unit regressions. Author required e2e/integration/conformance scenarios and executable walkthroughs; their heavy execution belongs to Tier 2 CI, not the per-task gate. Retain only CLAUDE.md's conditional e2e run for Claude-routed tasks deleting/replacing code paths. Walkthrough creation uses showboat; any verification uses `dev/bin/safe-showboat-verify`, never the project root. This backlog assumes 00171 has completed the shell-to-Rust harness migration.

### Critical Scenarios
- **Happy path**: a clean repo → Expected: `ddb doctor` / `indexHealth` returns an empty quarantine list.
- **Edge case**: a poison file present → Expected: `ddb doctor`, GraphQL `indexHealth`, REST, and FFI all list the same path + reason without a rebuild.
- **Error case**: the poison file is fixed and re-synced → Expected: the listing becomes empty again on every surface (no stale quarantine entry).

## Risks

- **Parity drift across four transports**: each surface serializes the list independently. Mitigation: a single `list_index_warnings` service verb is the one source; the parity test asserts identical entries on CLI + GraphQL (and REST/FFI where wired).
- **Scan cost on large corpora**: a full lenient `parallel_parse` scan on every `ddb doctor` call could be slow. Mitigation: the scan is read-only and on-demand (not on the hot path); document the cost; callers who need the hot-path signal use the 00169 verb warning.
- **New public read shape**: `indexHealth` and the REST endpoint add public surface. Mitigation: additive, read-only; document the shape; CHANGELOG note.
- **Two FFI warning shapes**: the shipped `RebuildReport.warnings` (`{code, message}`) and the new structured record coexist. Mitigation: document both; do not retrofit the rebuild report (compat).

## Transport Impact

- **CLI**: Guaranteed — new `ddb doctor` lists quarantined files without a rebuild (`ddb reindex` already names each path, 00169).
- **GraphQL**: Guaranteed — new `indexHealth` query lists quarantined files.
- **REST**: Guaranteed — index-health exposed as a REST read.
- **FFI**: Guaranteed — index-health exposed via the driver with structured `path`/`reason` (the shipped `RebuildReport.warnings` keeps its fused `message`).
- **PgWire**: Intentionally absent — SQL surface has no index-health read; the boundary matches 00169.
- **NoSQL HTTP**: Intentionally absent — read-only document surface; index-health is surfaced via GraphQL/REST/CLI, not the NoSQL read.
- **Cross-interface conformance plan**: with one poison file synced into a repo, the doctor/`indexHealth` listing is asserted equal on CLI and GraphQL (and REST/FFI where wired); e2e scenarios pin the listing surface.
