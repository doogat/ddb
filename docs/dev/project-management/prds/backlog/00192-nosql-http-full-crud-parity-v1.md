---
catchup: force
default_model: opus
model_tier_rationale: cross-cutting shape change - three new NoSQL verbs on the contract plus a read-store guarantee
---

# NoSQL HTTP full CRUD parity

**Priority:** P2 · **Roadmap:** Phase 3 (parity gaps; MUST land after the app-contract block 00172-00178) · **Depends on:** 00172-00178 (app contract complete — this PRD is also its acceptance test), 00173 (unified error policy) · **Source:** maintainer decision 2026-07-13 ("no specialization exceptions; attack the six-implementations problem at the root"), `dev/local/audit-results/2026-07-12-purpose-fit-critique.md` §5

## Overview

### Problem Statement
The NoSQL HTTP API is read-only — `GET /{id}`, `GET /?type=|?tag=`, `GET /{id}/backlinks` (`ddb-server/src/nosql_api.rs:27-32`) — which violates the AGENTS.md baseline that CRUD is the minimum Guaranteed capability for every public application interface. The maintainer explicitly declined the specialization exception (2026-07-13): the interface stays public and Guaranteed, so the missing verbs are a parity gap to close.

The sequencing is the point. Closing this gap BEFORE the app contract lands would hand-plumb a seventh write path — the exact disease 00172-00178 cure. Closing it AFTER doubles as the acceptance test for that cure: if each NoSQL verb is more than a thin adapter over `AppCommand → DoogatService → AppOutput`, the contract is still half-built and that finding goes back into the contract work.

