---
design: skip
catchup: force
default_model: sonnet
model_tier_rationale: deletions and a gated migration with named regression tests; the classifier escalates contract edits; catchup forced because 00183 splits crdt_resolver.rs and git_ops/mod.rs
---

# Prune dead abstractions and close small fail-loud gaps

**Priority:** hygiene · **Roadmap:** appended 2026-07-16 (last) · **Depends on:** none (each requirement independent; if 00183 landed first, re-verify line numbers). (`design: skip`: independent micro-fixes; a design doc adds only ceremony.)

Source: `dev/local/audit-results/evolution-assessment-2026-07-16.md` F13, L4-L9 (lenses 1, 2, 6).

## Problem

The assessment's simplification and safety lenses surfaced a cluster of small, independent items: dead abstractions, a trust-destroying auto-migration behavior (`GitRepo::open` commits a migration file into ANY repo it is pointed at — the stray `.ddb-version` at ddb's own source root is the proof), duplicated micro-helpers, and silent-failure edges. None justifies its own PRD; together they are one session of cleanup that removes ~120 LOC and closes three fail-loud gaps.

## Solution

One pass of independent, individually-tested fixes: gate the auto-migration on positive ddb-repo evidence, delete the two dead traits, make the corrupt-config path fail loud, settle the `updated_at` semantics, dedupe the micro-helpers, and document the gitfile-layout lock limitation.

## Requirements

### Must have

- `GitRepo::open` performs the v0→v1 auto-migration ONLY when the repo shows positive ddb evidence (`ddb/` present in HEAD); opening a non-ddb git repo performs no write and no error on read paths. Premise: `.ddb-version` at ddb's source root is auto-migration pollution, not a live marker (it was cleaned in `28a37ea` and auto-recommitted in `e3306a3`) — re-check at execution; after the gate lands, remove the stray file. Regression test: opening a non-ddb repo writes nothing; a genuine v0 ddb repo (has `ddb/` in HEAD, no `.ddb-version`) still migrates.
- Dead trait `TypedMaterializationPort` (`traits.rs:324-347` + pass-through impl `indexer/ports.rs:328-344`; zero consumers, both methods already on `IndexPort`) deleted; `docs/src/architecture/modules.md:13-14,:74` (lists it) updated.
- Dead trait `ConflictResolver` + never-constructed `DefaultResolver` (`traits.rs:374-375`, `crdt_resolver.rs:860-870`; sole production caller uses the free function `sync_manager/mod.rs:529`) deleted; modules.md rows (`:74`, `:79`) updated. (Line refs at `8fe51c9`; 00183 splits `crdt_resolver.rs` and `git_ops/mod.rs` first — re-ground at execution.)
- `set_auto_maintenance` stops silently replacing a corrupt config: `load_config().unwrap_or_default()` (`service/ops.rs:63`) propagates the TOML parse error instead. Unit test with malformed TOML.
- `SearchResult.updated_at` DECIDED (2026-09-05 backlog review): derive it from the git commit time of the file's last change, resolved at index time. Today it is `Utc::now()` stamped at index time (`indexer/mod.rs:300,:307`, read back at `indexer/search.rs:454`); the commit-time cascade already exists in `indexer/graph.rs:938-952` (`resolve_last_date`). A unit test pins that an externally committed edit sets `updated_at` to that commit's time; document the semantic and the reindex cost (one revwalk per file) in `docs/src/technical/indexer.md` + `docs/src/guide/building-apps.md`; CHANGELOG `Changed` (public field semantic on every hit-serving transport).

### Nice to have

- Micro-dedupe: keep one `capitalize` per crate or move to core (`ddb-server/src/schema/base_types.rs:903` vs `capitalize_first` at `consistency/mod.rs:354`); rename `pluralize` → `data_key_plural` with a contract comment (its "boxs" English is load-bearing for stored data keys — do NOT fix the English; `pluralize_preserving_case` stays the display pluralizer). (Grounding 2026-08-14: the previously-listed "CLI duplicate `date:`-defaulting at `main.rs:683`" does not exist — that line is a log-filename date; `date:` defaulting lives only in core at `service/create.rs:237` / `batch.rs:345` — item dropped. Note `pluralize`/`capitalize` live in ddb-server, `capitalize_first` in ddb-core.)
- Document the gitfile-layout lock limitation (`write_lock::acquire` is layout-agnostic since `f9c1e26`, but its sole caller `git_ops/mod.rs:315` passes `<repo>/.git` as the lock dir and `create_dir_all` fails on a gitfile `.git`; worktree/submodule checkouts fail loud on every write) in `docs/src/technical/git-ops.md` known-limitations; while touching the file, check whether `fs2` → `fs4` (maintained fork) is a drop-in and switch only if it is.

## Implementation

### Module: git_ops open/migration (ddb-core/src/git_ops/mod.rs:181-226)
- **Location**: `ddb-core/src/git_ops/`
- **Responsibility**: evidence-gated auto-migration; no writes into non-ddb repos.
- **Exports**: `GitRepo::open` (same signature, gated behavior).

