# Mint doogat IDs from UTC with a per-node suffix

**Priority:** P1 · **Roadmap:** Phase 1 (identity correctness) · **Depends on:** 00170 (the recorded ID-format decision), 00164 (unified ID mint path)

> **Provenance:** the shape is already decided — `docs/src/architecture/design-decisions.md` → "ID Format: UTC Timestamp Plus Per-Node Suffix" (2026-08-16, PRD 00170). This PRD implements it; it does not re-decide it. Evidence trail: `dev/local/notes/00170-id-format-blast-radius.md`. **Premise:** every code citation below is carried from that note (tree state of 2026-08-16) — re-verify each path and symbol at execution; if a premise fails (symbol moved or gone), locate the current equivalent and report, never force the edit.

## Overview

### Problem Statement
Doogat IDs are minted from **local time** at **second resolution** with no per-device component, so two devices can independently mint the *identical* ID with zero coordination, and one device repeats a second across a DST fold. The collision is caught only reactively at merge time by `resolve_add_add_collision` (`ddb-core/src/sync_manager/mod.rs:135`-`:198`) + `derive_content_id` (`ddb-core/src/id_minting.rs:71`-`:97`).

- **The mint spin-waits on the wall clock.** `generate_unique_id` (`ddb-core/src/parser/mod.rs:954`-`:968`) holds a process-global `static LAST: Mutex<String>`, formats a candidate from `chrono::Local::now()` at second resolution, and sleeps 100ms in a loop until the wall second advances. Exactly one distinct candidate exists per second, so the mint caps creates at **~1 per second per process**.
- **The batch path hides the cap instead of fixing it.** `SqlEngine::unique_ids` (`ddb-core/src/sql_engine/mod.rs:126`-`:150`) mints one seed then steps the timestamp `+1s` per additional row, so a 10-row `INSERT` spans ~10 wall-clock seconds of ID space.
- **The ID is the public contract.** It is the filename (`ddb/{id}.md`), the wikilink target (`[[20260226120000]]`), and the `id` value every one of the six interfaces returns. Any format change ripples across all of them and across every stored link.
- **The decision is already made.** Keep the 14-digit `YYYYMMDDHHmmss` prefix, mint it from **UTC**, append a 6-digit zero-padded sub-second field, and append an 8-character lowercase-hex per-node discriminator sourced the way `HlcClock` sources its node id (from `.git/ddb-node`, `ddb-core/src/git_ops/hlc_clock.rs:42`-`:46`) and truncated by `truncate_node` (`ddb-core/src/hlc.rs:123`-`:125`). Full shape: **28 characters**, `^[0-9]{20}[0-9a-f]{8}$`, e.g. `20260816143012042317a1b2c3d4`. This PRD ships that shape.
- **The scheme is collision-*resistant*, not collision-free.** `truncate_node` keeps only the first 8 non-dash characters of the node UUID — 32 bits — so two distinct node UUIDs sharing that prefix produce an identical node field (2^-32 per node pair; ~50% somewhere in a population of ~77,000 nodes). `resolve_add_add_collision` + `derive_content_id` therefore stay wired as the backstop, for the residual case as well as for legacy IDs. No task in this PRD may assert absolute collision-freedom.

### Target Users
Downstream apps (jink) and any bulk importer that needs to mint more than one ID per second, plus every multi-device user who today can silently collide two devices' IDs. **Scope boundary, stated up front:** this PRD lifts the *ID mint* ceiling, not the end-to-end create ceiling. A bulk importer stays bound by the per-doogat git commit (~1s each, `docs/src/technical/performance.md:87`, `:95`) until the write path amortizes many doogats into one commit, which is out of scope here.