A second inconsistency rides along: NoSQL reads are served from the redb mirror, which is best-effort by contract (`NoSqlMirrorPort`, `ddb-core/src/traits.rs:352-358`; dual-written on CRUD with `let _ =` at `service/write_helpers.rs:253` and `service/delete.rs:171`; REBUILT from scratch on every NoSQL read by `open_nosql_fresh`, `service/utility.rs:111-115`, `nosql.rs:224-247`), while every other transport reads the SQLite index. A Guaranteed interface backed by a best-effort store is parity drift in disguise. DECIDED (2026-09-05 backlog review): KEEP the redb mirror as the NoSQL read store and give it a defined, tested guarantee (rejected alternative: serve NoSQL reads from the SQLite index and retire the mirror's serving role); this PRD records that decision and implements the guarantee.

### Target Users
Downstream apps choosing the document-shaped HTTP interface (the positioning's "each downstream picks its access shape") that today silently cannot write through it; ddb maintainers who need a concrete measure of whether the app contract actually made transports cheap.

### Success Metrics
- `POST /nosql`, `PUT /nosql/{id}`, `DELETE /nosql/{id}` exist, route through the app contract, and pass the same golden CRUD workflow the other interfaces pass.
- Adapter thinness measured and recorded: each new verb's transport-layer code is a thin mapping (target: one screenful per verb, no business logic, no re-decided errors/warnings); a written note records the actual line counts as contract feedback.
- Error and warning shapes derive from the unified policy (00173) — byte-identical codes/redaction with REST for the shared error set.
- The read-store decision (keep the redb mirror, rejected alternative: the core index — decided 2026-09-05) is recorded in `design-decisions.md` with rationale, and NoSQL's freshness/consistency guarantee (write-then-read exact; the mirror write on the NoSQL write path is no longer best-effort) appears on the consistency contract page (00190).
- The CRUD conformance suite includes the NoSQL leg alongside CLI/GraphQL/REST.

## Functional Decomposition

### Capability: NoSQL write verbs
#### Feature: Create (POST /nosql)
- **Description**: create a doogat from a JSON document.
- **Inputs**: JSON body (type, fields, body content) matching the document shape the GET routes already serve.
- **Outputs**: 201 with the created document (canonical core result type serialized), warnings included per AppOutput.
- **Behavior**: map JSON → the same `AppCommand` create the REST/GraphQL adapters use; no NoSQL-specific business logic; ID minting, locking, indexing all happen in core.

#### Feature: Update (PUT /nosql/{id})
- **Description**: update fields/body of an existing document.
- **Inputs**: id path param; JSON body of changed fields.
- **Outputs**: 200 with the updated document + AppOutput warnings; 404 via the unified policy when absent.
- **Behavior**: thin map to the contract update command; partial-update semantics identical to REST's update (same core command, same result).

#### Feature: Delete (DELETE /nosql/{id})
- **Description**: delete a document.
- **Inputs**: id path param.
- **Outputs**: HTTP 200 with the canonical DeleteResult/warnings envelope on success; 404 through the unified policy when absent. Broken-backlink warnings are preserved.
- **Behavior**: thin map to the contract delete command (00175's routed delete).

### Capability: Read-store coherence
#### Feature: Decide and align the NoSQL read store
- **Description**: the decision is made (keep the redb mirror as the NoSQL read store; 2026-09-05 backlog review); this feature records it and defines the guarantee: a NoSQL write is visible to the next NoSQL read (write-then-read exact), the mirror write on the NoSQL write path is no longer best-effort (a failed mirror write fails the request), and cross-process freshness keeps riding the per-call rebuild (`open_nosql_fresh`) unless design shows an incremental mirror is cheap.
- **Inputs**: mirror behavior today (best-effort dual-write, per-call rebuild), the 00190 consistency contract.
- **Outputs**: a `design-decisions.md` entry (decision, rationale, rejected alternative: serving from the SQLite index); the implementation aligned to it; the consistency page updated.
- **Behavior**: Phase 0 task records the decision + guarantee. Premise: the redb mirror currently serves only the three GET routes and is best-effort — re-check this at execution time; if other consumers have appeared, the guarantee must cover them (skip-and-report on a failed premise, never force).

### Capability: Cross-interface CRUD conformance
#### Feature: NoSQL leg of the golden CRUD workflow
- **Description**: the same create→read→update→delete workflow the other interfaces pass, executed against the NoSQL API.
- **Inputs**: the golden workflow definition used by existing conformance/parity tests.
- **Outputs**: a passing NoSQL conformance test; parity failures block, not warn.
- **Behavior**: drive the four verbs over HTTP; assert result fields against the canonical core result type; assert error/warning parity with REST for the shared set.

## Structural Decomposition

### Repository Structure

```
ddb-server/src/
├── nosql_api.rs        # Maps to: NoSQL write verbs (new POST/PUT/DELETE routes, thin adapters)
├── http_error.rs       # Maps to: NoSQL write verbs (unified policy rendering, already shared)
└── lib.rs              # Maps to: route registration under the auth middleware
ddb-core/src/
├── nosql.rs            # Maps to: Read-store coherence (mirror serving role per the decision)
└── traits.rs           # Maps to: Read-store coherence (NoSqlMirrorPort contract per the decision)
ddb-server/tests/
└── nosql_crud_*.rs     # Maps to: Cross-interface CRUD conformance (NoSQL leg)
docs/src/
├── architecture/design-decisions.md  # Maps to: Read-store coherence (recorded decision)
├── technical/server.md               # Maps to: NoSQL API docs (verbs, shapes, consistency)
├── technical/nosql.md                # Maps to: NoSQL index doc (API + Server Integration sections gain the write verbs)
└── technical/conformance-harness.md  # Maps to: Cross-interface CRUD conformance (module table + deferred-scope row for the NoSQL driver)
```

### Module: nosql_api (ddb-server/src/nosql_api.rs)
- **Maps to capability**: NoSQL write verbs
- **Responsibility**: HTTP encoding only — JSON ↔ AppCommand/AppOutput; zero business logic, zero re-decided errors.
- **Exports**: the router with GET (existing) + POST/PUT/DELETE (new).

### Module: nosql + traits (ddb-core/src/nosql.rs, traits.rs)
- **Maps to capability**: Read-store coherence
- **Responsibility**: implement the defined redb guarantee (mirror kept as the read store; the NoSQL write path's mirror write becomes must-succeed).
- **Exports**: `RedbIndex`/`NoSqlMirrorPort` as decided.

### Module: conformance tests (ddb-server/tests/)
- **Maps to capability**: Cross-interface CRUD conformance
- **Responsibility**: pin the NoSQL leg of the golden workflow and error/warning parity.
- **Exports**: `nosql_crud_*` test suite.

### Module: capability publication (ddb-server/src/capabilities.rs)
- **Maps to capability**: NoSQL write verbs
- **Responsibility**: publish nosql_http_crud from the existing 00186 registry and verify its declared interface/build scope.
- **Exports**: the registry entry and matching handshake fixtures.

## Dependency Graph

### Foundation Layer (Phase 0)
No dependencies within this PRD (external: 00172-00178 + 00173 landed).

- **Read-store decision record**: the guarantee the write verbs must keep (redb mirror kept, decided 2026-09-05).

### Core Layer (Phase 1)
- **write verbs (POST/PUT/DELETE)**: Depends on [read-store decision] — writes must land in whatever store serves reads.

### Integration Layer (Phase 2)
- **conformance suite + docs + thinness note**: Depends on [write verbs].

## Implementation Phases

### Phase 0: Foundation
**Goal**: the read-store question is decided, not inherited.

**Tasks**:
- [ ] Record the read-store decision (keep the redb mirror; rejected alternative: the SQLite index) and its guarantee (write-then-read exact; mirror write no longer best-effort on the NoSQL write path) in `design-decisions.md`; re-check the premise that the mirror serves only the three GET routes (no deps) - Acceptance: decision entry exists with rationale, rejected alternative, and the stated guarantee; premise re-checked at execution time.

**Exit Criteria**: recorded decision; Phase 1 knows its target store.

### Phase 1: Core
**Goal**: the three write verbs, thin.

**Tasks**:
- [ ] Add POST/PUT/DELETE routes mapping JSON ↔ the contract commands/outputs, registered under the existing auth middleware; errors/warnings via the unified policy (depends on: Phase 0) - Acceptance: each verb works end to end; transport-layer diff per verb is a thin adapter (no business logic); error codes byte-match REST for the shared set; DELETE returns 200 with all broken-backlink warnings, and a clean delete carries an empty warnings array.
- [ ] Align the read path to the Phase 0 decision (retain redb and implement its defined write-then-read guarantee) so a NoSQL write is immediately visible to a NoSQL read (depends on: Phase 0) - Acceptance: write-then-read over NoSQL returns the write; the consistency guarantee is stated and tested; if git commits but mirror refresh fails, return a structured partial-progress failure identifying the committed write, preserve authoritative data, and make retry/read recovery explicit rather than implying the write never happened.

**Exit Criteria**: full CRUD over NoSQL HTTP, coherent with its read store.

### Phase 2: Integration
**Goal**: parity pinned, thinness measured, docs landed.

**Tasks**:
- [ ] Add the NoSQL leg to the CRUD conformance suite (golden workflow + error/warning parity with REST). This includes building the harness's NoSQL HTTP driver (`tests/e2e/conformance/`; the REST driver exists from 00175) (depends on: Phase 1) - Acceptance: conformance green; parity asserted, not eyeballed; the NoSQL create honors `on_conflict` and returns the `existed` indicator identically to REST (the 00187 capability rides the shared `CreateCommand` — pinned, not assumed).
- [ ] Record the adapter thinness measurement (lines per verb, any business logic found) as contract feedback; if any verb exceeded a thin adapter, file the finding against the contract work (depends on: Phase 1) - Acceptance: a written note with actual numbers exists; overruns filed as issues/PRD notes, not swallowed.
- [ ] Author the scenarios in the Rust e2e harness (`tests/e2e/`; the shell harness was removed by 00171) covering NoSQL CRUD over HTTP, a showboat walkthrough for the NoSQL API, docs updates (`server.md` + consistency page NoSQL row), and a CHANGELOG entry (depends on: Phase 1) - Acceptance: e2e scenarios + walkthrough + docs + CHANGELOG landed; push `master`. Include the `nosql_http_crud` registry addition and update exact-set plus health/GraphQL agreement fixtures; Acceptance also requires the flag to match compiled availability and the implemented workflow.

**Exit Criteria**: conformance + measurement + scenarios + docs landed.

## Test Strategy

**Validation:** per-task local gates are `cargo build`, `cargo clippy --workspace --all-targets`, and `cargo test-ci`, including colocated unit regressions. Author required e2e/integration/conformance scenarios and executable walkthroughs; their heavy execution belongs to Tier 2 CI, not the per-task gate. Retain only CLAUDE.md's conditional e2e run for Claude-routed tasks deleting/replacing code paths. Walkthrough creation uses showboat; any verification uses `dev/bin/safe-showboat-verify`, never the project root. This backlog assumes 00171 has completed the shell-to-Rust harness migration.

### Critical Scenarios
- **Happy path**: POST a document, GET it, PUT a field, GET again, DELETE, GET → Expected: 201/200/200/200/200/404, results matching the canonical core type at every step.
- **Edge case**: PUT to a nonexistent id and POST with an invalid type → Expected: 404 and validation error respectively, codes and redaction byte-identical to REST's for the same inputs.
- **Error case**: a write that produces core warnings (e.g. unknown field per current policy) → Expected: the warning surfaces in the NoSQL response exactly as AppOutput carries it — never dropped.

## Risks

- **Contract not actually thin**: the verbs might need real logic in the adapter, revealing 00172-00178 gaps. Mitigation: that is a designed-for outcome — measure, file against the contract, do not absorb the logic into the transport.
- **Mirror consistency trap**: keeping redb as the read store (decided) makes write-then-read coherence depend on the dual-write — so the dual-write stops being best-effort on the NoSQL write path. Mitigation: the write-then-read test; the per-call rebuild (`open_nosql_fresh`) remains the cross-process freshness floor until design shows an incremental mirror is cheap.
- **Experimental-label expectations**: the API is labeled experimental (`ddb-cli/src/main.rs:439`); adding writes changes its weight. Mitigation: docs task states the interface's status explicitly after this PRD (drop or keep the label deliberately).

**Capability registry (depends on 00186):** add `nosql_http_crud` to the existing sorted registry only with this implemented surface; document the applicable interface/workflow and compile-time availability. Internal refactors and tests are not separate flags.

## Transport Impact

- **CLI**: Guaranteed — unchanged; shares the contract commands.
- **GraphQL**: Guaranteed — unchanged; parity reference for warnings.
- **REST**: Guaranteed — unchanged; parity reference for error shapes.
- **PgWire**: Guaranteed — unchanged.
- **FFI**: Guaranteed — unchanged (00177 owns its contract routing).
- **NoSQL HTTP**: Guaranteed — gains create/update/delete; reaches the CRUD baseline; consistency guarantee documented.
- **Cross-interface conformance plan**: the golden CRUD workflow runs on CLI, GraphQL, REST, and NoSQL with the same assertions; error/warning parity NoSQL↔REST pinned in tests; the thinness measurement feeds back into the contract work. jink impact: none directly today (jink uses GraphQL); the platform stops carrying a public interface that silently cannot write.
