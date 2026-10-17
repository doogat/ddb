---
default_model: opus
model_tier_rationale: cross-cutting shape change - one error table consumed by five transports and by 00177/00180
---

# Unify server error-mapping paths

**Priority:** P2 · **Roadmap:** Phase 2 (app contract) · **Depends on:** none · **Feeds:** 00177 (FFI error variants), 00180 (PgWire redaction).

## Overview

### Problem Statement
The same underlying error renders through **four** divergent mappers that disagree on code, redaction, HTTP status, and structured context, and a fifth surface (FFI) has its own lossy table. The 00161 doubt review surfaced only the tip; the audit found the full fan-out:

1. **GraphQL** → `to_graphql_error_from_app` (`ddb-server/src/error.rs`): converts to `app_contract::AppError`, consults `AppErrorCategory`, redacts `Internal`-category to `"internal error"` and drops `field`/`details` on that branch only (non-Internal categories keep them, `error.rs:126-142`).
2. **REST + NoSQL HTTP** → `classify` (`ddb-server/src/error.rs:9`) + `http_error_response` (`ddb-server/src/http_error.rs`): switch on legacy `DoogatError`, pass `Structured { code, message }` through **verbatim**, never consult `AppErrorCategory`.
3. **FFI** → its own allowlist in `ddb-core/src/ffi/records.rs:72-129` (`impl From<DoogatError> for DdbError`): routes `Structured` codes by hand and **lossily collapses** the rest — `SINGLETON_NOT_FOUND`→`Validation` (not NotFound, via the Structured catch-all), `Conflict`→`Git`, `Sync`→`Git`, `Index`→`Sql` (records.rs:98-100).
4. **PgWire** → `pgwire.rs:107,114` (plus the 7 sibling sites `:157,:180,:183,:186,:189,:255,:260`): wraps every failure as `ApiError(Box::new(e))` — raw `DoogatError` Display, no code, no category, **no redaction** (leaks `git:`/`io:` detail GraphQL redacts).

**Root cause, verified**: there is no single policy, and `From<DoogatError> for AppError` (`ddb-core/src/app_contract/error.rs:49`) has **no `SqlEngine` arm** — it falls through to the `INTERNAL_ERROR` catch-all (`error.rs:148`, category `Internal`, redacted). Meanwhile `classify` (`error.rs:21`) maps `SqlEngine` → `SQL_ERROR` with the message verbatim. So a SQL syntax error — the most common user error on the SQL surface — ships as `SQL_ERROR` + full message on REST/PgWire but as redacted `INTERNAL_ERROR` "internal error" on GraphQL. `SQL_ERROR` is on the wire but is not an `AppError` code at all.

Two costs: (1) cross-interface parity violation (the AGENTS.md 120% posture) plus a redaction-policy inconsistency (REST/PgWire surface nested engine errors the policy redacts on GraphQL); (2) a maintainability hazard — every new code must be taught up to four mappers that already disagree, and the 00161 cycle initially forgot to teach `http_error.rs` two new codes.

### Target Users
Downstream consumers (jink and future apps) that branch on error codes/shapes across GraphQL/REST/NoSQL/FFI/PgWire and need consistent semantics; ddb maintainers who add error codes and currently must update up to four mappers in lockstep.

### Success Metrics
- A single error-rendering decision (`code`, redaction-applied `message`, `http_status`, `structured_context`, `category`, and an `ffi_variant` hint) lives in ONE place and is the source of truth for every transport.
- For any `DoogatError`/`AppError`, GraphQL and REST return the SAME code and the SAME redaction decision; HTTP status is a per-CODE column of the one table (DECIDED 2026-09-05 backlog review): today's REST statuses are preserved — `SQL_ERROR`, `NOT_NULL_VIOLATION`, `UNKNOWN_FIELD`, `TYPE_NOT_REGISTERED` stay 422 (`http_error.rs:18,:27-29`, pinned by `rest.rs:519-521` and `nosql_api.rs:151-156`), `VALIDATION_ERROR` stays 400, Conflict→409, NotFound→404, Internal→500; the category drives redaction and the `ffi_variant`, never the status. (Naming note: there is NO `Validation` category variant — `DoogatError::Validation` and the `VALIDATION_ERROR` code map TO category `InvalidInput`; do not mint a fifth variant.)
- **SQL errors agree across surfaces**: `SqlEngine` maps to one code (`SQL_ERROR`) with one category and one redaction decision on GraphQL, REST, and PgWire — the missing `AppError` arm is added and `classify`'s divergent handling is retired.
- The policy exposes the `ffi_variant` and the redaction decision that 00177 (FFI) and 00180 (PgWire) consume, so those surfaces stop diverging (they are wired in their own PRDs; this PRD defines the surface and proves it for GraphQL/REST/NoSQL).
- Adding a new error code requires touching ONE table; a conformance test asserts GraphQL↔REST parity for a representative code set including `SCHEMA_APPLY_PARTIAL`, `SCHEMA_DESTRUCTIVE_BLOCKED`, and `SQL_ERROR`.
- No silent response regression: existing per-interface error tests stay green or are updated to the unified contract under the §3.3 compatibility checklist.