### Success Metrics
- The discriminator is pinned at both ends by unit tests, not by argument: (a) two nodes whose `.git/ddb-node` UUIDs **differ within their first 8 non-dash characters** mint disjoint IDs even in the same sub-second slot; (b) a test seeded with two distinct UUIDs that **share** their first 8 non-dash characters (e.g. `a1b2c3d4-1111-...` and `a1b2c3d4-2222-...`) asserts the documented residual — the node fields come out *identical* — so no later reader mistakes the scheme for absolutely collision-free. A "two distinct UUIDs" test does not satisfy (b): it passes trivially and proves nothing.
- The merge-time backstop survives the change: a regression test drives a genuine duplicate-ID add/add case through `resolve_add_add_collision` + `derive_content_id` and asserts it still resolves.
- The mint contains no wall-clock sleep on the common path; a benchmark mints **≥1000 IDs in one process in under one second** (today: ~1/sec, because exactly one candidate string exists per wall second). This metric measures the **ID mint**, not doogat creation — end-to-end `create` stays bound by its per-doogat git commit (~1s, `docs/src/technical/performance.md:87`, `:95`) and this PRD does not change that. The design-decisions estimate is **10^4-10^5 ID mints/sec/process, unbenchmarked** — this PRD replaces the estimate with a measured mint rate in `docs/src/technical/performance.md`.
- A malformed `.git/ddb-node` value never yields an ID that fails `DoogatId::is_valid_shape`: short, non-hex, and dash-only values all resolve to a conforming 8-hex-character node field via the documented fallback, with a structured warning naming the file.
- Every existing 14-digit ID stays valid forever: no repo, file, index row, or stored `[[<id>]]` wikilink is rewritten by the upgrade, and a regression test resolves a legacy 14-digit wikilink after the change.
- `DoogatId::is_valid_shape` accepts both the legacy 14-digit shape and the new 28-character shape, and rejects everything else.
- An ID-format conformance suite asserts the new shape round-trips unmodified through all six public interfaces (CLI, GraphQL, REST, PgWire, FFI, NoSQL HTTP).

## Functional Decomposition

### Capability: Collision-resistant ID minting

#### Feature: UTC-minted timestamp prefix
- **Description**: mint the 14-digit `YYYYMMDDHHmmss` prefix from UTC instead of local time.
- **Inputs**: system clock.
- **Outputs**: a 14-ASCII-digit UTC timestamp prefix.
- **Behavior**: replace `chrono::Local::now()` with `chrono::Utc::now()` in `generate_unique_id` (`ddb-core/src/parser/mod.rs:960`). The prefix stays digit-only and fixed-width, so prefix-parsing consumers and lexical sort are unaffected.

#### Feature: Fixed-width sub-second component
- **Description**: append a zero-padded sub-second field so more than one ID exists per second.
- **Inputs**: the sub-second part of the same clock read; the last candidate minted by this process.
- **Outputs**: a 6-digit zero-padded sub-second field, e.g. `042317`. **The width is pinned at 6 — decided, not a guess:** 6 is the natural microsecond width of the same clock read, and 10^6 slots per second sits an order of magnitude above the estimated 10^4-10^5 mints/sec/process ceiling, so the overflow wait is unreachable at the estimated rate.
- **Behavior**: the field is a monotonic counter *seeded* from the microsecond component, not a literal clock reading — after a bump it no longer equals the clock. The monotonic guard replaces the sleep: if the computed candidate is not strictly greater than the last candidate minted by this process, increment the sub-second field by 1 and retry; only an overflow past `999999` waits for the next second. `std::thread::sleep` leaves the common path entirely.

#### Feature: Per-node discriminator
- **Description**: append a per-node component so two devices minting in the same instant almost never produce the same ID — 32 bits of separation, not a guarantee (see the residual in the Problem Statement).
- **Inputs**: the node id, read from `.git/ddb-node` the way `HlcClock` reads it (`ddb-core/src/git_ops/hlc_clock.rs:42`-`:46`), truncated to 8 characters the way `truncate_node` does (`ddb-core/src/hlc.rs:123`-`:125`).
- **Outputs**: an 8-character lowercase-hex node field, e.g. `a1b2c3d4`.
- **Behavior**: reuse the existing node-id source and truncation — do not add a second node-identity concept, and do not widen the field (widening would leave the ID and `HlcClock` rendering node identity differently; the 00170 decision took the bounded residual instead). When `.git/ddb-node` is absent, fall back to the same ephemeral non-empty node id `HlcClock` already generates (pinned by `tick_succeeds_with_nonempty_node_when_ddb_node_absent`, `ddb-core/src/git_ops/hlc_clock.rs:412`-`:434`). Full shape: `<14 UTC digits><6 sub-second digits><8 hex node chars>` = 28 characters, e.g. `20260816143012042317a1b2c3d4`.

