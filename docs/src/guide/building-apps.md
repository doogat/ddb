# Building Apps with Doogat DB

Doogat DB works as a backend for personal productivity apps. Your data lives in Git-backed Markdown files with full version history, CRDT sync across devices, and SQL/GraphQL access for frontends.

This guide covers data modeling, API access, and two worked examples.

## When to use Doogat DB

Doogat DB fits apps where:

- **You are the sole user** — single-writer, personal data
- **Data portability matters** — your data is Markdown in Git, readable by any tool
- **Multi-device sync is needed** — laptop, phone, tablet, all conflict-free
- **Write volume is moderate** — every mutation is a git commit; aim for ~100s of writes/day, not thousands

Examples: link managers, personal CRMs, reading logs, project trackers, habit trackers, recipe collections, travel planners.

## Architecture overview

```
Frontend (React, Swift, Kotlin, etc.)
    │
    ├─ GraphQL ─── ddb serve (HTTP, port 2891)        ← Mode 1: Server
    │                  │
    │                  └── Actor thread
    │                       ├── GitRepo (storage)
    │                       ├── Index (SQLite FTS5)
    │                       └── SqlEngine (DDL/DML)
    │
    ├─ FFI ─────── DoogatDriver (UniFFI, embedded)     ← Mode 2: Embedded native
    │                  ├── GitRepo (storage)
    │                  ├── Index (SQLite FTS5)
    │                  └── SqlEngine (DDL/DML)
    │
    └─ Host Shell ─ One app, multiple feature modules  ← Mode 3: Mobile host-shell
                       └── shared DoogatDriver
                            ├── GitRepo (one repo)
                            ├── Index (one index)
                            └── SqlEngine
```