## Functional Decomposition

### Capability: Unified error classification
A single source of truth that turns any server-surfaced error into a transport-neutral rendering decision.

#### Feature: Complete the AppError conversion
- **Description**: `From<DoogatError> for AppError` gains the missing `SqlEngine` arm so the app path and the legacy `classify` path agree.
- **Inputs**: a `DoogatError::SqlEngine(msg)`.
- **Outputs**: `AppError { code: "SQL_ERROR", category: InvalidInput, message: msg, .. }`.
- **Behavior**: add the explicit arm (mirroring `Parse`/`Validation` which are `InvalidInput`); register `SQL_ERROR` in the `codes` module so it is a first-class `AppError` code, not a wire-only string.

#### Feature: Single error-policy function
- **Description**: one function maps `DoogatError`/`AppError` to an `ErrorRendering` carrying `{ code, message (redaction applied), http_status, structured_context, category, ffi_variant }`.
- **Inputs**: a `DoogatError` (legacy transport entry) or an already-built `AppError`.
- **Outputs**: an `ErrorRendering` struct.
- **Behavior**: route `DoogatError` through the `From<DoogatError>` conversion so `AppErrorCategory` drives redaction AND the `ffi_variant` hint, while `http_status` is looked up per code in the same table (422 for `SQL_ERROR`/`NOT_NULL_VIOLATION`/`UNKNOWN_FIELD`/`TYPE_NOT_REGISTERED` as REST serves them today, 400 for `VALIDATION_ERROR`, otherwise the category default 409/404/500); `Internal`-category redacts to `"internal error"` and drops context; non-`Internal` preserves message + structured context.

#### Feature: Transport adapters consume the unified policy
- **Description**: GraphQL, REST, and NoSQL HTTP renderers all derive output from the single `ErrorRendering`; the policy also exposes the `ffi_variant`/redaction that FFI (00177) and PgWire (00180) will consume.
- **Inputs**: an `ErrorRendering`.
- **Outputs**: the per-transport error shape (GraphQL `Error` + `extensions`; REST/NoSQL `(StatusCode, ErrorBody)`) mapped mechanically.
- **Behavior**: each adapter applies only transport encoding; none re-decides redaction or status. `http_error_response` and `classify` collapse to thin wrappers; the `SqlEngine`-specific branch in `classify` is removed (the policy now covers it).

### Capability: Cross-interface conformance
Pins the parity so it cannot silently drift again.

#### Feature: Error-parity conformance tests
- **Description**: tests asserting GraphQL, REST, and NoSQL render the same code + redaction + status for a representative error set, and that the `ffi_variant` hint exists for each.
- **Inputs**: a table of `(error sample -> expected code / status / redaction / ffi_variant)` covering Validation, NotFound, Conflict, Internal, `SCHEMA_APPLY_PARTIAL`, `SCHEMA_DESTRUCTIVE_BLOCKED`, and `SQL_ERROR`.
- **Outputs**: passing parity tests.
- **Behavior**: drive each sample through each transport renderer and assert equality.

## Structural Decomposition

### Repository Structure
```text
ddb-core/src/
├── error.rs            # Maps to: Complete the AppError conversion (register codes::SQL_ERROR)
└── app_contract/
    └── error.rs        # Maps to: Complete the AppError conversion (add SqlEngine arm; own the sole neutral ErrorRendering policy)
ddb-server/src/
├── error.rs           # Maps to: Unified error classification (classify/to_graphql encode the core ErrorRendering)
├── http_error.rs      # Maps to: Transport adapters (REST/NoSQL) — thin wrapper over the policy
├── rest.rs            # Maps to: Transport adapters (REST) — rest_error delegates
└── nosql_api.rs       # Maps to: Transport adapters (NoSQL HTTP) — consumes the policy
ddb-server/tests/
└── error_parity_*.rs  # Maps to: Error-parity conformance tests
docs/src/technical/
├── errors.md          # Maps to: documents the unified policy and ffi_variant/redaction surface
└── server.md          # Maps to: documents the unified policy and ffi_variant/redaction surface
CHANGELOG.md            # Maps to: compatibility note for the REST/GraphQL SQL-error and redaction shape change
```