#### Feature: Validated node field
- **Description**: guarantee the node field is exactly 8 lowercase-hex characters before it can reach an ID.
- **Inputs**: the raw `.git/ddb-node` value (or the ephemeral fallback id).
- **Outputs**: a conforming 8-character lowercase-hex node field, plus a structured warning when the raw value did not conform.
- **Behavior**: **this validation is mandatory, not defensive padding** — `hlc_clock.rs:42`-`:46` accepts *any* non-empty value (nothing constrains `.git/ddb-node` to hex or to a minimum length), and `truncate_node` returns a *shorter* string when given fewer than 8 non-dash characters. A hand-edited, truncated, or corrupted node file therefore makes the mint emit an ID that its own `^[0-9]{20}[0-9a-f]{8}$` validator rejects. Normalize the raw value (strip dashes, lowercase, take 8) and check it against `^[0-9a-f]{8}$`. On a match, use it. On a miss, **do not fail the create and do not panic**: derive the 8-hex field deterministically by hashing the raw node value (same raw value → same field always, on every process and device, which is the only property the discriminator needs) and surface a structured warning naming `.git/ddb-node` on the create path's existing warning channel. A hard error is rejected here: the node file is machine-generated, and bricking every create on a device because one machine-local file is malformed trades a rare cosmetic problem for a total outage.
- **Test requirements**: a value shorter than 8 non-dash characters (`abc`); a non-hex value (`zzzzzzzz`, and a realistic hostname-style string); a dash-only value (`----`, which is non-empty yet normalizes to the empty string); a value with uppercase hex (normalizes, does not warn); and a well-formed UUID (uses truncation, does not warn). Every case asserts the emitted ID passes `DoogatId::is_valid_shape`, and the two conforming cases assert no warning is raised.

#### Feature: Sub-second batch minting
- **Description**: SQL batch `INSERT` mints real sub-second IDs instead of stepping `+1s` per row.
- **Inputs**: a row count and the existence oracle (`ddb-core/src/id_minting.rs:24`-`:35`).
- **Outputs**: `count` distinct IDs, all within the same wall-clock second when the batch is fast.
- **Behavior**: drop the `+1s` stepping in `SqlEngine::unique_ids` (`ddb-core/src/sql_engine/mod.rs:138`-`:147`) and call the new mint per row; ordering within the batch stays strictly increasing via the monotonic guard.

### Capability: Backward compatibility and migration

#### Feature: Relaxed ID shape validation
- **Description**: accept both the legacy and the new shape everywhere the shape is checked.
- **Inputs**: an ID string.
- **Outputs**: valid / invalid.
- **Behavior**: `DoogatId::is_valid_shape` (today "exactly 14 ASCII digits", exercised at `ddb-core/src/id_minting.rs:401`-`:409`) accepts `^[0-9]{14}$` (legacy) **or** `^[0-9]{20}[0-9a-f]{8}$` (new), and nothing else. The two shapes are unambiguous by length. This lands **before** the mint emits longer IDs.

#### Feature: Legacy wikilink resolution
- **Description**: stored `[[<id>]]` links written under the old shape stay resolvable, permanently.
- **Inputs**: existing doogat bodies containing 14-digit wikilink targets.
- **Outputs**: unchanged files; resolution succeeds for both shapes.
- **Behavior**: the new shape is a strict *extension* — it adds a suffix and never replaces or reformats the 14-digit prefix, so a legacy ID remains a valid ID and `[[20260226120000]]` keeps pointing at the same doogat. `extract_wikilinks_from` (`ddb-core/src/parser/mod.rs:518`) and `rewrite_wikilinks` (`:566`-`:586`) already match any non-`]`/`|` run with no length constraint, so **no wikilink rewrite and no content migration is performed**; a link-resolution test pins both shapes.

