---
default_model: opus
model_tier_rationale: security-sensitive - constant-time compare, DoS bounds, redaction parity
---

# Server security and resource hardening

**Priority:** P2 · **Roadmap:** Phase 2 (security) · **Depends on:** 00173 (unified server error policy — the PgWire redaction here derives from it), 00176 (contract `PageRequest` policy — the NoSQL scan cap consumes it)

> **Maintainer decision (2026-07-13, purpose-fit critique):** PgWire remains a Guaranteed WRITE surface — DML/DDL stay enabled, no SELECT-only gate. Its two riders (write-semantics docs; DML-error redaction case) are folded into the Phase 1/2 task acceptance below. Whether AppOutput warnings surface as PG notices is deferred to the app-contract PRDs (00175-00178) — record it there when picked up.

> **Resolved (2026-08-14 backlog review):** the 2026-07-16 ws scope-correction's "missing regression test" already exists — `tests/e2e/subscriptions.rs:59` (`ws_auth_invalid_header_returns_401`), `:108` (`ws_payload_auth_invalid_token`), `:142` (`ws_no_auth_no_payload_rejected`), plus the authenticated happy path at `:84`. `/ws` auth enforcement itself was verified correct (`ws.rs:30-68`, `:70-85`). No ws work remains in this PRD; the success metric (no data-bearing endpoint without the token; only `/health*` exempt) is already covered.

> **Provenance:** the NoSQL scan/backlinks cap was added by the 2026-07-16 assessment (F16) and folded into the body's Features/tasks at the 2026-08-14 backlog review. Re-verified at `d77ad58`: `nosql_scan_type`/`nosql_scan_tag`/`nosql_backlinks` (`ddb-core/src/service/utility.rs:132,:145,:158`) return the full id set with no limit parameter, while REST clamps `per_page` to 200 (`rest.rs:278`).

## Overview

### Problem Statement
The server ships without several standard denial-of-service guards and carries minor auth/transport hardening gaps. Each is a single, verifiable omission:

- **No GraphQL depth / complexity limit.** `ddb-server/src/schema/mod.rs:53`-`:95` builds the schema and finishes with only `.extension(WarningExtensionFactory)` (`:94`) — there is no `.limit_depth(...)`, `.limit_complexity(...)`, or `.disable_introspection()` anywhere in `ddb-server` (confirmed absent repo-wide). One authenticated client can pin the single actor thread with a deeply nested relation / backlink query.
- **No global request body limit.** REST Json inherits axum's smaller default; async-graphql-axum 7.2.1 reads the body stream directly and bypasses DefaultBodyLimit. A limit before extraction is required across all routes.
- **PgWire SELECT buffers the full result with no row cap.** `ddb-server/src/pgwire.rs` `do_query` (`:88`) collects rows and hands them to `build_rows_response` (`:218`) with no `LIMIT` backstop, so a large SELECT materializes the entire result set in memory.
- **Non-constant-time token compare.** `ddb-server/src/auth.rs:30`-`:34` (`validate_bearer`) compares with `t == expected` (`:33`), a short-circuiting comparison.
- **Non-loopback bind accepted silently, no TLS.** `ddb-server/src/config.rs:52`-`:55` defaults `bind` to `127.0.0.1` (`:55`) but accepts `--bind 0.0.0.0` with no warning; the bearer token then crosses the network in cleartext.
- **PgWire leaks unredacted internal errors.** `ddb-server/src/pgwire.rs:107` and `:114` (and the sibling sites `:157`, `:180`-`:189`, `:255`-`:260`) ship `PgWireError::ApiError(Box::new(e))` raw, exposing `git:` / `io:` internal detail that the GraphQL and REST paths redact.

### Target Users
Operators who expose ddb beyond localhost (jink deploying the server for a remote frontend), plus every authenticated client that shares the single actor thread. They need the server to survive a runaway or hostile query and to not leak internals or tokens.

### Success Metrics
- A query deeper/costlier than the configured bound is rejected before it reaches the actor, verified by a server test.
- An over-limit request body is rejected with the configured status, verified by a server test.
- A PgWire SELECT with no explicit `LIMIT` is capped by a default row cap (or streamed), verified by a test.
- Equal-length token comparison is content-independent through subtle::ConstantTimeEq; unequal lengths reject explicitly. Functional tests cover behavior, without claiming to prove constant-time execution by timing.
- A non-loopback `bind` logs a startup WARNING recommending a TLS-terminating proxy; a test asserts the warning fires for `0.0.0.0` and not for `127.0.0.1`.
- A PgWire internal error is redacted through the unified error policy (00173), matching GraphQL/REST; a test asserts no `git:`/`io:` detail crosses the wire — for a DML-triggered error as well as SELECT/internal ones.
- NoSQL scan/backlinks responses are capped per the 00176 `PageRequest` policy; no endpoint returns an unbounded id set.