### Module: app_contract::error (ddb-core/src/app_contract/error.rs)
- **Maps to capability**: Unified error classification
- **Responsibility**: own the single code-to-category/status/redaction/FFI-hint policy in core, complete the SqlEngine conversion, and derive existing category conversions from that table.
- **Exports**: `AppError`, `AppErrorCategory`, `codes::SQL_ERROR`, `ErrorRendering`, and `error_rendering`; status is a neutral integer and FFI hints are core enums, with no HTTP/UniFFI/server dependencies.

### Module: error (ddb-server/src/error.rs)
- **Maps to capability**: Unified error classification
- **Responsibility**: encode the core error policy as GraphQL errors or server response metadata; never re-decide classification.
- **Exports**: `to_graphql_error_from_app` and `classify` as thin adapters over `ddb_core::app_contract` rendering.

### Module: http_error (ddb-server/src/http_error.rs)
- **Maps to capability**: Transport adapters (REST/NoSQL)
- **Responsibility**: encode an `ErrorRendering` as `(StatusCode, Json<ErrorBody>)`.
- **Exports**: `http_error_response(DoogatError) -> (StatusCode, Json<ErrorBody>)` — thin wrapper.

### Module: HTTP callers and parity fixtures (ddb-server/src/rest.rs, nosql_api.rs, ddb-server/tests/error_parity_*.rs)
- **Maps to capability**: Unified error classification; Cross-interface conformance
- **Responsibility**: route HTTP failures through the core policy and verify identical codes/redaction across server transports.
- **Exports**: existing handlers and automated error-parity fixtures.

## Dependency Graph

### Foundation Layer (Phase 0)
No dependencies — built first.
- **app_contract SqlEngine arm + `ErrorRendering` policy**: provides the transport-neutral rendering + status/redaction/ffi-variant decision.

### Core Layer (Phase 1)
- **http_error / rest / nosql_api**: Depend on [policy]. Re-express REST + NoSQL over it.
- **error (graphql conversion)**: Depends on [policy]. Re-express `to_graphql_error_from_app` over it.

### Integration Layer (Phase 2)
- **error_parity tests + docs**: Depend on [policy, adapters]. Pin parity; document the contract; hand the `ffi_variant`/redaction surface to 00177/00180.

## Implementation Phases

### Phase 0: Foundation
**Goal**: complete the conversion and define the single policy, no public-shape change yet.

**Tasks**:
- [ ] Add the `SqlEngine` arm to `From<DoogatError> for AppError` (`SQL_ERROR`, category `InvalidInput`) and register `codes::SQL_ERROR` (no deps) — Acceptance: a unit test asserts `AppError::from(DoogatError::SqlEngine(..)).code == "SQL_ERROR"` and category `InvalidInput`; `SQL_ERROR` is no longer a wire-only string.
- [ ] Define the adapter-neutral `ErrorRendering` + `error_rendering(...)` in `ddb-core/src/app_contract/error.rs`; derive the existing core category conversion from its sole policy table, then route `DoogatError` through `AppError`/`AppErrorCategory`, exposing `http_status`, `redacted message`, `structured_context`, and `ffi_variant` (depends on: SqlEngine arm) — Acceptance: unit tests cover every registered code's category, status, redaction, and FFI hint from the sole table, including `SCHEMA_APPLY_PARTIAL` (Internal→500, redacted), `SCHEMA_DESTRUCTIVE_BLOCKED` (Conflict→409), `SQL_ERROR` (InvalidInput, 422 preserved, message preserved), and `VALIDATION_ERROR` (InvalidInput, 400).

**Exit Criteria**: the policy exists, is unit-tested, and covers SqlEngine; no transport consumes it yet.

### Phase 1: Core
**Goal**: every HTTP/GraphQL transport renders from the single policy.

**Tasks**:
- [ ] Re-express `http_error_response` (REST) over the policy; remove `classify`'s SqlEngine branch (depends on: Phase 0) — Acceptance: `http_error_t143` updated; `SCHEMA_APPLY_PARTIAL` REST message redacted to match GraphQL; `SQL_ERROR` still renders 422 with message preserved on REST (no HTTP status changes on any code — `rest.rs:519-521` and `nosql_api.rs:151-156` stay green unmodified); 409/500 mapping preserved.
- [ ] Re-express the NoSQL HTTP error path over the policy (depends on: Phase 0) — Acceptance: NoSQL error shape matches REST for the shared code set.
- [ ] Re-express `to_graphql_error_from_app` over the policy (depends on: Phase 0) — Acceptance: GraphQL error tests stay green; a SQL error now renders `SQL_ERROR` (not redacted `INTERNAL_ERROR`) consistently with REST.

**Exit Criteria**: GraphQL, REST, NoSQL derive code/redaction/status from the one policy; SQL errors agree.

### Phase 2: Integration
**Goal**: lock parity, document the contract, hand off the FFI/PgWire surface.