#### Feature: No-forced-migration upgrade path
- **Description**: an existing repo minting under the old shape upgrades by doing nothing.
- **Inputs**: a repo whose doogats all carry 14-digit IDs.
- **Outputs**: the repo works unchanged; new doogats get the new shape; old and new coexist.
- **Behavior**: old IDs stay valid permanently (they are simply shorter); files keep their `ddb/{id}.md` names; the SQLite index stores IDs as text, so no reindex is *required* — but `ddb reindex` stays idempotent and safe to run. Phase 0 verifies no index column, FFI record, or transport schema imposes a length constraint on the id; any that does is widened in Phase 1. Lexical sort stays coherent: a legacy 14-digit ID sorts immediately before any new ID sharing its prefix.

### Capability: Cross-interface ID-format conformance

#### Feature: Six-interface ID-format conformance suite
- **Description**: one suite proves the new shape survives every public interface unmodified.
- **Inputs**: a doogat created with a new-shape ID.
- **Outputs**: pass/fail per interface.
- **Behavior**: for each of CLI, GraphQL, REST, PgWire, FFI, NoSQL HTTP: create (or read) a doogat and assert the returned id is byte-identical to the minted 28-character value, that the same id round-trips as a lookup key, and that a legacy 14-digit id is still accepted as a lookup key on that interface.

#### Feature: Per-PRD deliverables
- **Description**: the AGENTS.md deliverables for a user-facing change.
- **Inputs**: the shipped format change.
- **Outputs**: smoke + integration scenarios, e2e tests, docs, walkthrough.
- **Behavior**: add a numbered scenario to `tests/smoke.sh` + `tests/smoke.ps1` (create → assert 28-char id shape) and to `tests/integration.sh` + `tests/integration.ps1` (two-node sync → assert no ID collision); author e2e coverage under `tests/e2e/`; update `docs/src/`; record a showboat walkthrough under `dev/local/walkthroughs/` via the showboat CLI (never edited by hand, verified with `dev/bin/safe-showboat-verify`).

## Structural Decomposition

### Repository Structure

```
ddb-core/src/
├── parser/mod.rs              # Maps to: UTC prefix, sub-second component, monotonic guard
├── id_minting.rs              # Maps to: Per-node discriminator (oracle + mint wiring)
├── types/                     # Maps to: Relaxed ID shape validation (DoogatId::is_valid_shape)
├── git_ops/hlc_clock.rs       # Read-only: the node-id source reused by the discriminator
└── sql_engine/mod.rs          # Maps to: Sub-second batch minting (unique_ids)
tests/
├── e2e/                       # Maps to: Six-interface conformance suite (e2e leg)
├── smoke.sh, smoke.ps1        # Maps to: Per-PRD deliverables (CLI scenario)
└── integration.sh, .ps1       # Maps to: Per-PRD deliverables (two-node collision scenario)
docs/src/
├── architecture/design-decisions.md   # Read-only: the decision this PRD implements
├── technical/data-model.md            # Maps to: documented ID shape
└── technical/performance.md           # Maps to: measured throughput replacing the estimate
dev/local/walkthroughs/        # Maps to: Per-PRD deliverables (walkthrough)
```

### Module: parser (ddb-core/src/parser/mod.rs)
- **Maps to capability**: Collision-resistant ID minting
- **Responsibility**: own the *pure* mint — UTC prefix, fixed-width sub-second field, monotonic guard, no sleep on the common path. `parser/` must gain **no filesystem or repository dependency**: it never reads `.git/ddb-node`.
- **Exports**: `generate_unique_id(exists, node)` — **the signature changes**, and this PRD says so deliberately. The mint needs a repo-derived node discriminator, but AGENTS.md's clean-architecture guardrail keeps I/O at the edges and `parser/` is a text layer, so the discriminator is passed *in* as an already-validated `&str` rather than read inside the parser. Every call site updates with it: `DoogatService::unique_id` (`ddb-core/src/service/write_helpers.rs:244`-`:247`) and `SqlEngine::unique_ids` (`ddb-core/src/sql_engine/mod.rs:126`-`:150`).