## Functional Decomposition

### Capability: Query-cost DoS guards
#### Feature: GraphQL depth + complexity limits
- **Description**: bound query depth and complexity at schema build; optionally disable introspection for production.
- **Inputs**: the `Schema::build(...)` builder in `schema/mod.rs`.
- **Outputs**: a schema that rejects over-bound queries with a GraphQL error.
- **Behavior**: chain `.limit_depth(16)` and `.limit_complexity(2000)` onto the builder (DECIDED 2026-09-05 backlog review; config keys `graphql_max_depth` / `graphql_max_complexity` in `config.rs`, overridable); gate `.disable_introspection()` behind `graphql_introspection: bool` (default `true`; `false` — set via config or `--production` — disables it).

#### Feature: PgWire SELECT row cap
- **Description**: bound the rows a single PgWire SELECT buffers.
- **Inputs**: `do_query` / `build_rows_response`.
- **Outputs**: a capped (or streamed) result.
- **Behavior**: apply a default row cap of 10_000 rows (= GraphQL `MAX_LIMIT`; DECIDED 2026-09-05) when the query carries no explicit `LIMIT`; an explicit `LIMIT` is honored as-is; document the cap.

#### Feature: NoSQL scan/backlinks cap
- **Description**: the NoSQL scan and backlinks endpoints stop returning unbounded id sets.
- **Inputs**: `GET /nosql/?type=|?tag=` and `GET /nosql/{id}/backlinks` requests; the 00176 contract `PageRequest` policy.
- **Outputs**: capped/paginated id lists consistent with the `PageRequest` default and max.
- **Behavior**: thread normalized limit/offset through `nosql_scan_type`/`nosql_scan_tag`/`nosql_backlinks` (`service/utility.rs:132,:145,:158`) and the `nosql_api.rs` routes, applying the 00176 policy so one authed client cannot materialize an entire large type's id vector.

### Capability: Request body limits
#### Feature: Router-wide body limit
- **Description**: cap request-body size across every extractor, including GraphQL.
- **Inputs**: the axum router assembly in `ddb-server/src/lib.rs`.
- **Outputs**: over-limit requests rejected with a 413-class response.
- **Behavior**: apply a global RequestBodyLimitLayer or equivalent bounded reader before extraction, capped at 8 MiB by max_body_bytes. Align REST/NoSQL Json DefaultBodyLimit to the same bound so no implicit 2 MiB limit remains. Oversized bodies yield HTTP 413, including streamed requests without Content-Length.

### Capability: Auth + transport hardening
#### Feature: Constant-time token compare
- **Description**: compare equal-length bearer-token bytes content-independently using the audited subtle primitive, with explicit unequal-length rejection.
- **Inputs**: `validate_bearer` in `auth.rs`.
- **Outputs**: a boolean computed content-independently for equal-length byte slices; unequal lengths reject without promising length secrecy.
- **Behavior**: compare via `subtle::ConstantTimeEq` (already in `Cargo.lock` transitively at 2.6.1; add it as a direct `ddb-server` dependency) instead of `==`.

#### Feature: Non-loopback bind warning
- **Description**: warn at startup when the server binds a non-loopback address without TLS.
- **Inputs**: the resolved `bind` value in `config.rs` / server startup.
- **Outputs**: a logged WARNING recommending a TLS-terminating proxy.
- **Behavior**: detect a non-loopback bind and log once at startup; do not block (operators may front it with a proxy).

### Capability: PgWire error redaction parity
#### Feature: Route PgWire errors through the unified policy
- **Description**: redact PgWire errors the same way GraphQL/REST do.
- **Inputs**: the `map_err(... Box::new(e))` sites in `pgwire.rs`.
- **Outputs**: a redacted, classified error before `ApiError`.
- **Behavior**: pass the error through the 00173 `classify` / unified policy so `git:`/`io:` detail is stripped before it becomes an `ApiError`.

## Structural Decomposition

### Repository Structure

```
ddb-server/src/
├── schema/mod.rs        # Maps to: GraphQL depth + complexity limits
├── lib.rs               # Maps to: Router-wide body limit
├── pgwire.rs            # Maps to: PgWire SELECT row cap; PgWire error redaction parity
├── auth.rs              # Maps to: Constant-time token compare
├── config.rs            # Maps to: Non-loopback bind warning; the bound config keys (graphql_max_depth, graphql_max_complexity, graphql_introspection, max_body_bytes)
├── nosql_api.rs         # Maps to: NoSQL scan/backlinks cap (routes thread the limit)
└── error.rs             # Maps to: PgWire error redaction parity (shared classify)
ddb-core/src/service/
└── utility.rs           # Maps to: NoSQL scan/backlinks cap (nosql_scan_type/tag/backlinks :132,:145,:158)
```

