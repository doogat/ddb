# Transport Policy Inventory

This is the policy map PRD 00149 Phase 0 produces: which transport-layer
decision each public interface makes, and whether that decision is **app-owned**
(lives in `ddb-core` service/app-contract code), **transport-owned** (lives in
the adapter because it is pure protocol framing), or **specialized** (a
deliberate per-interface difference recorded in the capability matrix).

It is the inventory Success Criterion 1 asks for. It also records the Phase 0
finding on the legacy REST error vocabulary (D-01/D-03/D-08) so later phases do
not invent removal work that is already done.

Seeded from `server.md` §"Shared transport glue" and §"Thin adapters" (the
already-centralized glue), then extended to the two surfaces that consolidation
did not touch: CLI and FFI.

## Public interfaces

The six public application interfaces, with their promise level from
[Choosing an interface](../guide/building-apps.md#choosing-an-interface):

| Interface | Code | Promise level |
|-----------|------|---------------|
| CLI | `ddb-cli/src/` | CRUD `Guaranteed` |
| GraphQL | `ddb-server/src/schema/` | CRUD `Guaranteed` (network flagship) |
| REST | `ddb-server/src/rest.rs` | `Specialized` (base CRUD + list/search) |
| PgWire | `ddb-server/src/pgwire.rs` | `Guaranteed` for SQL/reporting |
| FFI | `ddb-core/src/ffi/` | embedded driver |
| NoSQL HTTP | `ddb-server/src/nosql_api.rs` | read-only; writes `Intentionally absent` |

## Policy map

Each row is a transport-layer decision. Each cell records who owns it on that
interface.

| Decision | CLI | GraphQL | REST | PgWire | FFI | NoSQL HTTP |
|----------|-----|---------|------|--------|-----|------------|
| Input decoding | transport (`clap`) | app-glue (`schema/input.rs`) | transport (`serde`) | transport (SQL text) | transport (UniFFI args) | transport (query params) |
| Create routing | app facade (`DoogatService::create`) | app facade (via actor) | app facade (via actor) | n/a (SQL `INSERT`) | direct service (`_raw`) | n/a (read-only) |
| Update routing | app facade (`DoogatService::update`) | app facade (via actor) | app facade (via actor) | n/a (SQL `UPDATE`) | direct service (`_raw`) | n/a (read-only) |
| Error mapping | transport text + `format_app_error` | app vocab (`classify` / `to_graphql_error*`) | app vocab (`http_error_response`) | specialized (PG error strings) | specialized (untyped strings) | app vocab (`http_error_response`) |
| Warning channel | text on stderr | `extensions.warnings` | `warnings` array | specialized (none) | specialized (none) | n/a (read-only) |
| Schema-reload classification | n/a | app-glue (`classify.rs`) | n/a | app-glue (`classify.rs`) | n/a | n/a |
| Response shaping | transport (stdout text) | transport (GraphQL objects) | transport (`*Json`) | transport (PG rows) | transport (UniFFI records) | transport (`*Json`) |

### What is already app-owned

- **Create routing** runs through `DoogatService::create(CreateCommand) ->
  AppOutput<ParsedDoogat>` on CLI, GraphQL, and REST (PRD 00147). Warnings are
  forwarded on all three. This is the template the update slice copies.
- **HTTP error mapping** is one helper, `http_error_response`
  (`ddb-server/src/http_error.rs`), shared by REST and NoSQL HTTP. It calls
  `crate::error::classify`, which returns the unified code vocabulary
  (`NOT_FOUND`, `VALIDATION_ERROR`, `UNIQUE_VIOLATION`, ...) — the same codes
  GraphQL exposes under `extensions.code`.
- **SQL schema-mutation classification** is one helper,
  `requires_schema_reload` (`ddb-core/src/sql_engine/classify.rs`), shared by
  GraphQL `executeSql`/`executeBatch` and PgWire.
- **GraphQL input decoding** is shared across the four dynamic mutations via
  `ddb-server/src/schema/input.rs`.

### Closed by PRD 00149 (now app-owned)

These were this PRD's targets; both are now migrated:

- **Update routing.** GraphQL, CLI, and REST update now route through the
  `DoogatService::update(UpdateCommand) -> AppOutput<ParsedDoogat>` app facade
  (GraphQL and REST via `ActorHandle::update_doogat`, which now returns
  `AppOutput<ParsedDoogat>`; CLI calls `svc.update` directly). Errors map through
  the app vocabulary (`to_graphql_error_from_app` / `format_app_error` /
  `http_error_response`), and warnings are forwarded on all three (GraphQL
  `extensions.warnings`, CLI stderr, REST `warnings` array), mirroring the
  create slice (design contracts C-1..C-6).
- **REST typed create.** `CreateBody` now has a `fields` JSON-string member
  (parsed via `parse_fields_json`), so typed create is expressible over REST;
  the named typed columns are populated from `fields` on the created doogat
  (design contract C-7). This closed the D-04/D-09 overclaim.

### Specialized (deliberate per-interface difference)

These differences stay; they are recorded as specialized in the capability
matrix, not treated as drift to fix.

- **FFI error/warning shape.** FFI returns untyped string errors and has no
  structured warning channel (deprecation entries D-02/D-05/D-06/D-07/D-13).
  Routing FFI through the app facade needs a warning-carrying UniFFI record that
  does not exist yet. Deferred to the named follow-up PRD `ffi-typed-errors-v1`.
- **PgWire error/warning shape.** Errors surface as PostgreSQL error message
  strings, not `extensions.code` (PRD 00139 §T22); there is no notice/warning
  emission path (D-12). Deferred to `pgwire-structured-errors-v1`.
- **CLI machine-readable mode.** CLI warnings are unstructured text on stderr
  (`warning: <code>: <message>`); there is no JSON error/warning mode (D-10).
  Deferred to `cli-json-errors-v1`.
- **PgWire FTS5 search / backlinks.** Reachable only via raw SELECT against the
  FTS5 virtual table / `_ddb_*` tables, not a curated workflow shape.
- **NoSQL HTTP writes.** `Intentionally absent` by design (read-only interface).

## Finding: legacy REST error vocabulary (D-01/D-03/D-08)

PRD 00149's Problem statement lists "legacy REST short-string error vocabulary
removal (D-01/D-03/D-08)" as remaining work. **It is already done.** Confirmed
against the code, per the design's Phase 0 risk bullet:

- Every REST error path routes through `rest_error` (`rest.rs:157`), which is a
  one-line delegate to `crate::http_error::http_error_response`.
- `http_error_response` calls `crate::error::classify`, which returns the
  unified code vocabulary shared with GraphQL.
- A lockstep regression test (`rest_error_delegates_to_shared_http_helper`,
  `rest.rs`) pins REST status/code to the shared helper.

No REST-local short-string error code vocabulary remains. PRD 00149 must **not**
add a removal task for D-01/D-03/D-08; they were satisfied by the PRD 00147 /
00143 error centralization. The matching `building-apps.md` Migration Notes
already describe these as shipped.

## Correctness-over-compatibility flag

No silent-data-loss or invalid-write behavior surfaced in this inventory that a
thinning task would be tempted to preserve. The update slice keeps the existing
`update_doogat_parsed` write path unchanged, so the mutation/index/git behavior
is identical; only error and warning *shaping* changes. If a later phase finds
such a case, it must be fixed even when a downstream consumer depends on it
(see the compatibility checklist's correctness-override rule).