### Module: id_minting (ddb-core/src/id_minting.rs)
- **Maps to capability**: Collision-resistant ID minting
- **Responsibility**: be the I/O edge for identity — resolve, validate, and normalize the per-node discriminator from the existing `.git/ddb-node` source, then hand it to the pure mint; keep the repo-aware existence oracle unchanged. This module already reads the repo HEAD tree and the SQLite index to build the oracle, so the node-file read belongs here and nowhere further in.
- **Exports**: `existence_oracle` (unchanged), `derive_content_id` (unchanged — the merge-time backstop, now covering the residual node-field collision as well as legacy collisions), plus the node-discriminator resolver that owns the `Validated node field` behavior and returns the 8-hex value the mint consumes.

### Module: types (ddb-core/src/types/ — `DoogatId`)
- **Maps to capability**: Backward compatibility and migration
- **Responsibility**: own the one shape rule that accepts legacy and new IDs.
- **Exports**: `DoogatId::is_valid_shape(&str) -> bool`

### Module: sql_engine (ddb-core/src/sql_engine/mod.rs)
- **Maps to capability**: Collision-resistant ID minting
- **Responsibility**: mint one real ID per batch row instead of stepping the timestamp.
- **Exports**: `SqlEngine::unique_ids(count)` (same signature, sub-second values)

### Module: tests + docs (tests/, docs/src/, dev/local/walkthroughs/)
- **Maps to capability**: Cross-interface ID-format conformance
- **Responsibility**: prove the shape across all six interfaces and document it.
- **Exports**: the conformance suite, smoke/integration scenarios, e2e tests, docs updates, walkthrough.

## Dependency Graph

### Foundation Layer (Phase 0)
No dependencies - built first.

- **`DoogatId::is_valid_shape` relaxation**: must accept the new shape *before* anything mints it, or the first new ID fails its own validator.
- **Length-constraint audit**: the per-transport and index-schema survey that tells Phase 1 what else needs widening.

### Core Layer (Phase 1)
- **parser mint (UTC + sub-second + node)**: Depends on [`is_valid_shape` relaxation, length-constraint audit]
- **id_minting node discriminator**: Depends on [length-constraint audit]
- **sql_engine batch mint**: Depends on [parser mint]
- **transport length/shape fixes**: Depends on [length-constraint audit]

### Integration Layer (Phase 2)
- **conformance suite + per-PRD deliverables + docs**: Depends on [parser mint, sql_engine batch mint, transport length/shape fixes]

## Implementation Phases

### Phase 0: Foundation
**Goal**: make the new shape legal and know exactly where it is constrained.

**Tasks**:
- [ ] Relax `DoogatId::is_valid_shape` to accept `^[0-9]{14}$` (legacy) or `^[0-9]{20}[0-9a-f]{8}$` (new), rejecting everything else (no deps) - Acceptance: unit tests pin both shapes accepted, and a 13-digit, a 15-digit, and a 28-char non-hex value rejected; `cargo test-ci` green.
- [ ] Audit every ID length/shape constraint outside the core validator: index column definitions, FFI record types, GraphQL/REST/NoSQL schema declarations, PgWire column typing, and any exact-14 assertion in tests or fixtures; record findings in the task report (no deps) - Acceptance: a written list naming each constrained site (file:line) or stating explicitly that an interface passes the ID through opaquely.

**Exit Criteria**: the new shape validates; every remaining constraint on ID length is enumerated.

### Phase 1: Core
**Goal**: mint the new shape, with a validated per-node discriminator and without a wall-clock sleep.

