# Deterministic CRDT conflict resolution via stable actor IDs

**Priority:** P1 · **Roadmap:** Phase 1 (convergence) · **Depends on:** none (complementary to 00166: 00165 fixes tie-break determinism at the Automerge layer; 00166 fixes cross-commit HLC ordering that decides which side should win)

> **Scope addition (2026-07-16 assessment):** two resolver-adjacent correctness items ride along. (1) `map_to_yaml` (`crdt_resolver.rs:312-342`) writes merge-resolution frontmatter with weaker quoting than the parser's `yaml_quote` — list items interpolated into `- "{item}"` with no escaping, needs-quotes scalars emitted raw — so resolution output can round-trip into malformed frontmatter (a self-inflicted feeder of the 00169 poison-file class; 00169 only hardens the reader). Fix: expose the parser's quoting helpers (`parser/mod.rs:618-631`) `pub(crate)` and reuse them. (2) Decide the `.crdt/temp` lifecycle: `GitRepo::open` wipes it (`git_ops/mod.rs:174,577-595`) while sync persists `_fm.crdt` continuity state there (`sync_manager/mod.rs:72-90`) and compaction maintains it; no production reader exists, and `compacted_*` names fail `parse_crdt_temp_name`. Either delete the machinery or stop wiping on open and add the reader — document the choice. NOTE: this is the SAME decision the existing Phase 2 task already carries ("verify whether the persisted CRDT state is ever read back... record the keep-or-stop-writing decision") — fold this evidence into that task, do not plan it twice. Evidence: evolution-assessment-2026-07-16.md F10/F11.

## Overview

### Problem Statement
CRDT conflict resolution is non-deterministic because Automerge breaks concurrent-change ties by **random actor ID**, and the resolver never sets one:

- `crdt_resolver.rs` contains **zero** `set_actor` / `ActorId` / `with_actor` calls (confirmed by grep over the file; the same grep finds the three `AutoCommit::new()` sites, so the pattern is not a false negative).
- `merge_frontmatter` (`ddb-core/src/crdt_resolver.rs:107-151`) builds a doc via `AutoCommit::new()` (`:118`) then `fork()`s it into `doc_ours` (`:124`) and `doc_theirs` (`:136`). `fork()` mints a fresh **random** actor for each child, so two concurrent same-key scalar sets (e.g. `status`, `priority`, a typed column) are tie-broken by chance.
- `merge_body` (`crdt_resolver.rs:375-397`) has the same shape: `AutoCommit::new()` (`:376`) + `fork()` (`:381`, `:389`) + `splice_text`; concurrent same-position text inserts interleave by random actor order. A third resolver doc-build site sits at `crdt_resolver.rs:483`.
- The LWW fallback `pick_lww_winner` (`crdt_resolver.rs:578-589`) only decides *whole-file* winners when both HLCs are present; the *within-doc* CRDT merge above is where the randomness bites.

**Failure scenario (routine, not rare)**: the loser of a push race has already committed its own resolution before it sees the winner, so two nodes independently resolve the same `(ours, theirs)` conflict. With random actors they can pick **different** winners for a conflicting frontmatter scalar or interleave body text differently, forcing extra merge rounds and a final value that differs from what either node showed the user.

### Target Users
Multi-device users (jink) whose devices resolve the same conflict independently and must converge on one consistent value instead of a coin flip.

### Success Metrics
- Resolving the same `(ours, theirs)` pair twice yields **byte-identical** output (resolved text + CRDT state) — pinned by a regression test.
- Two simulated nodes resolving the same conflict independently (one seeing `(ours=X, theirs=Y)`, the other the swapped `(ours=Y, theirs=X)`) converge to the **same** winning value — pinned by a convergence test.
- A concurrent same-key frontmatter scalar conflict resolves to the same deterministic winner on both nodes; concurrent same-position body inserts interleave identically.
- No regression: existing resolver tests stay green; the change is limited to actor assignment.

## Functional Decomposition

### Capability: Deterministic conflict resolution

#### Feature: Content-derived stable actor IDs
- **Description**: every Automerge doc the resolver builds gets a deterministic actor ID derived from the merged **content**, symmetric under swapping `ours`/`theirs`.
- **Inputs**: the `ancestor`/`ours`/`theirs` content strings already passed to `merge_frontmatter`/`merge_body`.
- **Outputs**: a valid `ActorId` per doc, fixed for given content.
- **Behavior**: derive each side's actor from a hash of that side's content (equivalently its blob OID); assign via `set_actor` on the base doc and on each forked child before applying diffs. Deriving from content (not the local "ours"/"theirs" role) is required so a peer whose roles are swapped assigns the **same** actor to the **same** physical content and Automerge's actor-ordered tie-break selects the same winner.

#### Feature: Pure-function resolution boundary
- **Description**: `merge_frontmatter`/`merge_body`/the third doc-build path become pure functions of `(ancestor, ours, theirs)`.
- **Inputs**: the three content zones.
- **Outputs**: deterministic resolved content + CRDT bytes.
- **Behavior**: no `chrono::now`, no random actor, no process-local state enters the merge; identical inputs always produce identical bytes.

## Structural Decomposition

### Repository Structure

```
ddb-core/src/
├── crdt_resolver.rs   # Maps to: Content-derived stable actor IDs (set_actor at :118 base + :124/:136 forks; merge_body :376/:381/:389; third build :483)
ddb-core/tests/
└── crdt_determinism.rs # Maps to: Pure-function resolution boundary (determinism + two-node convergence)
```

