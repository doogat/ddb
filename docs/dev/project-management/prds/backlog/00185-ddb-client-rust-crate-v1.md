---
catchup: force
default_model: opus
model_tier_rationale: invented contract - a new public crate API downstreams build against
---
catchup: force

# Publish a ddb-client Rust crate (MVP)

**Priority:** P3 · **Roadmap:** Phase 3 (downstream enablement) · **Depends on:** none (cross-ref: 00173 unifies the server error-mapping paths, which sharpens the typed error codes this crate surfaces)

## Overview

### Problem Statement
Every downstream Rust app that talks to `ddb serve` hand-rolls the same GraphQL HTTP client. There is no shared, reusable workspace client, so the integration boilerplate is copied and re-verified per app.

Evidence (jink is the reference downstream):

- `jink: src/ddb_client.rs` is 768 LOC — 226 non-test (lines 1-226) + 542 test (the `#[cfg(test)]` module begins at line 227). It hand-rolls: a `DdbError` enum (line 5, `Request(#[from] reqwest::Error)`), bearer auth (line 76, `req.bearer_auth(token)`) with token read from `config.token_file` (line 42), a `graphql_code()` extractor (line 22), `typed_query`/`typed_query_with_vars` (line 146 + the `_with_vars` variant used across mutation modules), `execute_sql`/`query_sql` (lines 90/99), a partial-error policy (lines 122/172: "partial graphql errors (data present, continuing)"), `parse_connection_items` (line 180), and `parse_sql_rows` (line 201).
- jink's own architecture doc already specifies this client generically: `jink: docs/ddb-app-architecture.md:53-60` — "Create an HTTP client that wraps ddb-server's GraphQL API. It needs five capabilities" — and `:478` — "Don't write your own GraphQL introspection. It comes free via forwarding." A generic spec is the proof the need is not jink-specific.

**Pre-decided design choices (no maintainer fork left open):**
- **Error codes**: define the code-string constants in `ddb-client` and pin every one to `ddb_core::error::codes` with a workspace parity test — `ddb-core` is a **dev-dependency only** (test-time), never a normal dependency, so downstreams do not compile git2/rusqlite/automerge for a thin HTTP client; do NOT mint a new shared codes crate. (Revised 2026-07-08 from "reuse ddb-core directly": a hard ddb-core dependency taxes every downstream build, against the "consumable" end state this PRD serves.) Note: only the 10 structured-violation constants live in `ddb_core::error::codes` — the typed enum must ALSO carry `NOT_FOUND`, `VALIDATION_ERROR`, and `SQL_ERROR` from the docs "Stable error codes" list; 00173 registers `SQL_ERROR` as a first-class AppError code before this PRD runs, so re-check the codes module then.
- **Async-only**: ship the async `reqwest` client to match the downstream; no blocking variant.
- **MVP first-session cut**: error-enum + pure parsers + transport (`token`/bearer, `typed_query`, `typed_query_with_vars`, `execute_sql`, `query_sql`) + partial-error policy + `wait_ready()`, tested against a MOCK HTTP transport. This cut does NOT depend on 00188 (ephemeral-serve) — the mock is the test surface; a real-server integration test against 00188 is an optional later add, not a blocker.

### Target Users
Downstream Rust apps (jink today, others later) that embed a `ddb serve` GraphQL client and currently duplicate transport, auth, error extraction, response parsing, and readiness polling.

### Success Metrics
- A new `ddb-client` workspace crate exists (added to root `Cargo.toml` `members` — currently `["ddb-core", "ddb-cli", "ddb-server", "ddb-uniffi-bindgen", "tests"]` — and to `default-members`, currently the 3-crate subset `["ddb-core", "ddb-cli", "ddb-server"]`) exposing: HTTP transport to a running `ddb serve` GraphQL endpoint; token discovery (`~/.config/ddb/token`) + URL/env conventions; a typed error-code enum whose string values are pinned to `ddb_core::error::codes` (error.rs:103) by a dev-dependency parity test (no runtime ddb-core dependency); a variables-only request builder; response-envelope + connection-items + nested-payload + SQL-rows parsing; a partial-error policy; and `wait_ready()` with backoff.
- The crate covers everything `jink: src/ddb_client.rs` (226 non-test + 542 test LOC) hand-rolls plus the per-call-site parse glue, so jink can delete them and depend on `ddb_client::DdbClient` (the deletion itself is jink-side work, not verified in this repo).
- The crate ships its own integration tests, run against a mocked HTTP transport (MVP) or a spawned ephemeral server (00188, optional later add).