**Tasks**:
- [ ] Mint the prefix from `chrono::Utc::now()` and append the 6-digit zero-padded microseconds-within-second field in `generate_unique_id`; replace the 100ms sleep with the monotonic guard (bump the sub-second field; wait only on overflow past `999999`) (depends on: Phase 0) - Acceptance: a unit test creating 1000 IDs in one process gets 1000 distinct, strictly increasing values in under one second, with no `thread::sleep` on that path.
- [ ] Add the node-discriminator resolver to `id_minting`: read `.git/ddb-node` via the existing `HlcClock` node-id source (ephemeral fallback when the file is absent), then normalize and validate to `^[0-9a-f]{8}$` with the deterministic hash fallback and structured warning from the `Validated node field` feature (depends on: Phase 0) - Acceptance: unit tests cover a short value, a non-hex value, a dash-only value, an uppercase-hex value, a well-formed UUID, and an absent file; every case yields a field passing `^[0-9a-f]{8}$`, the two conforming cases raise no warning, and the same raw value always maps to the same field.
- [ ] Thread the discriminator into the pure mint: change `generate_unique_id(exists)` to `generate_unique_id(exists, node)` and update both call sites (`DoogatService::unique_id`, `SqlEngine::unique_ids`); `parser/` gains no filesystem or repo dependency (depends on: node-discriminator resolver) - Acceptance: a unit test with two node ids differing within their first 8 non-dash characters, minting in the same instant, yields disjoint IDs; a second test with two UUIDs *sharing* those 8 characters asserts identical node fields, pinning the documented residual; `parser/mod.rs` contains no `.git/ddb-node` read and no `std::fs` call added by this task.
- [ ] Replace the `+1s` stepping in `SqlEngine::unique_ids` with a real per-row mint (depends on: parser mint) - Acceptance: a 10-row batch `INSERT` yields 10 distinct IDs sharing the same 14-digit second prefix.
- [ ] Widen or remove every length/shape constraint the Phase 0 audit found (depends on: Phase 0) - Acceptance: each site from the audit list is either fixed or documented as needing no change; `cargo clippy --workspace --all-targets` clean, `cargo test-ci` green.

**Exit Criteria**: new doogats carry 28-character IDs, two nodes with different truncated node fields cannot collide, the residual shared-prefix case is asserted and backstopped, and the mint no longer sleeps.

### Phase 2: Integration
**Goal**: prove the shape across all six interfaces and ship the deliverables.

**Tasks**:
- [ ] Author the six-interface ID-format conformance suite: per interface, assert a new-shape id round-trips byte-identically and a legacy 14-digit id is still accepted as a lookup key (depends on: Phase 1) - Acceptance: the suite covers CLI, GraphQL, REST, PgWire, FFI, and NoSQL HTTP with one test each and all pass.
- [ ] Author the backward-compatibility regression: a repo seeded with legacy 14-digit doogats and stored `[[<14-digit-id>]]` wikilinks resolves every link and rewrites no file after the upgrade (depends on: Phase 1) - Acceptance: the test passes and asserts zero content modifications.
- [ ] Add the smoke scenario (`tests/smoke.sh` + `tests/smoke.ps1`: create → assert the 28-character shape) and the integration scenario (`tests/integration.sh` + `tests/integration.ps1`: two nodes create in the same second → sync → assert two distinct doogats, no collision repair) plus e2e coverage under `tests/e2e/` (depends on: Phase 1) - Acceptance: all four scripts carry the new numbered section with the `pass` helper; the e2e test is **authored locally and executed by CI** (AGENTS.md Tier 2 — e2e execution is delegated, not a local per-task gate), so the local gate is `cargo test-ci` plus `cargo clippy --workspace --all-targets` clean.
- [ ] Benchmark the new mint and record the measured **ID mints/sec/process** in `docs/src/technical/performance.md`, replacing the unbenchmarked 10^4-10^5 estimate (depends on: Phase 1) - Acceptance: the doc states a measured mint rate produced by a committed benchmark, labelled as an ID-mint rate and explicitly distinguished from the ~1s per-doogat create latency already recorded at `performance.md:87`, `:95`.
- [ ] Update `docs/src/technical/data-model.md` (ID shape, both accepted shapes, UTC), `docs/src/architecture/design-decisions.md` (mark the entry shipped), `AGENTS.md`'s "DoogatId: 14-digit timestamp string" convention line, the GraphQL schema descriptions that hardcode "14-digit timestamp ID" (`ddb-server/src/schema/queries.rs:68`, `:70` — this wording propagates into downstream generated clients, e.g. jink's `frontend/src/lib/graphql/generated.ts:642`), and add a `CHANGELOG.md` `Changed` entry; record the showboat walkthrough under `dev/local/walkthroughs/` via the showboat CLI and verify it with `dev/bin/safe-showboat-verify`; push `master` after the commits land (depends on: Phase 1) - Acceptance: each named file reflects the new shape; the walkthrough file exists and verifies clean.