### Module: crdt_resolver (ddb-core/src/crdt_resolver.rs)
- **Maps to capability**: Deterministic conflict resolution
- **Responsibility**: assign stable, content-derived Automerge actor IDs at every doc construction so merge output is a pure function of `(ancestor, ours, theirs)`.
- **Exports**:
  - `merge_frontmatter(...)` / `merge_body(...)` - now `set_actor` deterministically on base + forks
  - a private `derive_actor(content: &str) -> ActorId` helper - symmetric, content-addressed

### Module: crdt_determinism test (ddb-core/tests/crdt_determinism.rs)
- **Maps to capability**: Deterministic conflict resolution
- **Responsibility**: assert byte-identical re-resolution and two-node convergence.
- **Exports**: the determinism + swap-convergence tests

## Dependency Graph

### Foundation Layer (Phase 0)
No dependencies - built first.

- **derive_actor helper**: content-hash → valid `ActorId`, symmetric under swap.

### Core Layer (Phase 1)
- **actor assignment in the resolver**: Depends on [derive_actor] - apply `set_actor` at every doc-build site.

### Integration Layer (Phase 2)
- **determinism + convergence tests + docs**: Depends on [actor assignment].

## Implementation Phases

### Phase 0: Foundation
**Goal**: a deterministic, symmetric actor derivation.

**Tasks**:
- [ ] Add `derive_actor(content) -> ActorId` (hash content → fixed-length actor bytes) (no deps) - Acceptance: unit test proves same content → same actor, different content → different actor, and the actor is a valid Automerge `ActorId`.

**Exit Criteria**: actor derivation is deterministic and valid.

### Phase 1: Core
**Goal**: remove all randomness from resolution.

**Tasks**:
- [ ] Call `set_actor` with the derived actor on the base doc and each fork in `merge_frontmatter`, `merge_body`, and the `:483` build site, before diffs are applied (depends on: Phase 0) - Acceptance: grep shows `set_actor` at every doc-build site; a test resolving `(ours, theirs)` twice returns byte-identical output.

**Exit Criteria**: resolution is a pure function of its content inputs.

### Phase 2: Integration
**Goal**: prove convergence and document it.

**Tasks**:
- [ ] Add a property/integration determinism test (re-resolve → identical) and a two-node convergence test (swapped roles → same winner) in `ddb-core/tests/crdt_determinism.rs`; add a CRDT-conflict convergence scenario to `integration.sh`/`.ps1` (depends on: Phase 1) - Acceptance: both fail against the old random-actor code and pass after.
- [ ] Update `docs/src/technical/crdt-resolver.md` (deterministic actor assignment; determinism is 00165, HLC winner-ordering is 00166) + CHANGELOG; while in the resolver, verify whether the persisted CRDT state (`fm_crdt_bytes` / `.crdt` files) is ever read back anywhere, and record the keep-or-stop-writing decision in `docs/src/architecture/design-decisions.md` (closes the purpose-fit critique §8.3 leftover) (depends on: Phase 1) - Acceptance: docs describe the pure-function boundary; the .crdt read-back finding and decision are recorded. Internal resolution change with no new CLI command or user-visible surface, so no walkthrough is required.

**Exit Criteria**: convergence tests + docs landed.

## Test Strategy

### Critical Scenarios
- **Happy path**: resolve one `(ours, theirs)` pair twice → Expected: identical resolved content and CRDT bytes.
- **Edge case**: node A resolves `(ours=X, theirs=Y)`, node B resolves `(ours=Y, theirs=X)` → Expected: both pick the same physical winner (role-swap symmetric).
- **Error case**: concurrent same-key scalar (`status: A` vs `status: B`) and same-position body inserts → Expected: identical deterministic outcome on both nodes, no extra merge round that flips the value.

## Risks

- **Role-swap divergence**: if the actor derives from the local "ours"/"theirs" role or the local node UUID rather than content, two peers assign opposite actors and diverge. Mitigation: derive strictly from content/blob OID; the swapped-role test is the guard.
- **Stability is not intent**: 00165 guarantees the *same* winner every time, not that the *higher-HLC* side wins. Mitigation: 00166 layers HLC ordering on top; document the split so neither PRD is mistaken for the other.
- **CRDT state bytes shift**: `fm_crdt_bytes` produced after the fix differ from pre-fix bytes. Mitigation: CRDT state is derived/rebuildable (reindex regenerates it); no data migration; note in CHANGELOG.
- **ActorId validity**: Automerge requires a well-formed actor. Mitigation: truncate/pad hash bytes to a valid length; unit-test construction.

## Transport Impact

- **CLI**: internal — conflict resolution is not a per-transport surface; `ddb sync` merge output becomes deterministic, no public change.
- **GraphQL**: internal — sync/merge mutations return the same shape; only the resolved value becomes stable.
- **REST**: internal — no public surface change.
- **PgWire**: internal — no public surface change.
- **FFI**: internal — sync via the driver converges deterministically; no signature change.
- **NoSQL HTTP**: internal — read-only mirror reflects the converged value; no surface change.
- **Cross-interface conformance plan**: a core property test asserts re-resolution determinism and a two-node convergence test asserts swap-symmetry; an `integration.sh`/`.ps1` CRDT-conflict scenario pins end-to-end convergence. No new transport surface, no response/error shape change.