### Module: schema (ddb-server/src/schema/mod.rs)
- **Maps to capability**: Query-cost DoS guards
- **Responsibility**: build the dynamic schema with depth/complexity bounds.
- **Exports**: the schema builder (now bounded).

### Module: pgwire (ddb-server/src/pgwire.rs)
- **Maps to capability**: Query-cost DoS guards; PgWire error redaction parity
- **Responsibility**: cap SELECT rows; redact wire errors.
- **Exports**: `do_query`, `build_rows_response` (capped + redacted).

### Module: auth / config / lib (ddb-server/src/)
- **Maps to capability**: Auth + transport hardening; Request body limits
- **Responsibility**: constant-time compare; bind warning; body-limit layer.
- **Exports**: `validate_bearer` (constant-time); router assembly (body-limited); startup bind check.

### Module: NoSQL pagination and error adapters (ddb-server/src/nosql_api.rs, error.rs; ddb-core/src/service/utility.rs; ddb-server/Cargo.toml)
- **Maps to capability**: Query-cost DoS guards; PgWire error redaction parity; Auth + transport hardening
- **Responsibility**: apply the shared PageRequest policy, encode core error rendering, and declare the direct subtle dependency.
- **Exports**: bounded NoSQL reads and existing transport error helpers.

## Dependency Graph

### Foundation Layer (Phase 0)
No dependencies - built first.

- **DoS guards (depth/complexity, body limit, PgWire row cap)**: independent, additive layers.

### Core Layer (Phase 1)
- **auth + bind hardening**: independent of the DoS guards.
- **PgWire redaction**: Depends on [00173 unified error policy] - reuse `classify` so redaction matches GraphQL/REST.

### Integration Layer (Phase 2)
- **tests + docs**: Depends on [all Phase 0/1 items].

## Implementation Phases

### Phase 0: Foundation
**Goal**: land the DoS guards.

**Tasks**:
- [ ] Add `.limit_depth(16)` + `.limit_complexity(2000)` (config `graphql_max_depth` / `graphql_max_complexity`) to the schema builder and `.disable_introspection()` gated on `graphql_introspection == false` (default `true`) (no deps) - Acceptance: a query of depth 17 (and one over complexity 2000) is rejected with a GraphQL error; depth 16 passes; a normal query is unaffected; introspection is answered by default and refused when the flag is false; a server test pins each.
- [ ] Enforce max_body_bytes (default 8 MiB) globally before extraction with RequestBodyLimitLayer or equivalent, and align Json's extractor bound (no deps) - Acceptance: valid exact-limit requests pass the size gate and limit+1 requests return 413 through REST, GraphQL, and NoSQL, both with Content-Length and chunked/streamed bodies without it; do not rely on DefaultBodyLimit alone.
- [ ] Add a default row cap of 10_000 to PgWire SELECT in `do_query`/`build_rows_response` (no deps) - Acceptance: a SELECT with no `LIMIT` over 10_001 rows returns 10_000; an explicit `LIMIT` is honored as-is; a test asserts the cap.
- [ ] Cap the NoSQL scan/backlinks endpoints via the 00176 `PageRequest` policy: thread normalized limit/offset through `nosql_scan_type`/`nosql_scan_tag`/`nosql_backlinks` and the `nosql_api.rs` routes (depends on: 00176) - Acceptance: a scan over a type with more ids than the default page returns the capped page (default 50/max 10,000 and offset 0 honored with checked pagination; a limit above the max is rejected with `InvalidInput`, the 00176 rule); a test asserts no unbounded id vector is materialized on the wire.

**Exit Criteria**: depth/complexity, body-limit, PgWire row-cap, and NoSQL scan-cap guards active and tested.

### Phase 1: Core
**Goal**: auth + transport + redaction hardening.

**Tasks**:
- [ ] Replace == in validate_bearer with subtle::ConstantTimeEq on equal-length bytes and explicit unequal-length rejection (no deps) - Acceptance: correct, wrong, equal-prefix, empty, and different-length tokens produce the expected auth result; tests pin use of the audited primitive without a flaky timing assertion or token-length-secrecy claim.
- [ ] Log a startup WARNING when `bind` is non-loopback (no deps) - Acceptance: a test asserts the warning fires for `0.0.0.0` and not for `127.0.0.1`.
- [ ] Route the `pgwire.rs` error sites through the 00173 `classify` / unified policy before `ApiError` (depends on: 00173) - Acceptance: a test asserts a `git:`/`io:` internal error is redacted on the PgWire wire, matching GraphQL/REST; the redaction test matrix includes a DML-triggered error (e.g. a failing `INSERT`), not only SELECT/internal errors.