## Functional Decomposition

### Capability: Authenticated GraphQL transport
#### Feature: Token discovery + bearer auth
- **Description**: construct a client that discovers the bearer token and attaches it to every request.
- **Inputs**: an endpoint URL (arg/env, default `http://127.0.0.1:2891/graphql`) and a token file path (default `~/.config/ddb/token`) or explicit token.
- **Outputs**: a `DdbClient` that sends `Authorization: Bearer <token>`.
- **Behavior**: read the token file at construction (mirroring `jink: src/ddb_client.rs:41-52`); allow env overrides; a no-token constructor for local/test use.

#### Feature: Variables-only request builder
- **Description**: execute a GraphQL operation with a JSON variables object (no string interpolation).
- **Inputs**: a query string + a `serde_json::Value` variables map.
- **Outputs**: the parsed `data` object or a typed error.
- **Behavior**: port `typed_query`/`typed_query_with_vars`; never format values into the query text (the escaping-free contract jink adopted in its PRD 00022).

### Capability: Response parsing
#### Feature: Envelope + partial-error policy
- **Description**: interpret the `{ data, errors, extensions }` envelope with an explicit partial-error rule.
- **Inputs**: a raw GraphQL response.
- **Outputs**: `Ok(data)` when `data` is non-null (even with `errors` present, logged as partial); `Err(typed)` when `data` is null.
- **Behavior**: replicate the pinned jink policy (`jink: src/ddb_client.rs:122,172` and its test at :702, `typed_query_with_vars_partial_errors_with_data_returns_ok`).

#### Feature: Connection-items, nested-payload, and SQL-rows parsers
- **Description**: typed helpers over the common response shapes.
- **Inputs**: a `data` object.
- **Outputs**: Result-wrapped connection items, nested mutation payloads, and SQL columns/rows, with a typed parse error on malformed content.
- **Behavior**: port useful parsing structure with typed errors for malformed connection items or SQL rows; never filter_map-drop a bad item or substitute an empty row. Include the item/row location in the parse error, preserve valid empty collections, and retain DeserializeOwned support. The already-decided non-null-data-plus-errors envelope policy remains independent.

### Capability: Typed error codes
#### Feature: Error-code enum reusing ddb-core codes
- **Description**: map `extensions.code` to a typed enum.
- **Inputs**: a GraphQL error envelope.
- **Outputs**: a `DdbClientError` carrying the code + message + context.
- **Behavior**: define stable code strings locally in ddb-client and parity-test them against both ddb_core::error::codes and ddb_core::app_contract::error::codes with a dev-dependency only; no core runtime dependency.

### Capability: Readiness
#### Feature: wait_ready() with backoff
- **Description**: block until the server answers `/health`.
- **Inputs**: a timeout + backoff schedule.
- **Outputs**: `Ok(())` when healthy, `Err` on timeout.
- **Behavior**: poll `/health` (server returns `{status, version, …}`, lib.rs:239) with capped exponential backoff, matching `jink: tests/common/mod.rs:411` `poll_health`.