**Tasks**:
- [ ] Add error-parity conformance tests across GraphQL/REST/NoSQL for the representative table incl. `SQL_ERROR` (depends on: Phase 1) — Acceptance: identical code + redaction across transports, status per code.
- [ ] Update `docs/src/technical/errors.md` + `server.md`; run the §3.3 compatibility/deprecation checklist for the changed REST/GraphQL SQL-error shape; CHANGELOG compatibility note (depends on: Phase 1) — Acceptance: docs describe ONE policy and the `ffi_variant`/redaction surface that 00177/00180 consume; the SQL-error and SCHEMA_APPLY_PARTIAL REST changes are recorded.

**Exit Criteria**: parity tests green; docs + compatibility note landed; FFI/PgWire surface defined for their PRDs.

## Test Strategy

**Validation:** per-task local gates are `cargo build`, `cargo clippy --workspace --all-targets`, and `cargo test-ci`, including colocated unit regressions. Author required e2e/integration/conformance scenarios and executable walkthroughs; their heavy execution belongs to Tier 2 CI, not the per-task gate. Retain only CLAUDE.md's conditional e2e run for Claude-routed tasks deleting/replacing code paths. Walkthrough creation uses showboat; any verification uses `dev/bin/safe-showboat-verify`, never the project root. This backlog assumes 00171 has completed the shell-to-Rust harness migration.

### Critical Scenarios
- **Happy path**: a `Validation` error → code `VALIDATION_ERROR`, message preserved, 400 on REST, same code on GraphQL.
- **Edge case**: `SCHEMA_APPLY_PARTIAL` (Internal) → message redacted to `"internal error"` on BOTH GraphQL and REST, REST status 500.
- **SQL case**: a SQL syntax error → `SQL_ERROR` + message on BOTH GraphQL and REST (no longer redacted `INTERNAL_ERROR` on GraphQL), REST status unchanged (422); the same code is available to PgWire (00180).
- **Error case**: a brand-new structured code with category Conflict → 409 on REST + same code on GraphQL with NO mapper edit beyond the single table.

## Risks
- **Public error-shape change (two directions)**: REST currently passes some `Internal` messages verbatim (unifying redaction will redact them) AND GraphQL currently redacts SQL errors (unifying will now surface `SQL_ERROR` + message). Mitigation: run the §3.3 compatibility checklist for both, add CHANGELOG compatibility notes, and treat the SQL-error change as a fix (it makes the most common user error legible on GraphQL).
- **FFI/PgWire scope**: this PRD defines the `ffi_variant`/redaction surface but wires FFI in 00177 and PgWire in 00180 to stay session-sized. Mitigation: ship the surface + a unit test that the `ffi_variant` exists for each category, so the consuming PRDs have a stable target; do NOT leave FFI/PgWire silently divergent — they are explicitly tracked, not ignored.
- **CLI already uses the facade**: CLI (`app_err`) renders via `AppError`; confirm it inherits the SqlEngine arm correctly (SQL errors now `SQL_ERROR` not `INTERNAL_ERROR` at the CLI too) and update its expectation if pinned.
- **Status column is per code, not per category** (decided 2026-09-05): keeping 422 for `SQL_ERROR`/`NOT_NULL_VIOLATION`/`UNKNOWN_FIELD`/`TYPE_NOT_REGISTERED` while `VALIDATION_ERROR` is 400 means two statuses under `InvalidInput`. Mitigation: document it as deliberate in `errors.md` (no public status change on any transport) so a later reader does not "fix" it into a compat break.

## Transport Impact
- **CLI**: Guaranteed — renders via the `app_contract` facade; inherits the new SqlEngine arm (SQL errors become `SQL_ERROR`). Action: confirm + update any pinned expectation.
- **GraphQL**: Guaranteed — `to_graphql_error_from_app` consumes the policy; SQL errors now legible, other redaction unchanged.
- **REST**: Guaranteed — `http_error_response`/`classify` consume the policy; `Internal` messages redacted, status per code, `SQL_ERROR` preserved.
- **NoSQL HTTP**: Guaranteed — error path consumes the policy for shape parity with REST.
- **FFI**: Guaranteed via the policy's `ffi_variant` — the lossy allowlist (`records.rs:72-129`) is replaced by the policy-derived variant in **00177**; this PRD defines and unit-tests that variant surface.
- **PgWire**: Guaranteed via the policy's redaction — PgWire stops shipping raw errors by consuming the policy in **00180**; this PRD provides the redacted message + code it will use.
- **Cross-interface conformance plan**: the error-parity table (Phase 2) drives the representative set (incl. `SQL_ERROR`) through GraphQL/REST/NoSQL asserting identical code + redaction; a unit test asserts the `ffi_variant` exists per category; the §3.3 checklist gates the REST/GraphQL SQL-error and redaction changes. 00177 and 00180 add the FFI and PgWire legs of the conformance matrix.