**Exit Criteria**: constant-time auth, bind warning, and PgWire redaction parity landed.

### Phase 2: Integration
**Goal**: scenario coverage + docs.

**Tasks**:
- [ ] Add the scenarios to the Rust e2e harness (`tests/e2e/`; the shell harness was removed by 00171): too-deep query rejected, oversized body rejected, non-loopback bind warning, PgWire error redacted (depends on: Phase 0/1) - Acceptance: all four e2e scenarios authored and green (this absorbs the previously-separate too-deep-query e2e).
- [ ] Update `docs/src/technical/server.md` (limits, bind/TLS guidance, PgWire cap + redaction, NoSQL scan cap, and verified transaction ownership); add a `CHANGELOG.md` entry; push `master` after commits land (depends on: Phase 0/1) - Acceptance: premise re-checked at execution: FT-6 (`2f04f3a`) rejects cross-request transaction verbs on the Shared actor, while embedded Exclusive transactions and request-local batches retain their documented boundaries. Preserve that distinction and DROP/table-rename rollback limits. If 00206 has since shipped, describe only its conformance-proven session behavior; otherwise retain the rejection. Skip and report already-correct prose. This PRD must not enable shared cross-request transactions. Docs match the verified branch and CHANGELOG is present.

**Exit Criteria**: scenarios + e2e + docs landed.

## Test Strategy

**Validation:** per-task local gates are `cargo build`, `cargo clippy --workspace --all-targets`, and `cargo test-ci`, including colocated unit regressions. Author required e2e/integration/conformance scenarios and executable walkthroughs; their heavy execution belongs to Tier 2 CI, not the per-task gate. Retain only CLAUDE.md's conditional e2e run for Claude-routed tasks deleting/replacing code paths. Walkthrough creation uses showboat; any verification uses `dev/bin/safe-showboat-verify`, never the project root. This backlog assumes 00171 has completed the shell-to-Rust harness migration.

### Critical Scenarios
- **Happy path**: a normal-depth GraphQL query and a small REST body → Expected: unaffected; identical responses to today.
- **Edge case**: a query exactly at the depth/complexity bound and a body exactly at the limit → Expected: accepted; one past the bound → rejected.
- **Error case**: a PgWire query that triggers a `git:`/`io:` internal error → Expected: a redacted error on the wire (no internal detail), matching GraphQL/REST.

## Risks

- **Tuning the bounds too tight**: a legitimate deep query or large body could break. Mitigation: the defaults (depth 16, complexity 2000, 8 MiB, 10_000 rows) are config-overridable; design sanity-checks depth 16 against the deepest relation fixture in `tests/e2e/typed_where_filter.rs` and raises it if a legitimate fixture exceeds it; document.
- **PgWire row cap changes result semantics**: a client expecting all rows gets a capped set. Mitigation: cap only when no explicit `LIMIT`; document the default; prefer streaming if cheap.
- **00173 coupling**: PgWire redaction reuses the unified `classify`. Mitigation: depend on 00173; if it is not yet merged, gate this one task and land the rest.
- **`disable_introspection` breaks tooling**: some clients rely on introspection. Mitigation: gate it behind a production flag, default on in dev.

## Transport Impact

These are server-side guards; the in-process interfaces (CLI, FFI) have no network surface and are unaffected.

- **CLI**: Intentionally absent — in-process, no network/server surface; DoS, auth, and bind guards do not apply.
- **GraphQL**: Guaranteed — depth + complexity limits, optional introspection-disable, router body limit, and constant-time auth all apply.
- **REST**: Guaranteed — router body limit + constant-time auth + bind warning apply (shared router/middleware).
- **FFI**: Intentionally absent — in-process library binding, no network surface.
- **PgWire**: Specialized — no GraphQL-style depth limit applies, but it gains a SELECT row cap, unified-policy error redaction, and (shared) the bind warning.
- **NoSQL HTTP**: Guaranteed — router body limit + constant-time auth apply (shares the axum router/middleware).
- **Cross-interface conformance plan**: server-crate tests + the Rust e2e scenarios (`tests/e2e/`; the shell harness was removed by 00171) pin the depth-limit rejection, the body-limit rejection, and the PgWire redaction; the constant-time compare and bind warning are unit-tested. jink impact: ddb is safer when exposed beyond localhost, and a runaway frontend query can no longer pin the server.