### Capability: Configuration conventions
#### Feature: URL + env discovery
- **Description**: resolve the endpoint URL and token from explicit args or environment, with documented precedence.
- **Inputs**: `DDB_URL`/`DDB_TOKEN` env vars (`DDB_TOKEN` is already the name the e2e conformance harness reads, `tests/e2e/conformance/setup.rs:49` — reuse it, do not mint a second spelling; generic replacements for jink's `JINK_DDB_SERVER_URL`/`JINK_DDB_TOKEN`, `jink: frontend/codegen.ts:14,22`) or explicit values.
- **Outputs**: a resolved endpoint + token source.
- **Behavior**: precedence is explicit arg > env > default (`http://127.0.0.1:2891/graphql`; token file `~/.config/ddb/token`); publish the precedence so downstreams stop re-deriving it.

## Structural Decomposition

### Repository Structure

```
ddb-client/                 # NEW workspace crate (add to root Cargo.toml members)
├── Cargo.toml              # deps: reqwest, serde_json, thiserror; dev-deps: ddb-core (codes parity test only)
└── src/
    ├── lib.rs              # Maps to: DdbClient facade + re-exports
    ├── transport.rs        # Maps to: Authenticated GraphQL transport
    ├── parse.rs            # Maps to: Response parsing (envelope, items, nested, sql rows)
    ├── error.rs            # Maps to: Typed error codes (reuses ddb_core::error::codes)
    └── readiness.rs        # Maps to: Readiness (wait_ready)
Cargo.toml                  # Maps to: add "ddb-client" to members/default-members
```

### Module: transport (ddb-client/src/transport.rs)
- **Maps to capability**: Authenticated GraphQL transport
- **Responsibility**: build/send authenticated requests; expose `typed_query` + `typed_query_with_vars` + `execute_sql`/`query_sql`.
- **Exports**: `DdbClient::{new, with_token, typed_query, typed_query_with_vars, execute_sql, query_sql}`

### Module: parse (ddb-client/src/parse.rs)
- **Maps to capability**: Response parsing
- **Responsibility**: envelope handling + `parse_connection_items` + nested-payload + `parse_sql_rows`.
- **Exports**: `parse_connection_items`, `parse_nested`, `parse_sql_rows`

### Module: error (ddb-client/src/error.rs)
- **Maps to capability**: Typed error codes
- **Responsibility**: `DdbClientError` + `graphql_code()`; string values parity-tested against `ddb_core::error::codes`.
- **Exports**: `DdbClientError`, `code()` accessor

### Module: facade, readiness, and configuration (ddb-client/src/lib.rs, transport.rs, ddb-client/Cargo.toml; Cargo.toml)
- **Maps to capability**: Authenticated GraphQL transport; Readiness; Configuration conventions
- **Responsibility**: expose the client, resolve explicit/env/token-file configuration, bound readiness polling, and register the workspace crate.
- **Exports**: DdbClient facade, wait_ready, and documented configuration inputs.

## Dependency Graph

### Foundation Layer (Phase 0)
No dependencies - built first.

- **error + parse**: pure code-mapping and response parsing; no live transport.

### Core Layer (Phase 1)
- **transport**: Depends on [error, parse] - executes authenticated requests and routes responses through the parsers.

### Integration Layer (Phase 2)
- **readiness + facade + tests + docs**: Depends on [transport].

## Implementation Phases

### Phase 0: Foundation
**Goal**: pure error-code mapping + response parsers, no network.

**Tasks**:
- [ ] Scaffold the `ddb-client` crate, add it to root `Cargo.toml` members; implement the error-code enum (string values defined in-crate) + the pure parsers (envelope, connection items, nested payload, sql rows), plus a parity test with `ddb-core` as a dev-dependency pinning every enum code string to its canonical constant in error::codes or app_contract::error::codes (no deps) - Acceptance: unit tests parse fixture envelopes and reject malformed connection items/SQL rows with typed errors rather than dropped/empty rows; valid empty results still succeed; the parity test proves every enum value matches ddb-core; `ddb-core` appears only under `[dev-dependencies]`; crate builds under `cargo build`.

**Exit Criteria**: parsers + typed codes unit-tested against fixtures.

### Phase 1: Core
**Goal**: authenticated transport + partial-error policy.

**Tasks**:
- [ ] Implement `DdbClient` transport (token discovery, bearer auth, `typed_query`/`typed_query_with_vars`, `execute_sql`/`query_sql`) routing through the Phase 0 parsers, with the data-present-continues partial-error policy (depends on: Phase 0) - Acceptance: mocked-transport tests cover happy path, partial-errors-with-data → `Ok`, and error-envelope → typed code (mirrors the jink tests).

**Exit Criteria**: transport works against a mocked server; policies pinned.

### Phase 2: Integration
**Goal**: readiness, real-server tests, docs.

**Tasks**:
- [ ] Add `wait_ready()` backoff over `/health` + the `DdbClient` facade re-exports (depends on: Phase 1) - Acceptance: `wait_ready()` returns once healthy, errors on timeout.
- [ ] Crate integration tests against the mocked transport (MVP; a spawned ephemeral server via 00188 is an optional later add, not a dependency); add `docs/src/technical/` client page + cross-ref from `docs/src/guide/building-apps.md`; CHANGELOG (depends on: Phase 1) - Acceptance: integration tests green; docs describe construction, auth, error codes, readiness. No CLI/server surface is added, so no smoke/walkthrough is required (library crate) — this is called out so the omission is deliberate, not missed.

**Exit Criteria**: workspace-delivered crate with tests + docs; registry publication is outside scope; jink can depend on it.

## Test Strategy

**Validation:** per-task local gates are `cargo build`, `cargo clippy --workspace --all-targets`, and `cargo test-ci`, including colocated unit regressions. Author required e2e/integration/conformance scenarios and executable walkthroughs; their heavy execution belongs to Tier 2 CI, not the per-task gate. Retain only CLAUDE.md's conditional e2e run for Claude-routed tasks deleting/replacing code paths. Walkthrough creation uses showboat; any verification uses `dev/bin/safe-showboat-verify`, never the project root. This backlog assumes 00171 has completed the shell-to-Rust harness migration.

### Critical Scenarios
- **Happy path**: `typed_query_with_vars` with a variables object → parsed `data`, no interpolation.
- **Edge case**: a response with `errors` but non-null `data` → `Ok(data)` (partial-error policy), pinned like `jink: src/ddb_client.rs:702`.
- **Error case**: an error envelope with `extensions.code == "UNIQUE_VIOLATION"` → the typed error-code enum variant, its string value parity-tested against `ddb_core::error::codes`.

## Risks

- **Code-string drift**: defining the strings in-crate risks silent divergence from ddb-core. Decision (revised 2026-07-08, no maintainer fork left open): the workspace parity test with `ddb-core` as a dev-dependency fails CI on any drift, while downstreams never compile ddb-core's native deps (git2, rusqlite, automerge). The earlier accept-the-hard-dependency choice was reversed because it taxed every downstream build against the "consumable" end state.
- **Error-code vocabulary drift**: `SQL_ERROR` is added by the server classifier and is not in `ddb_core::error::codes`; 00173 is unifying the two mapping paths. Mitigation: track the guide's documented "Stable error codes" list, include `SQL_ERROR`, and cross-ref 00173 so the enum converges when it lands.
- **Scope creep**: codegen and convenience layers are OUT (owned by 00189). Mitigation: keep the MVP to CRUD + search + SQL passthrough + errors + readiness.
- **Async runtime coupling**: jink's client is async `reqwest`. Mitigation: ship the async client to match downstream; a blocking variant is out of scope.

## Transport Impact

This is a client crate; Transport Impact is framed as which ddb interface the crate consumes.

- **GraphQL**: Consumes (primary) — the crate is a GraphQL client (`typed_query`/`typed_query_with_vars`, generic mutations).
- **SQL passthrough**: Consumes — `execute_sql`/`query_sql` over GraphQL `executeSql`/`sql`.
- **REST / PgWire / NoSQL HTTP**: Intentionally absent — a REST or PgWire client backend is out of MVP scope; the crate targets the flagship GraphQL surface only.
- **FFI**: Intentionally absent — FFI is in-process (`DoogatDriver`); embedded consumers need no HTTP client.
- **CLI**: Intentionally absent — the deliverable is a library, not a CLI subcommand.
- **Cross-interface conformance plan**: the crate's own integration tests are the conformance surface — against a mock transport (MVP; or the 00188 ephemeral server as an optional later add), assert `typed_query_with_vars`, `executeSql` passthrough, typed error-code extraction, the partial-error policy, and `wait_ready()`. No new server-side surface is introduced.