**Exit Criteria**: all six interfaces conform, legacy IDs and wikilinks still resolve, deliverables landed.

## Test Strategy

### Critical Scenarios
- **Happy path**: mint 1000 IDs in one process → Expected: 1000 distinct, strictly increasing 28-character IDs in under one second, all sharing one node field. (This exercises the mint only. It is *not* a 1000-doogat create: each create still costs its own git commit, ~1s.)
- **Happy path**: two nodes whose `.git/ddb-node` UUIDs differ within their first 8 non-dash characters create in the same wall-clock second, then sync → Expected: two distinct doogats, no collision, no `derive_content_id` reassignment.
- **Residual case**: two nodes whose distinct UUIDs *share* their first 8 non-dash characters → Expected: identical node fields (this is the documented 2^-32 residual, asserted so it stays visible), and when the sub-second slot also coincides, the resulting duplicate ID is resolved by `resolve_add_add_collision` + `derive_content_id` exactly as a legacy collision is.
- **Edge case**: a repo full of legacy 14-digit doogats with stored `[[<id>]]` wikilinks is opened after the upgrade → Expected: every link resolves, no file is rewritten, no reindex is required.
- **Edge case**: `.git/ddb-node` is absent → Expected: the mint still produces a non-empty 8-character node field via the ephemeral fallback, and the ID validates.
- **Edge case**: `.git/ddb-node` holds a short (`abc`), non-hex (`zzzzzzzz`), or dash-only (`----`) value → Expected: the deterministic hash fallback yields a conforming 8-hex field, the ID passes `DoogatId::is_valid_shape`, a structured warning names the file, and the create succeeds rather than erroring.
- **Edge case**: the sub-second field overflows within one second → Expected: the mint waits for the next second rather than emitting a duplicate or a malformed ID.
- **Error case**: a 15-digit or 28-character non-hex ID reaches `DoogatId::is_valid_shape` → Expected: rejected, with the caller surfacing a structured error rather than persisting it.

## Risks