**Web/desktop apps**: talk to `ddb serve` over GraphQL.
**Single native apps**: embed `DoogatDriver` via UniFFI (Swift/Kotlin bindings) — same SQL engine, typed CRUD, transactions, and schema discovery as the server, no server process needed.
**Mobile mini-apps**: one host app embedding DoogatDriver with multiple feature modules — see [Mobile mini-apps](#mobile-mini-apps) below.
**CLI scripts**: use `ddb query` and `ddb create` directly.

## Choosing an interface

Doogat DB exposes several network and embedded interfaces. They are not equivalent. The table below tells you which one to use for each integration class.

| Integration class | Use this | Fallback | Notes |
|-------------------|----------|----------|-------|
| **Network app** (web, desktop frontend) | **GraphQL** | REST | GraphQL is the flagship network API. Every CRUD baseline capability is `Guaranteed`. Structured error codes (`extensions.code`), typed mutations, subscriptions via WebSocket. |
| **Embedded / mobile app** | **FFI (`DoogatDriver` via UniFFI)** | GraphQL over local HTTP | In-process Swift/Kotlin bindings; no server process needed. CRUD baseline `Guaranteed` within the Experimental stability envelope. Use the host-shell model for mobile (see below). |
| **CLI automation / scripting** | **CLI (`ddb` binary)** | GraphQL via `curl` + Bearer token | Shell-first: `ddb create`, `ddb query`, `ddb search`, `ddb sync`. Falls back to GraphQL when scripts need machine-readable error codes or structured warnings (CLI emits text/exit-code only). |
| **SQL / reporting** (BI tools, psql, DBeaver) | **PgWire** (port 2892) | GraphQL `executeSql` | Any PostgreSQL client works without DDB-specific code. SELECT, DML (INSERT/UPDATE/DELETE), and DDL (CREATE/ALTER/DROP TABLE) against materialized type tables. DDL triggers the hot schema reload signal — observable readiness over GraphQL may lag by up to a few seconds (poll `schemaVersion`). Errors surface as PostgreSQL messages, not `extensions.code` — use GraphQL `executeSql` when you need structured error codes alongside DML. |
| **REST CRUD/search** | **REST (`/rest/*`)** | GraphQL | Base-doogat CRUD and list/search over standard HTTP. No GraphQL library needed. Typed create/update is `Specialized` (not `Guaranteed`) until per-type REST routes land — use GraphQL when you need typed mutations. |
| **NoSQL document access** | **NoSQL HTTP (`/nosql/*`)** | REST `GET /rest/doogats/:id` | Read-only by design. O(1) document fetch and prefix scan by type or tag. All write/mutate operations are `Intentionally absent` — route writes through GraphQL or REST. |

Each row corresponds to a golden workflow defined in `dev/local/notes/downstream-golden-workflows.md` (GW-1 through GW-12). The conformance harness exercises the cross-interface CRUD baseline (GW-3, `crud_baseline` fixture) against CLI and GraphQL. Golden workflow examples for GW-1, GW-3, GW-4, GW-5, and GW-7 appear in [Golden workflow examples](#golden-workflow-examples) below.

### What "Specialized" means

`Specialized` is a narrower but still binding promise. The capability is present and supported, but with explicit constraints: a different response or error shape than the `Guaranteed` form, a workflow that differs from the primary interface, or coverage of a subset of the operations the capability spans elsewhere. The four matrix labels (`Guaranteed`, `Specialized`, `Intentionally absent`, `Deprecated`) are all real, maintainer-decided promises — `Specialized` is **not** a soft "partial" placeholder.

When a cell in the table above is `Specialized`, the note explains the specific constraint and points to the recommended alternative if you need the `Guaranteed` shape.

### Promise labels

The interface table and the CRUD baseline section use four promise labels:

- **`Guaranteed`**: present in canonical shape; conformance tests pin response, error, and field-name shape.
- **`Specialized`**: present with constraints — narrower workflow, different response/error shape, or subset of operations.
- **`Intentionally absent`**: not on this interface by design; use a different interface for this capability.
- **`Deprecated`**: reachable but has a named replacement; see [Compatibility and Deprecation](#compatibility-and-deprecation) for the migration timeline.

### Compatibility and Deprecation

Per-interface deprecation lists live in the interface docs:

- Network interfaces (GraphQL, REST, PgWire, NoSQL HTTP): [Compatibility and Deprecation](../technical/server.md#compatibility-and-deprecation) in the server docs.
- Embedded interface (FFI / `DoogatDriver`): [Compatibility and Deprecation](../technical/ffi.md#compatibility-and-deprecation) in the FFI docs.

For CLI consumers, one deprecation applies: warnings currently surface as unstructured stderr text. The candidate `cli-json-errors-v1` follow-up PRD will add a `--json-errors` / `--json-output` mode that serializes `AppWarning` entries as structured JSON. Status: planned, not yet implemented. Until then, scripted consumers parse stderr text or fall back to GraphQL for machine-readable warnings.

#### Migration Notes

One note per deprecation entry. All deprecations are Risk=low (no shim entries exist as of this writing). Notes follow the source order in `interface-deprecations.md` §2 (D-01 through D-13). For deprecations whose replacement is a candidate slug ("Status: planned, not yet implemented"), no client action is required yet; the current behavior remains supported until the named follow-up PRD ships.

##### REST search error envelope (D-01)

- **Old behavior**: REST `GET /rest/doogats?q=...` returns HTTP 4xx + `{ error, message }` JSON envelope. The `error` field carries a REST-local short-string code.
- **New behavior**: AppError envelope shipped by PRD 00147 routes REST errors through the same code vocabulary as GraphQL. The `{ error, message }` envelope shape is unchanged; the `error` field now carries the unified code (e.g. `NOT_FOUND`, `VALIDATION_ERROR`, `UNIQUE_VIOLATION`) instead of REST-local short-strings.
- **Replacement interface**: REST's `error` field, carrying the same code vocabulary GraphQL exposes under `extensions.code`. See `docs/src/technical/server.md` REST error section. (REST has no `extensions` object — that path is GraphQL-only.)
- **Required client changes**: branch on the value of the `error` field using the unified code vocabulary; the field name is unchanged. Until PRD 00149 removes the legacy short-strings, both vocabularies can appear; prefer the unified codes.
- Source: `dev/local/notes/interface-deprecations.md` §2 D-01.

##### FFI search error variant (D-02)

- **Old behavior**: FFI `DoogatDriver.search` throws `DdbError::Sql(msg)` carrying engine error text in the string payload.
- **New behavior**: planned typed FFI error variants mirroring AppError codes; not yet shipped.
- **Replacement interface**: candidate `ffi-typed-errors-v1` follow-up PRD will add per-code typed `DoogatError` enum variants.
- **Required client changes**: none yet. Wait for `ffi-typed-errors-v1` to ship; current `catch DdbError.sql` substring-matching remains supported.
- Source: `dev/local/notes/interface-deprecations.md` §2 D-02.

##### REST CRUD mutation error envelope (D-03)

- **Old behavior**: REST `POST/PUT /rest/doogats[/:id]` returns HTTP 4xx + `{ error, message }` envelope on validation failure. The `error` field carries a REST-local short-string code.
- **New behavior**: AppError envelope shipped by PRD 00147 feeds the same unified code vocabulary into REST mutations. Envelope shape is unchanged; the `error` field now carries the unified code.
- **Replacement interface**: REST's `error` field, carrying the same code vocabulary GraphQL exposes under `extensions.code`.
- **Required client changes**: branch on the value of the `error` field using the unified code vocabulary; the field name is unchanged. Until PRD 00149 removes the legacy short-strings, both vocabularies can appear; prefer the unified codes.
- Source: `dev/local/notes/interface-deprecations.md` §2 D-03.

##### REST typed create/update (D-04)

- **Old behavior**: REST `POST/PUT /rest/doogats` typed payloads silently wrote only the base-doogat shape; type-specific tables were not populated atomically.
- **New behavior**: typed-write paths shipped by PRD 00147 route REST typed create/update through the unified AppCommand, populating typed tables atomically.
- **Replacement interface**: PRD 00147 typed-write path on REST (same `POST/PUT /rest/doogats` route, now with typed-column atomicity).
- **Required client changes**: none. Same route, same payload; typed columns are now populated server-side. Legacy base-only behavior remains reachable until PRD 00149 removes it.
- Source: `dev/local/notes/interface-deprecations.md` §2 D-04.

##### FFI not-found error variant on `get` (D-05)

- **Old behavior**: FFI `DoogatDriver.get` throws `DdbError.io(msg)` with the substring `"not found"` when the id is missing.
- **New behavior**: planned typed `DdbError::NotFound { id }` variant mirroring AppError's `NOT_FOUND` code; not yet shipped.
- **Replacement interface**: candidate `ffi-typed-errors-v1` follow-up PRD will expose a typed `NotFound` variant on `DoogatError`.
- **Required client changes**: none yet. Wait for `ffi-typed-errors-v1` to ship; current `msg.contains("not found")` substring-matching remains supported.
- Source: `dev/local/notes/interface-deprecations.md` §2 D-05.

##### FFI not-found error variant on `delete` + `get` (D-06)

- **Old behavior**: post-delete FFI `DoogatDriver.get` inherits the same untyped `DdbError.io(msg)` "not found" shape as D-05.
- **New behavior**: planned typed `NotFound` variant covers the delete-then-get path identically; not yet shipped.
- **Replacement interface**: candidate `ffi-typed-errors-v1` follow-up PRD (same gate as D-05).
- **Required client changes**: none yet. Wait for `ffi-typed-errors-v1` to ship; current behavior remains supported.
- Source: `dev/local/notes/interface-deprecations.md` §2 D-06.

##### FFI invalid-type error variant on `create` (D-07)

- **Old behavior**: FFI `DoogatDriver.create` with an invalid type throws `DdbError.sql(msg)` whose message string contains `TYPE_NOT_REGISTERED`.
- **New behavior**: planned typed FFI variants for AppError validation codes (including `TYPE_NOT_REGISTERED`); not yet shipped.
- **Replacement interface**: candidate `ffi-typed-errors-v1` follow-up PRD will expose validation codes as typed `DoogatError` enum variants.
- **Required client changes**: none yet. Wait for `ffi-typed-errors-v1` to ship; parsing `msg` for `TYPE_NOT_REGISTERED` remains supported.
- Source: `dev/local/notes/interface-deprecations.md` §2 D-07.

##### REST validation error vocabulary (D-08)

- **Old behavior**: REST `POST /rest/doogats` (invalid) returns HTTP 400/422 with the `error` field carrying a short REST-local code string (and 500 on server errors).
- **New behavior**: AppError envelope shipped by PRD 00147 routes REST validation errors through the unified code vocabulary. The `error` field now carries the same codes GraphQL validation errors expose under `extensions.code`.
- **Replacement interface**: REST's `error` field, carrying the same code vocabulary GraphQL exposes under `extensions.code`.
- **Required client changes**: branch on the value of the `error` field using the unified code vocabulary; the field name is unchanged. Until PRD 00149 removes the legacy short-strings, both vocabularies can appear; prefer the unified codes.
- Source: `dev/local/notes/interface-deprecations.md` §2 D-08.

##### REST typed create/update route (D-09)

- **Old behavior**: REST `POST/PUT /rest/doogats` typed payloads routed through the base endpoint without atomic typed-column population (no per-type route shape).
- **New behavior**: typed-write paths shipped by PRD 00147 land typed columns atomically through the unified AppCommand on the same base route.
- **Replacement interface**: PRD 00147 typed-write path on REST (same base route, now with atomic typed-column population).
- **Required client changes**: none. Same route, same payload; typed columns are populated server-side. Legacy base-only handler remains reachable until PRD 00149 removes it.
- Source: `dev/local/notes/interface-deprecations.md` §2 D-09.

##### CLI warnings (D-10)

- **Old behavior**: CLI warnings emit as human-readable text on stderr; no machine-readable envelope.
- **New behavior**: planned `--json-errors` / `--json-output` mode will serialize `AppWarning` entries as structured JSON on stderr; not yet shipped.
- **Replacement interface**: candidate `cli-json-errors-v1` follow-up PRD will add the structured-JSON CLI mode.
- **Required client changes**: none yet. Wait for `cli-json-errors-v1` to ship; current text-scraping remains supported. Scripts that need structured warnings today can fall back to GraphQL.
- Source: `dev/local/notes/interface-deprecations.md` §2 D-10.

##### REST warnings (D-11)

- **Old behavior**: REST warnings surface as text embedded in the HTTP response body; no structured channel.
- **New behavior**: REST `warnings` array shipped by PRD 00147 carries structured `AppWarning` entries alongside `data`.
- **Replacement interface**: REST top-level `warnings` array on the response envelope.
- **Required client changes**: parse the structured `warnings` array on REST responses; the legacy text-in-body emission remains until PRD 00149 removes it.
- Source: `dev/local/notes/interface-deprecations.md` §2 D-11.

##### PgWire warnings (D-12)

- **Old behavior**: PgWire warnings are not surfaced over the wire protocol; no structured channel and no notice-emission path.
- **New behavior**: planned mapping of AppError codes and AppWarning entries onto PostgreSQL `ErrorResponse`/`NoticeResponse` messages; not yet shipped.
- **Replacement interface**: candidate `pgwire-structured-errors-v1` follow-up PRD (extends deferred PRD 00139 §T22 work).
- **Required client changes**: none yet. Wait for `pgwire-structured-errors-v1` to ship; PgWire consumers needing warnings today must read them through GraphQL.
- Source: `dev/local/notes/interface-deprecations.md` §2 D-12.

##### FFI warnings (D-13)

- **Old behavior**: FFI return types omit the structured warning channel (`RebuildReport` explicitly omits warnings; other types drop `AppWarning` at the UniFFI boundary).
- **New behavior**: planned structured-warnings field on FFI return types (e.g. `AppOutput<T>`-equivalent UniFFI record carrying `warnings: Vec<WarningEntry>`); not yet shipped.
- **Replacement interface**: candidate `ffi-typed-errors-v1` follow-up PRD will add the structured-warnings surface.
- **Required client changes**: none yet. Wait for `ffi-typed-errors-v1` to ship; FFI consumers needing warnings today must read them through GraphQL.
- Source: `dev/local/notes/interface-deprecations.md` §2 D-13.

### CRUD baseline

Every public application interface supports the CRUD baseline operations listed below. Labels use the [promise vocabulary](#promise-labels). Source of truth for per-interface evidence: `dev/local/notes/interface-compatibility-inventory.md`.

#### GraphQL

- **Create**: `Guaranteed` — `createDoogat` returns the created object with typed fields; `executeSql(INSERT ...)` writes the same Git-backed doogat and returns the created id in `SqlResult.message`.
- **Read (single by id)**: `Guaranteed` — `doogat(id: ...)` query returns the full object or a `NOT_FOUND` error envelope.
- **Update**: `Guaranteed` — `updateDoogat` returns the updated object; `executeSql(UPDATE ...)` applies the same write path and returns an affected-row count.
- **Delete**: `Guaranteed` — `deleteDoogat` mutation returns confirmation; cascade cleans child rows atomically.
- **List**: `Guaranteed` — typed `<type>s(limit, offset)` queries return typed rows; `executeSql(SELECT ...)` returns `SqlResult` rows.
- **Search (basics)**: `Guaranteed` — `search(query: ...)` returns `SearchConnection` hits with `id`, `title`, `snippet`, `rank`.
- **Validation error handling**: `Guaranteed` — errors return HTTP 200 with `{ errors: [{ message, extensions: { code } }] }`; codes include `VALIDATION_ERROR`, `NOT_NULL_VIOLATION`, `UNIQUE_VIOLATION`, `REFERENCES_VIOLATION`, `TYPE_NOT_REGISTERED`.
- **Not-found behavior**: `Guaranteed` — `doogat` query and mutations on missing ids return `extensions.code == "NOT_FOUND"` in the error envelope.

#### CLI

- **Create**: `Guaranteed` — `ddb create` prints the new doogat id on stdout; exits 0 on success.
- **Read (single by id)**: `Guaranteed` — `ddb read <id>` prints the raw Markdown; not-found returns non-zero exit and stderr message.
- **Update**: `Guaranteed` — `ddb update <id> ...` prints the updated id; SQL `UPDATE` through `ddb query` prints affected-row count.
- **Delete**: `Guaranteed` — `ddb delete <id>` removes the doogat; SQL `DELETE` through `ddb query` prints affected-row count.
- **List**: `Guaranteed` — `ddb query "SELECT ..."` returns tabular results on stdout.
- **Search (basics)**: `Guaranteed` — `ddb search "<query>"` returns matching doogats in tabular format.
- **Validation error handling**: `Specialized` — validation failures exit non-zero with human-readable stderr text; no machine-readable error code. Use GraphQL via `curl` when structured codes are required.
- **Not-found behavior**: `Guaranteed` — not-found on read, update, and delete returns non-zero exit and stderr message; SQL `SELECT ... WHERE id = ...` follows SQL semantics and returns zero rows.

#### FFI (`DoogatDriver` via UniFFI)

- **Create**: `Guaranteed` — `DoogatDriver.create_doogat(content, message)` returns the new doogat id; `execute_sql("INSERT ...")` returns created ids in `SqlResultRecord.message`.
- **Read (single by id)**: `Guaranteed` — `DoogatDriver.read_doogat(id)` returns raw Markdown; throws `DdbError` on missing id.
- **Update**: `Guaranteed` — `DoogatDriver.update_doogat(...)` updates the doogat; `execute_sql("UPDATE ...")` returns affected rows.
- **Delete**: `Guaranteed` — `DoogatDriver.delete_doogat(id, message)` removes the doogat; throws `DdbError` on missing id.
- **List**: `Guaranteed` — `DoogatDriver.list_doogats()` returns ids; `execute_sql("SELECT ...")` returns a `SqlResultRecord` with columns and rows.
- **Search (basics)**: `Guaranteed` — `DoogatDriver.search(query)` returns `Vec<SearchResult>` with `id`, `title`, `snippet`, `rank`.
- **Validation error handling**: `Specialized` — validation failures throw `DdbError::Validation` or `DdbError::SqlEngine` with structured code/context when available; per-code enum variants (`TYPE_NOT_REGISTERED`, `UNIQUE_VIOLATION`, etc.) are deferred to the `ffi-typed-errors-v1` follow-up PRD. Use GraphQL when structured error codes are required.
- **Not-found behavior**: `Specialized` — missing id throws `DdbError::NotFound { msg }`; an id-specific not-found payload is deferred to the `ffi-typed-errors-v1` follow-up PRD.

#### PgWire (port 2892)

- **Create**: `Guaranteed` — `INSERT INTO <type> (...)` via any PostgreSQL client populates the materialized type table and the Git-backed doogat.
- **Read (single by id)**: `Guaranteed` — `SELECT * FROM <type> WHERE id = '...'` returns the typed row.
- **Update**: `Guaranteed` — `UPDATE <type> SET ... WHERE id = '...'` modifies the doogat and updates the index.
- **Delete**: `Guaranteed` — `DELETE FROM <type> WHERE id = '...'` removes the doogat.
- **List**: `Guaranteed` — `SELECT ... FROM <type>` with standard PostgreSQL filtering and ordering returns typed rows.
- **Search (basics)**: `Specialized` — FTS5 free-text search is reachable only via raw `SELECT` against internal `_ddb_*` tables, not through a curated PgWire workflow shape. Use GraphQL `search` for the `Guaranteed` FTS5 surface.
- **Validation error handling**: `Specialized` — constraint violations return PostgreSQL error message strings (e.g. `UNIQUE_VIOLATION` text); no `extensions.code` envelope. Use GraphQL `executeSql` when structured error codes alongside DML are required.
- **Not-found behavior**: `Guaranteed` — `SELECT` on a missing id returns zero rows (standard PostgreSQL semantics); `UPDATE`/`DELETE` on a missing id returns zero affected rows.

#### REST (`/rest/*`)

- **Create**: `Guaranteed` — `POST /rest/doogats` creates a base-doogat and returns a `{ data, warnings }` JSON envelope.
- **Read (single by id)**: `Guaranteed` — `GET /rest/doogats/:id` returns the base-doogat in a `{ data, warnings }` JSON envelope.
- **Update**: `Guaranteed` — `PUT /rest/doogats/:id` updates the doogat and returns a `{ data, warnings }` JSON envelope.
- **Delete**: `Guaranteed` — `DELETE /rest/doogats/:id` removes the doogat and returns HTTP 204.
- **List**: `Guaranteed` — `GET /rest/doogats` returns `{ data: [...], pagination: {...} }`.
- **Search (basics)**: `Guaranteed` — `GET /rest/doogats?q=...` returns `{ data: [...], total_count }`.
- **Validation error handling**: `Specialized` — validation failures return HTTP 4xx + `{ error, message }` JSON envelope; the `error` field uses the unified code vocabulary, but REST has no GraphQL `extensions` object. Use GraphQL for the `Guaranteed` structured-code surface. Typed create/update is `Specialized` until per-type REST routes land — use GraphQL for typed mutations.
- **Not-found behavior**: `Guaranteed` — `GET /rest/doogats/:id` on a missing id returns HTTP 404 + `{ "error": "NOT_FOUND", "message": "..." }`.

#### NoSQL HTTP (`/nosql/*`)

- **Create**: `Intentionally absent` — NoSQL HTTP is read-only by design; route writes through GraphQL or REST.
- **Read (single by id)**: `Guaranteed` — `GET /nosql/:id` returns the raw doogat JSON from the redb index.
- **Update**: `Intentionally absent` — NoSQL HTTP is read-only by design; route writes through GraphQL or REST.
- **Delete**: `Intentionally absent` — NoSQL HTTP is read-only by design; route writes through GraphQL or REST.
- **List**: `Specialized` — `GET /nosql?type=<type>` and `GET /nosql?tag=<tag>` return prefix-scan results; no pagination or field filtering. Use GraphQL or REST for full list/filter semantics.
- **Search (basics)**: `Intentionally absent` — FTS5 free-text search is not supported on the NoSQL HTTP surface; use GraphQL `search` for full-text search.
- **Validation error handling**: `Intentionally absent` — NoSQL HTTP has no write path; validation errors do not apply. `POST`/`PUT`/`DELETE` return HTTP 405 Method Not Allowed.
- **Not-found behavior**: `Specialized` — `GET /nosql/:id` on a missing id returns HTTP 404; the body uses a provisional `{ "error": "not_found", "message": "..." }` shape that is not yet pinned by the error contract (G-13 in the inventory). For a `Guaranteed` not-found shape, use GraphQL or `GET /rest/doogats/:id`. The shape will be standardized in a follow-up documentation pass.

### Specialized and intentionally absent capabilities

The interface table and CRUD baseline list per-operation promise labels. This section explains the reasoning behind each non-flagship interface's constraints so you know whether to add a fallback or simply avoid that operation.

#### REST (`/rest/*`)

REST is the base-doogat CRUD surface over standard HTTP. No GraphQL library is needed. Use it when you need straightforward create/read/update/delete/list/search without typed mutations or structured error-code envelopes.

**Specialized:**
- *Typed create/update* - `POST/PUT /rest/doogats` accepts typed payloads but uses the base-doogat route shape. Per-type REST routes are deferred to a follow-up PRD. Use GraphQL `executeSql(INSERT INTO ...)` or typed mutations when you need typed-column guarantees in the response shape.
- *Validation error handling* - REST returns `{ error, message }` where `error` carries a unified code string, but there is no `extensions` object. Use GraphQL when your client needs to branch on structured error codes in the GraphQL envelope format.

**Intentionally absent:**
- Nothing in the CRUD baseline is intentionally absent on REST. All eight baseline operations are `Guaranteed` or `Specialized`.

#### PgWire (port 2892)

PgWire is the SQL/reporting surface. Any PostgreSQL-compatible client (psql, DBeaver, BI tools) works without DDB-specific code. Best for read-heavy reporting, ad-hoc SQL, and DDL-driven schema management.

**Specialized:**
- *FTS5 free-text search* - reachable only via `SELECT` against internal `_ddb_*` tables; no curated `search(query: ...)` workflow shape. Use GraphQL `search` for the `Guaranteed` FTS5 surface.
- *Validation error handling* - constraint violations return PostgreSQL error message strings, not `extensions.code`. Use GraphQL `executeSql` when you need structured error codes alongside DML.

**Intentionally absent:**
- *Structured warnings* - PgWire has no AppWarning channel today. Warnings are not surfaced over the wire protocol (deferred to `pgwire-structured-errors-v1`). Consumers needing warnings must read them through GraphQL.

#### NoSQL HTTP (`/nosql/*`)

NoSQL HTTP is a read-only document fetch and prefix-scan surface. Use it for O(1) by-id lookup and prefix scans by type or tag when you want low-overhead document access without building a full GraphQL query.

**Specialized:**
- *List* - `GET /nosql?type=<type>` and `GET /nosql?tag=<tag>` return prefix-scan results with no pagination or field filtering. Use GraphQL or REST for full list/filter semantics.
- *Not-found error shape* - HTTP 404 is `Guaranteed`, but the JSON body shape is provisional (`{ "error": "not_found", "message": "..." }`) and not yet pinned by the error contract (G-13 in the inventory). Use GraphQL or REST when your client needs a stable not-found body shape.

**Intentionally absent:**
- *All write operations* - Create, Update, and Delete are absent by design. `POST`, `PUT`, and `DELETE` return HTTP 405 Method Not Allowed. Route all writes through GraphQL or REST.
- *FTS5 free-text search* - not on the NoSQL HTTP surface. Use GraphQL `search`.
- *Validation error handling* - no write path; validation errors do not apply.

#### FFI (`DoogatDriver` via UniFFI)

FFI is the embedded/mobile surface. It runs in-process with no server needed and gives Swift and Kotlin apps direct access to the Git-backed repo, SQL engine, and FTS5 search. Use it for iOS/Android apps or any context where running a server process is not acceptable.

**Specialized:**
- *Validation error handling* - validation failures throw `DdbError::Validation` or `DdbError::SqlEngine` with structured code/context when available; per-code enum variants are deferred to `ffi-typed-errors-v1`. Use GraphQL when structured error codes are required.
- *Not-found behavior* - missing id on `read_doogat` and `delete_doogat` throws `DdbError::NotFound { msg }`. An id-specific not-found payload is deferred to `ffi-typed-errors-v1`.

**Intentionally absent:**
- *Real-time subscriptions* - no GraphQL subscription equivalent on the embedded surface. Mobile apps that need real-time push must compose with `ddb serve`.
- *Server auth* - the host app owns the repo in-process; no Bearer token or server auth is needed or available.
- *Ongoing remote sync* - continuous push/pull runs through CLI `ddb sync` or the GraphQL `sync` mutation. FFI's remote-sync promise is `Specialized` (bundle-shaped export/import only).

### Auth and setup

**Server-mode interfaces** (GraphQL, REST, PgWire, NoSQL HTTP) share one setup chain:

1. `ddb init` in the data directory — see [Getting started](getting-started.md).
2. `ddb serve [--port 2891] [--pg-port 2892]` — see [Server docs](../technical/server.md).
3. The server writes a UUID v4 Bearer token to `~/.config/ddb/token` on first start.
4. Pass `Authorization: Bearer <token>` on every HTTP/WebSocket request (GraphQL, REST, NoSQL HTTP).
5. For **PgWire**: connect as user `ddb` with the token as the password (MD5 password auth) — see [Server docs](../technical/server.md).

**Embedded-mode (FFI)**: standard documented setup in [FFI docs](../technical/ffi.md). Link the platform binding (XCFramework on iOS, `.aar` on Android), construct a `DoogatDriver` with the local repo path, and call `executeSql`. No auth — the host app owns the repo in-process.

**CLI**: install `ddb`, run `ddb init`. No auth required — direct repo access.

### Error and warning handling

AppError codes (from PRD 00147) are the stable cross-interface vocabulary. The code values are stable across transports; only the envelope that wraps them is transport-specific. See [Promise labels](#promise-labels) for label definitions and [CRUD baseline](#crud-baseline) for per-interface behavior.

#### Stable error codes

These codes come from two sources: the Structured violation codes in `ddb-core::error::codes` and the AppError envelope mappings in `ddb-core::app_contract::error`. All codes in this list are stable across transports.

- `VALIDATION_ERROR` — a field value fails schema validation (missing required field, wrong format).
- `NOT_NULL_VIOLATION` — a non-nullable column received a null value.
- `UNIQUE_VIOLATION` — duplicate value for a `UNIQUE` constraint.
- `REFERENCES_VIOLATION` — a foreign-key or reference constraint was violated.
- `TYPE_NOT_REGISTERED` — the requested doogat type has no registered `_typedef`.
- `UNKNOWN_FIELD` — a typed insert/update referenced a field that the typedef does not declare.
- `CASCADE_CYCLE` — a delete cascade would form a cycle in the reference graph.
- `NOT_FOUND` — no doogat exists with the given id.
- `PARSE_ERROR` — input could not be parsed (malformed JSON, invalid frontmatter, etc.).
- `INVALID_PATH` — a doogat path argument is invalid (empty, malformed, or out of scope).
- `BAD_REQUEST` — the request was structurally valid but rejected by the application contract.
- `CONFLICT` — the operation conflicts with current state (e.g., a concurrent write).
- `SINGLETON_VIOLATION` — a create was attempted for a type that allows only one instance.
- `SINGLETON_NOT_FOUND` — a singleton read was attempted but no instance exists yet.
- `INTERNAL_ERROR` — a catch-all for unexpected internal errors that do not map to a specific code above. Clients should treat this as "report and retry"; downstream code should not pattern-match on it as part of normal flow.

#### Per-interface error framing

- **GraphQL**: `errors[].extensions.code` carries the AppError code. See [CRUD baseline](#crud-baseline).
- **REST**: `{ error, message }` JSON body; `error` carries the AppError code. See [CRUD baseline](#crud-baseline).
- **PgWire**: PostgreSQL `ERROR` message string (structured per-code mapping is planned; see [PgWire warnings (D-12)](#pgwire-warnings-d-12)). See [CRUD baseline](#crud-baseline).
- **NoSQL HTTP**: `{ error, message }` JSON body; `error` carries the AppError code. See [CRUD baseline](#crud-baseline).
- **FFI**: `DdbError::*` enum variant (typed variants per code are planned; see [FFI search error variant (D-02)](#ffi-search-error-variant-d-02)). See [CRUD baseline](#crud-baseline).
- **CLI**: stderr text with a non-zero exit code. See [CRUD baseline](#crud-baseline).

#### Warnings

Structured warnings use the `AppWarning` type (PRD 00147). Two interfaces surface the `warnings` channel today:

- **GraphQL**: top-level `warnings` array on the response.
- **REST**: top-level `warnings` array in the `{ data, warnings }` response envelope (see [REST warnings (D-11)](#rest-warnings-d-11)).

The remaining interfaces defer structured warnings:

- **PgWire**: not yet surfaced over the wire protocol — see [PgWire warnings (D-12)](#pgwire-warnings-d-12). Use GraphQL in the interim.
- **FFI**: warning fields omitted at the UniFFI boundary — see [FFI warnings (D-13)](#ffi-warnings-d-13). Use GraphQL in the interim.
- **CLI**: human-readable stderr text only; no machine-readable envelope — see [CLI warnings (D-10)](#cli-warnings-d-10).

### Support diagnostics

When reporting an interface issue, include the following so the problem can be reproduced without additional back-and-forth:

- **Interface**: which interface you used (GraphQL, CLI, FFI, PgWire, REST, or NoSQL HTTP).
- **Workflow**: the operation being performed (create, read, update, delete, search, or a named golden workflow such as GW-1).
- **Command or request shape**: the exact query, mutation, CLI command, or HTTP request (redact any sensitive data).
- **Error or warning code**: the AppError code from the response (e.g., `NOT_FOUND`, `VALIDATION_ERROR`). See [Error and warning handling](#error-and-warning-handling) for the stable code vocabulary.
- **Version**: output of `ddb --version`.
- **Minimal reproduction**: the smallest set of steps that triggers the issue.

This set of fields is sufficient to identify which interface path failed and whether the behavior is a regression in a `Guaranteed` capability, a `Specialized` behavior, or a known limitation. Reports lacking an interface and error code typically require at least one round of clarification before investigation can begin.

## Golden workflow examples

These examples cover the five primary golden workflows (GW-1, GW-3, GW-4, GW-5, GW-7). The CLI and GraphQL examples (GW-1, GW-3) are pinned by the `crud_baseline` conformance harness. The PgWire (GW-4), FFI (GW-5), and REST (GW-7) examples match the documented contract; harness coverage for those interfaces is tracked under PRD 00148. All examples use only documented API surfaces — no hidden project-specific adapters.

See [Choosing an interface](#choosing-an-interface) for the full promise matrix and [Error and warning handling](#error-and-warning-handling) for how each interface surfaces errors.

### GW-1: GraphQL typed create/update

**Interface:** GraphQL — all CRUD baseline operations `Guaranteed`.

Assumes a `project` typedef exists (`ddb type install project` or via `CREATE TABLE`).

**Typed create** (use `executeSql` for explicit column control):

```graphql
mutation {
  executeSql(sql: "INSERT INTO project (title, status, priority) VALUES ('Q3 Planning', 'active', 'high')") {
    message
  }
}
```

Or via the generic mutation (returns typed fields on the response):

```graphql
mutation {
  createDoogat(input: { title: "Q3 Planning", type: "project" }) {
    id
    title
    type
  }
}
```

**Typed update** (patch — unspecified columns are unchanged):

```graphql
mutation {
  executeSql(sql: "UPDATE project SET status = 'done' WHERE id = '20260301130000'") {
    message
  }
}
```

**Delete:**

```graphql
mutation {
  deleteDoogat(id: "20260301130000")
}
```

**Validation error response shape** (all GraphQL errors follow this envelope):

```json
{
  "data": null,
  "errors": [{
    "message": "UNIQUE_VIOLATION: title already exists for type project",
    "extensions": {
      "code": "UNIQUE_VIOLATION",
      "context": { "field": "title", "existing_id": "20260101120000" }
    }
  }]
}
```

Error codes returned on the GraphQL surface: `VALIDATION_ERROR`, `NOT_NULL_VIOLATION`, `UNIQUE_VIOLATION`, `REFERENCES_VIOLATION`, `TYPE_NOT_REGISTERED`, `SINGLETON_VIOLATION`. Read-side not-found returns `extensions.code == "NOT_FOUND"`.

### GW-3: Cross-interface CRUD baseline

**Interfaces:** GraphQL, CLI, FFI, PgWire, REST — all five independently support the full CRUD baseline. NoSQL HTTP is excluded from this workflow (write operations intentionally absent).

This is the conformance baseline: the same doogat lifecycle (create / read / update / delete / list / search) verified on each interface. The conformance harness (`crud_baseline` fixture) exercises CLI and GraphQL. Use GW-1, GW-4, GW-5, GW-7 for interface-specific examples.

**Canonical create across interfaces** (all create a base doogat titled "Baseline test"):

```bash
# CLI
ddb create --title "Baseline test" --type note
```

```graphql
# GraphQL
mutation { createDoogat(input: { title: "Baseline test", type: "note" }) { id } }
```

```sql
-- PgWire (psql / any PostgreSQL client)
INSERT INTO note (title) VALUES ('Baseline test');
```

```http
# REST
POST /rest/doogats
Authorization: Bearer <token>
Content-Type: application/json

{ "title": "Baseline test", "type": "note" }
```

```swift
// FFI (Swift)
let id = try driver.createDoogat(content: "---\ntitle: Baseline test\ntype: note\n---\n", message: "create")
```

**Not-found behavior differs by interface** — see the [CRUD baseline table](#crud-baseline) for the exact response shape on each. GraphQL returns `extensions.code == "NOT_FOUND"`; CLI exits non-zero; PgWire returns zero rows; REST returns HTTP 404; FFI throws `DdbError::NotFound`.

### GW-4: PgWire SQL/reporting with DDL and schema-reload

**Interface:** PgWire (port 2892) — SQL/reporting surface. Any PostgreSQL-compatible client works without DDB-specific code.

**Connect** using MD5 password auth (username `ddb`, password = Bearer token from `~/.config/ddb/token`):

```bash
psql "host=127.0.0.1 port=2892 user=ddb password=$(cat ~/.config/ddb/token) dbname=ddb"
```

**Define a type via DDL:**

```sql
CREATE TABLE report (title TEXT NOT NULL, status TEXT, period TEXT);
```

DDL triggers a hot schema reload. The reload is asynchronous — a subsequent `SELECT * FROM report` over PgWire works immediately, but a GraphQL request against the new type may lag by up to a few seconds. Wait for `SELECT * FROM pg_class WHERE relname = 'report'` to return a row before issuing GraphQL queries against the new type, or poll `query { schemaVersion }` until it advances.

**DML (SELECT, INSERT, UPDATE, DELETE):**

```sql
-- Insert a typed row
INSERT INTO report (title, status, period) VALUES ('Q3 Summary', 'draft', '2026-Q3');

-- Query with filter and ordering (typed columns, not TEXT blobs)
SELECT title, status, period FROM report WHERE status = 'draft' ORDER BY period;

-- Update in place
UPDATE report SET status = 'final' WHERE title = 'Q3 Summary';

-- Delete
DELETE FROM report WHERE status = 'final';
```

**Error shape:** Constraint violations return PostgreSQL error message strings (e.g. `ERROR: UNIQUE_VIOLATION`), not the GraphQL `extensions.code` envelope. Use GraphQL `executeSql` when structured error codes alongside DML are required — same engine, different transport.

### GW-5: FFI embedded CRUD/search with typed errors

**Interface:** FFI (`DoogatDriver` via UniFFI) — CRUD baseline `Guaranteed` within the Experimental stability envelope. No server process required.

**macOS note:** After linking `libddb_core.dylib`, codesign it before loading or `syspolicyd` will silently kill the import: `codesign -f -s - path/to/libddb_core.dylib`.

**Construct** the driver (initializes the repo if empty — no manual `ddb init`):

```swift
// Swift
let driver = try DoogatDriver(repoPath: "/path/to/repo")
```

**Typed create and read** via SQL passthrough:

```swift
// Create a typed doogat
let result = try driver.executeSql(sql: "INSERT INTO project (title, status) VALUES ('My Project', 'active')")
// result.message contains the created doogat id

// Read back
let id = "20260301130000"
let markdown = try driver.readDoogat(id: id)   // raw Markdown or throws DdbError.notFound

// Search
let hits = try driver.search(query: "My Project")
// hits: [SearchResult] with .id, .title, .snippet, .rank
```

**Delete:**

```swift
try driver.deleteDoogat(id: id, message: "remove project")
```

**Error handling:**

```swift
do {
    let markdown = try driver.readDoogat(id: "nonexistent")
} catch DdbError.notFound(let msg) {
    // id not in repo
} catch DdbError.validation(let msg) {
    // constraint violation — code embedded in msg (e.g. "UNIQUE_VIOLATION: ...")
    // per-code typed variants deferred to ffi-typed-errors-v1
} catch DdbError.sql(let msg) {
    // SQL-engine error
}
```

**Kotlin** uses the same API shape with `throws` replaced by `try/catch` on `DdbException` subclasses.

### GW-7: REST CRUD/search

**Interface:** REST (`/rest/*`) — base-doogat CRUD `Guaranteed`; typed create/update `Specialized` (no per-type routes yet).

All requests require `Authorization: Bearer <token>` (from `~/.config/ddb/token`).

**Create:**

```http
POST /rest/doogats
Content-Type: application/json
Authorization: Bearer <token>

{ "title": "Meeting notes", "type": "note", "tags": ["work"] }
```

Response: `{ "data": { "id": "20260301130000", "title": "Meeting notes", ... }, "warnings": [] }`

Typed fields are not populated in the REST create path. Use GraphQL `executeSql(INSERT INTO ...)` when you need typed-column values in the response.

**Read, update, delete:**

```http
GET    /rest/doogats/20260301130000
PUT    /rest/doogats/20260301130000   body: { "title": "Updated notes" }
DELETE /rest/doogats/20260301130000
```

DELETE returns HTTP 204. Not-found returns HTTP 404 + `{ "error": "NOT_FOUND", "message": "..." }`.

**List and search:**

```http
GET /rest/doogats?type=note&q=meeting&tag=work
```

Response: `{ "data": [...], "pagination": { "total": 12, "limit": 20, "offset": 0 } }`

**Error shape:** HTTP 4xx/5xx + JSON `{ "error": "<code>", "message": "<detail>" }`. The `error` field uses the unified code vocabulary but there is no `extensions` envelope — use GraphQL for the `Guaranteed` structured-error surface.

## Data modeling

> **Always use `CREATE TABLE` via `ddb query` to define types.** Do not create `_typedef` doogats manually - manual creation bypasses CRDT tracking and may cause sync conflicts across devices.

### Entities become tables

Each entity in your app maps to a SQL table, which maps to a `_typedef` doogat, which auto-generates a GraphQL type.

```
SQL table ←→ _typedef doogat ←→ GraphQL type ←→ Markdown files
```

Define schemas with SQL:

```sql
CREATE TABLE bookmark (
  title TEXT NOT NULL,
  url TEXT NOT NULL,
  category TEXT REFERENCES category(id)
);
```

This single statement:
1. Creates a `_typedef` doogat at `ddb/_typedef/{id}.md`
2. Creates a materialized SQLite table for queries
3. Generates a `Bookmark` GraphQL type with a `bookmarks()` query

### Zone mapping

Each column maps to a zone in the doogat Markdown file:

| Zone | Stored as | Best for |
|------|-----------|----------|
| `frontmatter` | YAML field | Scalars: numbers, booleans, dates, short strings |
| `body` | `## Heading` section | Long-form text, notes, descriptions |
| `reference` | `- key:: value` line | Links between entities (FK references, wikilinks) |

**Zone assignment rules** (in priority order):

1. Explicit `zone` in the typedef always wins
2. Otherwise, the SQL type determines the default:

| SQL Type | Default Zone |
|----------|-------------|
| `REFERENCES` column | reference |
| `CHAR`, `CHAR(n)`, `VARCHAR(n≤255)`, `VARCHAR` (no size), `TINYTEXT` | frontmatter |
| `INTEGER`, `REAL`, `BOOLEAN` | frontmatter |
| `ENUM(...)`, `SET(...)` | frontmatter |
| Column with `allowed_values` | frontmatter |
| `VARCHAR(n>255)`, `TEXT`, `MEDIUMTEXT`, `LONGTEXT` | body |
| Everything else | body |

Rule of thumb: if it points somewhere else, it's a reference. If it describes the doogat, it's frontmatter. If it IS the doogat, it's body.

### Relationships

Foreign keys use `REFERENCES`:

```sql
CREATE TABLE category (
  name TEXT NOT NULL,
  panel TEXT REFERENCES panel(id)
);
```

This stores the FK as a wikilink in the reference section:

```markdown
---
- panel:: [[20260301120000]]
```

The SQL engine validates FK targets on INSERT. Backlinks are automatically indexed.

### Constraints

Use SQL `ENUM` and `SET` types for value constraints:

```sql
CREATE TABLE task (
  title TEXT NOT NULL,
  status ENUM('todo', 'doing', 'done') DEFAULT 'todo',
  priority ENUM('low', 'medium', 'high')
);
```

The engine translates `ENUM`/`SET` into `allowed_values` in the typedef YAML (stored as `TEXT` with constraints):

```yaml
columns:
  - name: status
    data_type: TEXT
    zone: frontmatter
    allowed_values: [todo, doing, done]
    default_value: todo
  - name: priority
    data_type: TEXT
    zone: frontmatter
    allowed_values: [low, medium, high]
```

`allowed_values` is enforced on INSERT - invalid values are rejected. `DEFAULT` fills missing columns.

You can also add constraints to existing tables:

```sql
ALTER TABLE task ADD COLUMN tags SET('urgent', 'blocked', 'review');
```

### Changing a column's type

When a declared type becomes too narrow (for example, `VARCHAR(255)` for URLs
that sometimes exceed the cap), migrate the column with
`ALTER TABLE ... ALTER COLUMN ... TYPE`:

```sql
ALTER TABLE link ALTER COLUMN url TYPE TEXT;
ALTER TABLE link ALTER COLUMN url TYPE VARCHAR(2048);
ALTER TABLE numeric ALTER COLUMN score TYPE REAL;
```

Supported conversions:

- **Widening `VARCHAR(N)` → `VARCHAR(M)` where `M ≥ N`**: metadata-only, no
  data scan. The same applies to `CHAR(N) → CHAR(M)` widening.
- **`VARCHAR(N)` / `CHAR(N)` → `TEXT`**: metadata-only, no data scan. Use this
  when the length cap is the problem.
- **Narrowing `VARCHAR(N)` → `VARCHAR(M)` where `M < N`, or `TEXT → VARCHAR`,
  or `CHAR(N) → CHAR(M)` where `M < N`**: runs a pre-flight scan. If any
  existing row exceeds the new limit, the statement fails with
  `cannot narrow <table>.<column> to <new_type>: <n> existing rows exceed
  limit`. Widen the problem rows or DELETE them first.
- **`INTEGER` ↔ `REAL`**: scans every existing value. Fractional values fail
  when narrowing to `INTEGER`; non-numeric values are also rejected.

`CHAR` and `VARCHAR` are different families (CHAR is fixed-width with padding
semantics) and cross-family conversions are rejected. Migrate via a temporary
column when you need to change family.

`REFERENCES` columns only accept widening within the same family or to `TEXT`.
Other type changes are rejected to keep the foreign-key target stable.

The `SET DATA TYPE` form is also accepted (`ALTER COLUMN url SET DATA TYPE
TEXT`). Both forms are identical in effect.

Out of scope for v1: `BOOLEAN` conversions, cross-category conversions needing
data rewrites (e.g. `TEXT → INTEGER` where some strings are non-numeric), and
changing `NOT NULL`/`DEFAULT`/`REFERENCES` alongside the type. For those,
migrate via a temporary column + `UPDATE` + `DROP COLUMN`.

### Body sections for rich content

Use `template_sections` to define expected body headings. Note: `template_sections` must be set by editing the typedef YAML directly - there is no SQL DDL syntax for this yet.

```yaml
template_sections:
  - Description
  - Notes
```

A doogat of this type will have:

```markdown
---
id: 20260301120000
title: My Record
type: task
status: todo
---

## Description

Task description here.

## Notes

Additional notes.

---
- assignee:: [[20260101000000]]
```

Body sections are stored as `TEXT` columns in the body zone, queryable via SQL and exposed in GraphQL.

### Title resolution

By default, a doogat's title comes from the `title` frontmatter field. For typed doogats, you can set a **title template** that auto-generates titles from column values:

```sql
ALTER TABLE contact SET TITLE TEMPLATE '{name} ({relationship})';
```

Template syntax: `{column_name}` placeholders are interpolated from the row's column values. Unfilled placeholders (missing values) are stripped automatically.

**Dereferencing REFERENCES columns.** When a column is declared `REFERENCES <target_type>`, use the dotted form `{column.field}` to reach through the reference and pull `field` off the target doogat. `field` can be the target's `title` or any typed column on the target's typedef.

```sql
CREATE TABLE link (url TEXT);
CREATE TABLE category (fqn TEXT);
CREATE TABLE "category-membership" (
  link TEXT REFERENCES link,
  category TEXT REFERENCES category
);
ALTER TABLE "category-membership"
  SET TITLE TEMPLATE '{link.title} in {category.fqn}';
```

Inserting a membership composes the title from the referenced doogats:

```sql
INSERT INTO "category-membership" (link, category)
VALUES ('20260101000000', '20260102000000');
-- title becomes "My Link in Work/Jink"
```

Rules:

- Only one hop is supported. `{a.b.c}` is rejected at typedef materialization.
- Bare `{col}` on a REFERENCES column keeps its existing behavior and substitutes the raw id.
- Typedefs with a bad dotted path (column not found, column not REFERENCES, field missing on target) are rejected when the template is applied.
- At runtime, a missing target row or NULL target field substitutes the empty string. The INSERT still succeeds.
- Title is recomputed on `UPDATE` when the SET list touches any column referenced by the template. Cascading re-title when the **target** doogat's field changes is out of scope; stale junction titles must be fixed via `ddb fix` or a follow-up `UPDATE`.

Remove a template:

```sql
ALTER TABLE contact DROP TITLE TEMPLATE;
```

`ddb fix` detects doogats whose titles don't match their type's template and offers to correct them.

> **Breaking change (unreleased):** the silent title fallback (url/description) has been removed. If `title` is `NOT NULL` and no `title_template` is set, an INSERT without an explicit `title` is rejected. Choose one: provide explicit titles, declare a `title_template`, or make `title` nullable.

### Zone overrides

Override the default zone for any column:

```sql
ALTER TABLE note SET ZONE body FOR summary;
```

This moves `summary` from its inferred default zone into the body zone. Available zones: `frontmatter`, `body`, `reference`.

After changing a zone, existing doogats need migration to move data to the new zone:

```bash
ddb fix --migrate
```

### Multi-valued references

When a `CREATE TABLE` includes a `REFERENCES` column, the engine auto-creates a **junction table** named `{type}_{column}` for storing multiple references:

```sql
CREATE TABLE bookmark (
  title TEXT NOT NULL,
  url TEXT NOT NULL,
  category TEXT REFERENCES category(id)
);
-- Auto-creates junction table: bookmark_category
```

**Insert** a reference:

```sql
INSERT INTO bookmark_category (bookmark_id, category_id)
VALUES ('20260301120200', '20260301120100');
```

This appends a `- category:: [[20260301120100]]` line to the bookmark's reference section.

**Delete** a reference:

```sql
DELETE FROM bookmark_category
WHERE bookmark_id = '20260301120200' AND category_id = '20260301120100';
```

**Query** references:

```sql
-- Display view (comma-separated IDs)
SELECT id, title, category FROM bookmark;

-- Relational query (JOIN for filtering)
SELECT b.title, c.name
FROM bookmark b
JOIN bookmark_category bc ON bc.bookmark_id = b.id
JOIN category c ON c.id = bc.category_id;
```

Dropping a table cascades to its junction tables.

### Required foreign keys (RESTRICT)

A column declared `NOT NULL REFERENCES other(id)` blocks the parent's delete: any row currently pointing at the parent will keep it alive. The error names the blocking table, column, and child row id, so client code can resolve the dependency before retrying:

```sql
CREATE TABLE link (url TEXT NOT NULL);
CREATE TABLE "category-membership" (
  link_id     VARCHAR(255) NOT NULL REFERENCES link(id),
  category_id VARCHAR(255) NOT NULL REFERENCES category(id),
  UNIQUE(link_id, category_id)
);

-- Fails: "cannot delete '<link-id>': NOT NULL REFERENCES from
--         category-membership.link_id in row '<membership-id>'"
DELETE FROM link WHERE id = '<link-id>';

-- Works: remove the membership first, then the link.
DELETE FROM "category-membership" WHERE link_id = '<link-id>';
DELETE FROM link WHERE id = '<link-id>';
```

Nullable `REFERENCES` columns keep the existing cascade (the wikilink is stripped and the parent is deleted). Use `NOT NULL REFERENCES` only when a missing parent should be treated as schema corruption.

## API access

### GraphQL

Start the server:

```bash
ddb serve                    # localhost:2891
ddb serve --playground       # enables GraphQL Playground at GET /graphql
```

Authenticate with the bearer token (auto-generated at `~/.config/ddb/token`):

```bash
curl -H "Authorization: Bearer $(cat ~/.config/ddb/token)" \
     -H "Content-Type: application/json" \
     -d '{"query": "{ bookmarks { id, title, url } }"}' \
     http://localhost:2891/graphql
```

#### Auto-generated queries

For each type, the server generates a typed query:

```graphql
# From CREATE TABLE bookmark (...)
query {
  bookmarks(tag: String, limit: Int, offset: Int): [Bookmark!]!
}

type Bookmark {
  id: ID!
  title: String!
  body: String!
  tags: [String!]!
  # ... typed fields from columns
  bookmarkTitle: String    # frontmatter TEXT
  url: String              # frontmatter TEXT
  category: Category       # singular: resolved referenced object (nullable)
  categories: [Category!]! # plural: all referenced objects
}
```

#### Mutations

Use the generic mutations or SQL passthrough:

```graphql
mutation {
  # Generic doogat creation
  createDoogat(input: { title: "My Link", type: "bookmark", tags: ["dev"] }) {
    id
  }

  # SQL for typed inserts (richer column control)
  executeSql(sql: "INSERT INTO bookmark (title, url, category) VALUES ('Rust Book', 'https://doc.rust-lang.org/book/', '20260301120000')") {
    message
  }
}
```

#### Complex queries via SQL passthrough

```graphql
query {
  sql(query: "SELECT c.name, COUNT(b.id) as count FROM category c LEFT JOIN bookmark b ON b.category = c.id GROUP BY c.id ORDER BY count DESC") {
    columns
    rows
  }
}
```

`columns` returns the column names as a string array (e.g. `["name", "count"]`). `rows` returns each row as a JSON string.

#### Core fields in type tables

Materialized type tables include `title`, `date`, and `updated_at` columns automatically, so queries like `SELECT title, url FROM bookmark` work without joining the `doogats` table.

#### Boolean columns

Boolean columns are stored as `1`/`0` integers. Use `WHERE pinned = 1` (not `WHERE pinned = 'true'`). If upgrading from a previous version, run `ddb reindex` to convert existing `"true"`/`"false"` strings.

#### Distinct values

Deduplicate typed query results by a column. Useful for populating dropdowns:

```graphql
query {
  categories(distinct: "space") {
    items { space }
    totalCount          # reflects deduplicated count
  }
}
```

Combine with `where` to filter before deduplication:

```graphql
query {
  bookmarks(distinct: "category", where: { pinned: { eq: 1 } }) {
    items { category { name } }
    totalCount
  }
}
```

#### Grouped aggregates

Get per-group counts and numeric aggregates with `groupBy`:

```graphql
query {
  bookmarksAggregate(groupBy: "status") {
    groups {
      key         # the group value (e.g. "active", "archived")
      count
      minPriority
      maxPriority
    }
  }
}
```

Without `groupBy`, the aggregate query returns a single row as before:

```graphql
query {
  bookmarksAggregate { count }   # total count, no grouping
}
```

#### Batch mutations (atomic)

Execute multiple SQL statements in one call. **All DML statements run in an implicit transaction** - if any statement fails, every preceding statement is rolled back. No partial state.

```graphql
mutation {
  executeBatch(statements: [
    "INSERT INTO bookmark (title, url) VALUES ('Link 1', 'https://one.com')",
    "INSERT INTO bookmark (title, url) VALUES ('Link 2', 'https://two.com')"
  ]) {
    message
    affected
  }
}
```

Transaction rules:
- **DML** (INSERT, UPDATE, DELETE): wrapped in implicit BEGIN/COMMIT. Failure at any point rolls back all prior statements.
- **DDL** (CREATE/DROP/ALTER TABLE): commits to git immediately and is not covered by the implicit transaction. DDL triggers a schema reload.
- **Explicit transactions**: if your batch includes `BEGIN`/`COMMIT`, the implicit transaction is skipped and you manage it yourself.

The same atomicity applies to multi-statement strings passed to `ddb query` and `DoogatDriver.executeSql()` in embedded mode.

### CLI

```bash
# Define schema
ddb query "CREATE TABLE bookmark (title TEXT NOT NULL, url TEXT NOT NULL)"

# Insert data
ddb query "INSERT INTO bookmark (title, url) VALUES ('Rust Book', 'https://doc.rust-lang.org/book/')"

# Query
ddb query "SELECT id, title, url FROM bookmark"

# Full-text search across all doogats
ddb search "rust programming"
```

### UniFFI (mobile)

Embed Doogat DB directly in Swift or Kotlin. The embedded API delegates to the same `SqlEngine` as `ddb serve` — DDL creates typedef doogats via Git, DML reads/writes Git-backed doogats, and SELECT returns typed rows.

```swift
let driver = try DoogatDriver.createRepo(repoPath: "/path/to/ddb")

// Schema — same DDL as server
try driver.executeSql("CREATE TABLE contact (name TEXT, email TEXT)")

// Insert — returns created doogat IDs
let ins = try driver.executeSql(
    "INSERT INTO contact (name, email) VALUES ('Alice', 'alice@example.com')"
)

// Query — returns SqlResultRecord with columns + rows
let contacts = try driver.executeSql("SELECT name, email FROM contact")
for row in contacts.rows {
    print("\(row[0]): \(row[1])")
}

// Transactions — buffer writes, commit as single Git commit
try driver.beginTransaction()
try driver.executeSql("INSERT INTO contact (name, email) VALUES ('Bob', 'bob@example.com')")
try driver.executeSql("UPDATE contact SET email = 'alice@new.com' WHERE name = 'Alice'")
try driver.commitTransaction()

// Type discovery — bootstrap app screens from schema metadata
let schemas = try driver.listTypeSchemas()
for schema in schemas {
    print("\(schema.tableName): \(schema.columns.map { $0.name })")
}
```

No server process needed. The app owns the git repo directly. See [FFI docs](../technical/ffi.md) for the full API surface.

## Mobile mini-apps

### Why not separate apps?

Mobile platforms do not support multiple independently installed apps sharing one local backend:

- **iOS**: apps are sandboxed; no shared filesystem, no `localhost` IPC between apps, background processes are killed aggressively
- **Android**: apps have private storage; `localhost` servers are killed by Doze mode and app standby; cross-app IPC requires explicit permissions and trust

Running `ddb serve` on a phone and connecting multiple installed apps to it is not portable and not supported.

### The host-shell model

The recommended mobile architecture is one installed app containing:

- One embedded Doogat DB core (`DoogatDriver` via UniFFI)
- One shared repository and index
- Multiple feature modules that feel like mini-apps
- Optional widgets and extensions bound to the same shared data

Users get the UX of several mini-apps. The OS sees one well-behaved app.

### iOS shape

- One main app target with SwiftUI
- Feature modules as Swift packages or local frameworks
- Optional widgets and extensions (WidgetKit, Share Extension)
- App Group storage for shared repo/index when extensions need access
- UniFFI-generated Swift bindings imported by the app and extensions

### Android shape

- One main application package with Jetpack Compose
- Feature modules as Gradle modules (`:feature-bookmarks`, `:feature-contacts`, etc.)
- Optional widgets (AppWidgetProvider) and services
- App-private storage, shared across modules within the same process
- UniFFI-generated Kotlin bindings inside the app

### Mini-app contract

Each feature module contributes:

- **Schema**: table definitions via `CREATE TABLE` (applied at app startup)
- **Queries/mutations**: SQL or typed CRUD calls through the shared `DoogatDriver`
- **UI**: screens, navigation destinations, local view state
- **Optional surfaces**: dashboard widgets, share extensions, shortcuts

Each module does **not** own:

- Its own storage engine or repo copy
- Its own local backend daemon
- Its own incompatible backend semantics

### Shared schema bootstrap

On app launch, the host shell initializes `DoogatDriver` once, then each module registers its tables:

```swift
// iOS example
let driver = try DoogatDriver.createRepo(repoPath: appGroupRepoPath)

// Each module bootstraps its schema (idempotent)
try driver.executeSql(sql: "CREATE TABLE IF NOT EXISTS category (name TEXT NOT NULL)")
try driver.executeSql(sql: "CREATE TABLE IF NOT EXISTS bookmark (title TEXT NOT NULL, url TEXT NOT NULL, category TEXT REFERENCES category(id))")
try driver.executeSql(sql: "CREATE TABLE IF NOT EXISTS contact (name TEXT NOT NULL, email TEXT)")
```

```kotlin
// Android example
val driver = DoogatDriver.createRepo(repoPath = appPrivateRepoPath)

// Each module bootstraps its schema (idempotent)
driver.executeSql("CREATE TABLE IF NOT EXISTS category (name TEXT NOT NULL)")
driver.executeSql("CREATE TABLE IF NOT EXISTS bookmark (title TEXT NOT NULL, url TEXT NOT NULL, category TEXT REFERENCES category(id))")
driver.executeSql("CREATE TABLE IF NOT EXISTS contact (name TEXT NOT NULL, email TEXT)")
```

`CREATE TABLE IF NOT EXISTS` is idempotent — if the table already exists, it's a no-op.

### Relationship to embedded parity

The host-shell model relies on the embedded API covering the workflows host modules need — but `DoogatDriver` and `ddb serve` are **not** unqualified equivalents. The shared core (`ddb-core`) gives both surfaces the same Git storage, types, sync semantics, SQL engine, and FTS5 search. What differs is at the surface boundary:

- CRUD, typed SQL (DDL/DML/SELECT), FTS5 search, type discovery, transactions, and local maintenance (reindex, compact) are `Guaranteed` on both — within the FFI Experimental stability envelope on the embedded side.
- Bundle export/import is exposed on FFI but the canonical workflow shape is CLI-primary (`ddb bundle export/import`); FFI is `Specialized`.
- Attachments are `Specialized` on FFI because the Attachments feature itself is `Experimental` per the stability tier table.
- Ongoing Git remote sync (push/pull/fetch) is **not** exposed on `DoogatDriver` — only bundle export/import is. Continuous remote sync runs through CLI `ddb sync` or the GraphQL `sync` mutation; FFI's remote-sync promise is `Specialized` (bundle-shaped).
- Real-time subscriptions and Auth are `Intentionally absent` on FFI — there is no GraphQL-subscription equivalent on the embedded surface, and the host app owns the repo (no server auth needed).

See the [FFI Promise Boundaries table](../technical/ffi.md#promise-boundaries) for the canonical per-capability promise on `DoogatDriver`. Host modules that need real-time push, remote sync orchestration, or server auth must compose with `ddb serve` directly.

## Worked example: link dashboard

A personal link dashboard with panels, categories, and bookmarks.

### Schema

```sql
CREATE TABLE panel (
  name TEXT NOT NULL,
  sort_order INTEGER DEFAULT 0
);

CREATE TABLE category (
  name TEXT NOT NULL,
  panel TEXT REFERENCES panel(id)
);

CREATE TABLE bookmark (
  title TEXT NOT NULL,
  url VARCHAR(255) NOT NULL,
  description TEXT,
  status ENUM('active', 'archived') DEFAULT 'active',
  category TEXT REFERENCES category(id)
);
```

Zone assignments: `url` is `VARCHAR(255)` (≤255, frontmatter). `description` is `TEXT` (body). `status` is `ENUM` (frontmatter). `category` has `REFERENCES` (reference zone).

### Sample data

```sql
INSERT INTO panel (name, sort_order) VALUES ('Development', 0);
INSERT INTO panel (name, sort_order) VALUES ('Research', 1);

-- Assume panel IDs are 20260301120000 and 20260301120001
INSERT INTO category (name, panel) VALUES ('Rust', '20260301120000');
INSERT INTO category (name, panel) VALUES ('AI/ML', '20260301120001');

-- Assume category IDs are 20260301120100 and 20260301120101
INSERT INTO bookmark (title, url, category) VALUES ('Rust Book', 'https://doc.rust-lang.org/book/', '20260301120100');
INSERT INTO bookmark (title, url, category) VALUES ('Tokio Tutorial', 'https://tokio.rs/tokio/tutorial', '20260301120100');
```

### Frontend queries

```graphql
# Load all bookmarks with resolved category objects
query {
  bookmarks {
    items {
      id, title, url
      category { id, name }       # singular: resolved object
      categories { id, name }     # plural: list of resolved objects
    }
    totalCount
  }
}

# Search across all bookmarks
query {
  search(query: "rust async") {
    totalCount
    hits { id, title, snippet, rank }
  }
}

# Search with filters: only bookmarks tagged "rust"
query {
  search(query: "async", types: ["bookmark"], tag: "rust") {
    totalCount
    hits { id, title, snippet }
  }
}

# Search with field filter: only active bookmarks
query {
  search(query: "async", where: [{ field: "status", eq: "active" }]) {
    totalCount
    hits { id, title }
  }
}

# Add a bookmark
mutation {
  executeSql(sql: "INSERT INTO bookmark (title, url, category) VALUES ('Serde Docs', 'https://serde.rs', '20260301120100')") {
    message
  }
}
```

### What each bookmark looks like on disk

```markdown
---
id: 20260301120200
title: Rust Book
type: bookmark
date: 2026-03-01
url: https://doc.rust-lang.org/book/
status: active
---

A comprehensive guide to the Rust programming language.

---
- category:: [[20260301120100]]
```

Three zones visible: frontmatter (url, status), body (description - editable in any text editor after creation), references (category wikilink). Editable in any Markdown editor or Obsidian.

## Worked example: personal CRM

Track contacts, life events, and interactions.

### Schema

```sql
CREATE TABLE contact (
  name VARCHAR(255) NOT NULL,
  relationship ENUM('family', 'friend', 'colleague', 'business', 'acquaintance'),
  email VARCHAR(255),
  phone VARCHAR(100)
);

CREATE TABLE life_event (
  event_type ENUM('birthday', 'married', 'graduated', 'moved', 'other') NOT NULL,
  event_date VARCHAR(10),
  contact TEXT REFERENCES contact(id)
);

CREATE TABLE interaction (
  interaction_date VARCHAR(10) NOT NULL,
  location VARCHAR(255),
  contact TEXT REFERENCES contact(id)
);
```

Body section headings are defined in the typedef YAML (no SQL DDL for this yet):

```yaml
template_sections:
  - Bio
  - Notes
```

### Sample data

```sql
INSERT INTO contact (name, relationship, email) VALUES ('Alice Chen', 'friend', 'alice@example.com');
INSERT INTO contact (name, relationship) VALUES ('Bob Smith', 'colleague');

-- Assume contact IDs are 20260301130000 and 20260301130001
INSERT INTO life_event (event_type, event_date, contact) VALUES ('birthday', '1990-05-15', '20260301130000');
INSERT INTO life_event (event_type, event_date, contact) VALUES ('married', '2024-06-20', '20260301130000');

INSERT INTO interaction (interaction_date, location, contact) VALUES ('2026-02-28', 'Coffee shop', '20260301130000');
```

### Frontend queries

```graphql
# All contacts
query {
  contacts(limit: 50) {
    id, name, relationship, email, phone
  }
}

# Contact's life events and interactions via SQL join
query {
  sql(query: "SELECT le.event_type, le.event_date FROM life_event le WHERE le.contact = '20260301130000' ORDER BY le.event_date") {
    rows
  }
}

# Recent interactions across all contacts
query {
  sql(query: "SELECT c.name, i.interaction_date, i.location FROM interaction i JOIN contact c ON i.contact = c.id ORDER BY i.interaction_date DESC LIMIT 20") {
    rows
  }
}

# Search across everything
query {
  search(query: "alice birthday") {
    id, title, snippet, rank
  }
}
```

### What a contact looks like on disk

```markdown
---
id: 20260301130000
title: Alice Chen
type: contact
date: 2026-03-01
relationship: friend
email: alice@example.com
---

## Bio

Met at RustConf 2024. Software engineer at Acme Corp.

## Notes

Interested in distributed systems and CRDT research.

---
- interaction:: [[20260301130100]]
```

### What an interaction looks like on disk

```markdown
---
id: 20260301130100
title: Coffee catch-up with Alice
type: interaction
date: 2026-02-28
interaction_date: 2026-02-28
location: Coffee shop
---

Talked about CRDT-based apps and the future of local-first software.
She recommended the Ink & Switch essay on local-first.

---
- contact:: [[20260301130000]]
```

Body content (Bio, Notes, free-form text) is added after creation by editing the Markdown file directly or via a frontend text editor.

## Schema design checklist

1. **One table per entity** — panels, categories, bookmarks, contacts, events
2. **Use frontmatter for filterable fields** — dates, enums, booleans, numbers
3. **Use body for rich text** — notes, descriptions, logs
4. **Use references for relationships** — FK columns with `REFERENCES`
5. **Use `allowed_values` for enums** — status, priority, relationship type
6. **Use `default_value` for sensible defaults** — status starts as "todo"
7. **Use `template_sections` for structured body** — consistent headings across records
8. **Keep types small and focused** — more small tables beats fewer bloated ones
9. **Use tags for cross-cutting labels** — tags work across all types
10. **Use search for discovery** — FTS indexes titles, bodies, and tags

## What you get for free

| Feature | How |
|---------|-----|
| Version history | Every mutation is a git commit |
| Offline-first | Works without network, syncs later |
| Multi-device | CRDT resolves conflicts automatically |
| Data portability | Markdown files in a git repo |
| Full-text search | FTS5 with porter stemming |
| Obsidian-compatible | Browse/edit data in any Markdown editor |
| Backups | `git push` to any remote |
| Audit trail | `git log` shows who changed what and when |