### Module: dead-trait removal (ddb-core/src/traits.rs, indexer/ports.rs, crdt_resolver.rs)
- **Location**: `ddb-core/src/`
- **Responsibility**: pure deletions; no behavior change (nothing consumes them).
- **Exports**: none added; `TypedMaterializationPort`, `ConflictResolver`, `DefaultResolver` removed.

### Module: ops + docs (ddb-core/src/service/ops.rs, docs/src/technical/)
- **Location**: `ddb-core/src/service/`, `docs/src/technical/`
- **Responsibility**: fail-loud config load; `updated_at` + gitfile-limitation documentation.
- **Exports**: `set_auto_maintenance` (error-propagating).

### Dependencies
- git_ops gate: no dependencies (foundation; the stray-file removal depends on the gate landing first).
- dead-trait removal, ops fail-loud, micro-dedupe, docs: independent of each other and of the gate.

## Tasks

### Phase 0: Foundation
- [ ] Gate `GitRepo::open` auto-migration on `ddb/` present in HEAD; regression tests both directions (non-ddb repo untouched; genuine v0 ddb repo still migrates); CHANGELOG `Fixed` - Acceptance: both tests pass, the non-ddb test fails on old code.
- [ ] Re-check the source-root .ddb-version premise after the gate lands; remove it only if present and verified as pollution, otherwise skip and report (depends on: the gate) - Acceptance: a fresh open of a non-ddb fixture writes nothing; the note records whether the source-root marker was absent, removed as pollution, or retained with evidence of legitimate use.

### Phase 1: Core
- [ ] Re-check consumers/constructors of TypedMaterializationPort, ConflictResolver, and DefaultResolver after 00183; delete only still-unused abstractions and update modules.md (no deps) - Acceptance: workspace builds and existing behavior is preserved; rg confirms no removed-symbol consumers; any newly used abstraction is retained with the changed premise reported.
- [ ] Propagate the config parse error in `set_auto_maintenance`; unit test with malformed TOML - Acceptance: corrupt config errors loudly instead of being replaced; test fails on old code.
- [ ] Derive `SearchResult.updated_at` from git commit time at index time; document the semantic + reindex cost; CHANGELOG `Changed` - Acceptance: a test pins that an externally committed edit sets `updated_at` to that commit's time; docs state the semantic and the cost.
- [ ] Update indexer/building-apps guidance and compatibility notes for updated_at, document non-ddb open and corrupt-config failures, and author e2e scenarios plus a showboat walkthrough for these user-facing fixes (depends on: their implementation tasks) - Acceptance: documentation and executable demo describe actual behavior; e2e scenarios are authored for CI, and semantic changes are recorded in CHANGELOG.

### Phase 2: Integration
- [ ] Optional micro-dedupe (capitalize, pluralizer rename with contract comment) - Acceptance: behavior-preserving; existing suites green; the data-key contract comment explains why "boxs" stays.
- [ ] Optional gitfile-limitation docs + fs2/fs4 equivalence check; change the dependency only if build/locking behavior is proven equivalent, otherwise retain fs2 and record why; apply CLAUDE.md's conditional e2e deletion safety net in Claude-routed sessions (this PRD deletes code paths per CLAUDE.md) - Acceptance: docs landed; fs decision recorded; e2e green.

## Success Criteria

**Validation:** per-task local gates are `cargo build`, `cargo clippy --workspace --all-targets`, and `cargo test-ci`, including colocated unit regressions. Author required e2e/integration/conformance scenarios and executable walkthroughs; their heavy execution belongs to Tier 2 CI, not the per-task gate. Retain only CLAUDE.md's conditional e2e run for Claude-routed tasks deleting/replacing code paths. Walkthrough creation uses showboat; any verification uses `dev/bin/safe-showboat-verify`, never the project root. This backlog assumes 00171 has completed the shell-to-Rust harness migration.

- Opening a non-ddb repo never writes; the stray `.ddb-version` is gone and stays gone.
- Two dead traits removed with zero behavior change; corrupt config fails loud; `updated_at` derived from git commit time and documented; optional micro-dedupe and lock/dependency work completed or explicitly skipped with rationale.
- Tier 1 green; e2e green.

## Transport Impact

No public wire SHAPE changes on CLI/GraphQL/REST/PgWire/FFI/NoSQL HTTP; `SearchResult.updated_at` changes MEANING (git commit time instead of index stamp) on every hit-serving transport — additive semantic change, CHANGELOG'd. The other observable behavior changes are the fixes themselves: opening a non-ddb repo now performs no write (CLI-adjacent, CHANGELOG'd); a corrupt maintenance config now errors instead of being silently replaced (CLI-facing, CHANGELOG'd). Cross-interface conformance plan: not applicable — no new capability surface; the regression tests above are the guard.