- **Sort discontinuity at the switchover**: on a machine east of UTC, the first UTC-minted ID can be numerically lower than the last local-time ID, so a strict "IDs increase over time" assumption breaks once, at upgrade. Mitigation: state it in `data-model.md`; nothing in the codebase may depend on cross-mint monotonicity for correctness — the Phase 0 audit checks for such an assumption and Phase 1 removes any found.
- **Downstream exact-length validators**: any consumer that whole-string-matches 14 digits breaks. Verified status: **jink does not** — it treats the id as an opaque GraphQL `ID` scalar (`frontend/src/lib/graphql/generated.ts:740`, `:1036`) with no slicing, length check, or date-from-id parsing; its only `14-digit` reference is a generated doc comment (`:642`) sourced from ddb's own schema wording. The exposure is therefore ddb's own validators and wording plus unknown future downstreams, not a known break. Mitigation: the shape is a strict extension so prefix parsers keep working; the CHANGELOG entry and `data-model.md` call out the exact-length case; the conformance suite pins both shapes as accepted lookup keys.
- **Residual node-field collision**: `truncate_node` keeps 32 bits, so two nodes can share a node field (2^-32 per pair; ~50% somewhere at ~77,000 nodes) and, if they also share a sub-second slot, mint the same ID. Mitigation: none at mint by design — the 00170 decision accepted the bounded residual to keep one node-identity concept. `resolve_add_add_collision` + `derive_content_id` stay wired as the backstop and get a regression test; the residual is documented in `design-decisions.md` and asserted by the shared-prefix unit test so it cannot quietly become an assumed guarantee.
- **Node discriminator drift**: inventing a second node-identity concept beside `HlcClock`'s would let the two disagree. Mitigation: reuse the existing `.git/ddb-node` source and `truncate_node` width; do not add a parallel node file.
- **Batch-mint spacing change**: `SqlEngine::unique_ids` currently guarantees second-spaced IDs; some consumer may rely on that spacing. Mitigation: the Phase 0 audit checks for a spacing assumption; ordering within a batch stays strictly increasing either way.
- **Throughput estimate is unbenchmarked, and easy to misread**: the 10^4-10^5 figure is an estimate of **ID mints/sec/process**, not of doogat creates, and it is not a measurement. Mitigation: Phase 2 benchmarks the mint and publishes the measured mint rate; every statement of it carries the "ID mints" unit alongside the unchanged ~1s per-doogat create cost, so the estimate never ships as fact and never ships as a create-throughput promise.
- **Longer IDs are less legible**: a 28-character wikilink reads worse than a 14-digit date. Mitigation: the date prefix is unchanged and still leads the string; no mitigation beyond documenting it — this cost was accepted by the 00170 decision.

## Transport Impact

The ID is a contract-level artifact: every interface that returns or accepts a doogat ID depends on its format, so this change is cross-cutting and ships everywhere at once. The expected posture below is *opaque pass-through* for all six — the Phase 0 audit confirms it per interface and any site that carries its own length/shape validation is fixed in Phase 1.

- **CLI**: Guaranteed — surfaces IDs in create output, wikilinks, and lookup arguments; expected opaque pass-through, plus the smoke scenario asserting the new shape.
- **GraphQL**: Guaranteed — `id` fields and id arguments are string-typed; expected opaque pass-through; audit checks the dynamic schema for any length/pattern constraint.
- **REST**: Guaranteed — `id` in JSON payloads and path segments; expected opaque pass-through; audit checks path-segment validation for a digit-count or length rule.
- **PgWire**: Guaranteed — the `id` column is text; expected opaque pass-through; audit checks the declared column type/width for a fixed-length declaration.
- **FFI**: Guaranteed — `DoogatId` crosses UniFFI as a string in `DoogatDriver` records; expected opaque pass-through; audit checks the generated record types for a length assertion.
- **NoSQL HTTP**: Guaranteed — the document `id`; expected opaque pass-through; audit checks the document-mirror key handling.
- **Cross-interface conformance plan**: the Phase 2 conformance suite owns this — per interface, a new-shape id must round-trip byte-identically and a legacy 14-digit id must still resolve as a lookup key. The wikilink-migration plan is "no migration": the new shape strictly extends the old one, stored `[[<id>]]` links are never rewritten, and legacy IDs stay valid permanently; the regression test asserts zero content modifications. jink impact: this lifts the **ID-mint** ceiling from ~1 mint/sec/process to an estimated 10^4-10^5 mints/sec/process (unbenchmarked — Phase 2 replaces the estimate with a measurement), and makes the cross-device ID collision that jink's multi-device sync can hit today residual (2^-32 per node pair) instead of routine. It does **not** lift jink's bulk-create ceiling: creating a doogat still costs a git commit (~1s each, `docs/src/technical/performance.md:87`, `:95`), so end-to-end creates stay at roughly 1/sec until a separate write-path change amortizes commits. jink needs no client change — its id handling is already opaque (`frontend/src/lib/graphql/generated.ts:740`, `:1036`).
