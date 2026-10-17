# Design: Route read and delete through the app contract

## Architecture fit

This lands in the existing app-contract layer (`ddb-core/src/app_contract/`) and
its one established consumption pattern: a `DoogatService` verb returns
`AppOutput<T>`, and every transport decodes its own request into the command,
calls the verb, and renders `value` + `warnings`. `create`/`update`/`apply_schema`
already follow this shape (`service/create.rs`, `service/update.rs`,
`service/schema_apply.rs`); `read`/`delete` are the two CRUD verbs left on the
old direct-binding path (`read_doogat(&str) -> Result<String>`,
`delete_doogat(&str, &str) -> Result<Vec<(String, String)>>`). No new layer is
introduced — this closes the contract-routing gap for the last two verbs,
reusing the warning-forwarding mechanics every transport already has:

- GraphQL: `forward_warnings(&ctx, &output.warnings)` (`warning_extension.rs`),
  already wired for `createDoogat`/`updateDoogat`/`applySchema`.
- REST: the `data` + `warnings: Vec<WarningJson>` envelope (`SingleResponse`,
  `SchemaApplyResponse` in `rest.rs`), already wired for POST/PUT/schema-apply.
- CLI: `crate::warnings::write_warnings(&output.warnings, &mut stderr)`,
  already wired for `create`/`update`.
- FFI: `*Record::from_output(AppOutput<T>)` (`ffi/records.rs`), already wired
  for `SchemaApplyReportRecord`.

Read and delete only need to start producing `AppOutput<ReadResult>` /
`AppOutput<DeleteResult>` at the service boundary; every transport's rendering
mechanics already exist and are reused unchanged.

## Module placement

**Edits to existing files** (no new modules):

- `ddb-core/src/app_contract/results.rs` — `ReadResult` gains `pub raw_content: String` alongside its existing `doogat: ParsedDoogat` (see Interfaces — this is the field change that lets every transport's primary output, including CLI's/FFI's raw text, come from ONE read inside the contract call, closing a verification-pass finding that an out-of-band second raw read was not "really" routed through the contract and could race a concurrent write between the two reads).
- `ddb-core/src/service/read.rs` — add `pub fn read(&self, cmd: ReadCommand) -> Result<AppOutput<ReadResult>>`, which reads the file ONCE and populates both `doogat` (parsed) and `raw_content` (the same bytes, unparsed) from that single read. Keep `read_doogat`/`read_doogat_raw`/`get_doogat_parsed`/`get_doogats_batch` as-is (other callers — `ReadPool`, GraphQL list/search, sync — use the raw/batch forms; only the single-id "get one doogat" entrypoint needs to speak the contract per the PRD's scope).
- `ddb-core/src/service/delete.rs` — add `pub fn delete(&self, cmd: DeleteCommand) -> Result<AppOutput<DeleteResult>>`, implemented as a thin wrapper around the existing `delete_doogat` (which keeps its own `Result<Vec<(String, String)>>` signature — same "thin decode-and-delegate shim" shape `create`/`update` established, named directly in the PRD's Risks section).
- `ddb-core/src/ffi/driver.rs` — `read_doogat`/`delete_doogat` decode into `ReadCommand`/`DeleteCommand`, call `svc.read`/`svc.delete`, return the new FFI records below instead of `String`/`()`.
- `ddb-core/src/ffi/records.rs` — add `ReadResultRecord`, `DeleteResultRecord`, `BrokenBacklinkRecord` (mirrors `SchemaApplyReportRecord`/`SchemaWarningRecord`/`from_output`).
- `ddb-server/src/actor/mod.rs` — `delete_doogat` changes return type from `ActorResult<()>` to `ActorResult<AppOutput<DeleteResult>>`, calling `svc.delete(DeleteCommand { id })` instead of the current `svc.delete_doogat(...).map(|_broken| ())` (which currently discards the broken-backlinks vector — this is the actor-level drop named in the PRD's Problem Statement). `get_doogat` (actor) is unchanged — GraphQL's single-doogat read goes through `ReadPool`, not the actor (see below).
- `ddb-server/src/read_pool.rs` — `get_doogat` changes from `Result<ParsedDoogat>` to `Result<AppOutput<ReadResult>>`, calling `svc.read(ReadCommand { id })` via `with_service` (`spawn_blocking`). This is the one place GraphQL/REST single-doogat reads are served from (confirmed: `queries.rs`'s `doogat(id)` field and `rest.rs`'s `get_doogat` handler both call `ReadPool::get_doogat`).
- `ddb-server/src/schema/queries.rs` — the `doogat(id)` field resolver unwraps `AppOutput<ReadResult>`, calls `forward_warnings(&ctx, &output.warnings)`, and renders `doogat_to_value(&output.value.doogat)` (identical value shape as today; only warnings are new).
- `ddb-server/src/schema/mutations/operations.rs` — `build_delete_doogat_field`: return type changes from `TypeRef::named_nn(TypeRef::BOOLEAN)` to `TypeRef::named_nn("DeleteResult")`; call `forward_warnings`; render `delete_result_to_value(&output.value)`.
- `ddb-server/src/schema/mutations/types.rs` — add `build_delete_result_type()` (`DeleteResult { brokenBacklinks: [BrokenBacklink!]! }`) and `build_broken_backlink_type()` (`BrokenBacklink { sourceId: ID!, sourcePath: String! }`), registered in `ddb-server/src/schema/mod.rs` alongside the existing `build_schema_apply_report_type()`/`build_plan_op_report_type()` registration call.
- `ddb-server/src/rest.rs` — `get_doogat` handler calls `read_pool.get_doogat` (now `AppOutput`), renders `SingleResponse::new(doogat_to_json(&output.value.doogat), warnings_to_json(output.warnings))` (today it hardcodes `vec![]`). `delete_doogat` handler changes from `-> Result<StatusCode, ...>` returning `204 NO_CONTENT` to `-> Result<Json<DeleteResponse>, ...>` returning `200 OK` with a `{ data: { broken_backlinks: [...] }, warnings: [...] }` envelope — snake_case, matching the REST surface's existing convention (`Pagination.per_page`/`total_pages`, `SingleResponse`/`SchemaApplyResponse` already serialize plain Rust field names with no `rename_all`; there is no camelCase precedent anywhere in `rest.rs` to match instead). New `DeleteResponse`/`DeleteResultJson`/`BrokenBacklinkJson` structs, same shape family as `SingleResponse`/`SchemaApplyResponse`, derive `Serialize` with no rename attributes — consistent with every existing REST response struct.
- `ddb-cli/src/commands/crud.rs` — `read()` calls `svc.read(ReadCommand { id })` and prints `output.value.raw_content` directly (ONE read, no second call — see Interfaces) plus any warnings via `write_warnings`. `delete()` calls `svc.delete(DeleteCommand { id, message: None })` and renders `output.warnings` (which now includes one `BROKEN_BACKLINK` entry per broken backlink) via `write_warnings` alone — the pre-existing hand-rolled `eprintln!` loop over the raw backlink tuples is DROPPED, since it would otherwise duplicate the same information `write_warnings` now prints; this is a deliberate, documented stdout/stderr wording change (see Risks), not a byte-for-byte-preserving one.
- `tests/e2e/conformance/driver_rest.rs` (**new file**, Phase 2 task) — mirrors `driver_graphql.rs`'s structure (reqwest-based), implementing `StepOp::ReadDoogat`/`StepOp::DeleteDoogat` (and the ops the harness already requires elsewhere) against the REST routes.
- `tests/e2e/` — new scenario file(s) for read shape parity and delete-with-broken-backlink-warning parity (naming left to `/autopilot:plan-tasks`; follows the existing `tests/e2e/main.rs` registration convention).
- Docs: `docs/src/technical/app-contract.md`, `server.md`, `rest-api.md`, `ffi.md`, `conformance-harness.md`, `CHANGELOG.md` — updated to describe the new read/delete response shapes (REST `DELETE` is now 200 not 204; GraphQL `deleteDoogat` is now `DeleteResult!` not `Boolean!`; FFI `read_doogat`/`delete_doogat` return records not `String`/`()`).
- `dev/local/walkthroughs/` — one showboat walkthrough demonstrating delete-surfaces-a-warning end to end (CLI or GraphQL).

**Not touched: `ddb-server/src/nosql_api.rs`.** The PRD's Transport Impact table marks NoSQL HTTP read `Guaranteed` ("`nosql_get` is a read and renders the contract shape") describing its EXISTING behavior, not a task for this PRD — `nosql_get`'s handler already renders the same `doogat_to_json` shape REST's `data` field uses, and its not-found case is `Option`-based (`Ok(None)` → 404) rather than an error, which is intentionally different from `ReadCommand`'s error-on-missing semantics. This PRD's task list (Phase 0/1/2) never names NoSQL, and delete is explicitly out of scope there (no delete route exists). No code change to `nosql_api.rs` is in scope.

## Interfaces & contracts

### `ddb-core/src/service/read.rs`

```rust
use crate::app_contract::{AppOutput, ReadCommand, ReadResult};

impl<G: GitBackend, I: IndexPort> DoogatService<G, I> {
    /// Fetch a doogat and render it through the app contract. Reads the file
    /// exactly ONCE — `raw_content` and the parsed `doogat` both come from
    /// that single `repo.read_file` call, so every transport's rendering
    /// (parsed value for GraphQL/REST, raw text for CLI/FFI) derives from
    /// the SAME bytes this one call fetched, with no second read that could
    /// race a concurrent write or double-trigger a stale-index rebuild.
    /// Also surfaces any incidental reindex warnings via `ensure_fresh()`,
    /// exactly like `apply_schema` does.
    pub fn read(&self, cmd: ReadCommand) -> Result<AppOutput<ReadResult>> {
        let reindex_warnings = self.ensure_fresh()?;
        let path = self.index.resolve_path(&cmd.id)?;
        let raw_content = self.repo.read_file(&path)?;
        let mut parsed = crate::parser::parse(&raw_content, &path)?;
        parsed.updated_at = self.index.lookup_updated_at(&cmd.id).unwrap_or(None);
        let warnings: Vec<AppWarning> = crate::app_contract::summarize_reindex_warnings(reindex_warnings)
            .into_iter()
            .collect();
        Ok(AppOutput { value: ReadResult { doogat: parsed, raw_content }, warnings })
    }
}
```

This duplicates `get_doogat_parsed`'s resolve/read/parse body rather than
calling it, because `get_doogat_parsed` only returns the parsed `ParsedDoogat`
— it does not hand back the raw bytes it read, and there is no way to recover
the exact original byte content from `ParsedDoogat` afterward (no serializer
back to source text exists; see Alternatives). A four-line body duplicated
once, with a comment pointing each half at its sibling, is the smaller and
clearer option versus adding a `(ParsedDoogat, String)`-returning variant of
`get_doogat_parsed` purely to avoid it; `get_doogat_parsed` itself is
untouched (unchanged signature, unchanged behavior for its other callers —
`ReadPool`'s other call sites, sync, discovery).

`summarize_reindex_warnings` is `pub(crate)` already (`app_contract/output.rs`);
no visibility change needed since `read.rs` is in the same crate.

### `ddb-core/src/service/delete.rs`

```rust
use crate::app_contract::{AppOutput, AppWarning, DeleteCommand, DeleteResult, BrokenBacklink};

/// Stable code for the per-backlink delete warning (PRD-required: the
/// PRD text calls broken backlinks "a BrokenBacklink warning" and ties an
/// acceptance criterion to the warnings channel, not just to `value`).
pub const BROKEN_BACKLINK: &str = "BROKEN_BACKLINK";

impl<G: GitBackend, I: IndexPort> DoogatService<G, I> {
    /// Delete via the app contract. Thin wrapper: `delete_doogat` keeps doing
    /// the real work and keeps its own `Result<Vec<(String, String)>>`
    /// signature (other internal callers are unaffected); this renders its
    /// return value as BOTH the structured `DeleteResult.broken_backlinks`
    /// (for transports that want structured per-entry data, e.g. GraphQL's
    /// `DeleteResult.brokenBacklinks` field) AND one `AppWarning` per broken
    /// backlink (for transports that only forward the generic warnings
    /// channel, e.g. GraphQL extensions / REST `warnings` / CLI stderr via
    /// `write_warnings`) — satisfying the PRD's literal "returns them as
    /// AppWarnings" wording without discarding the structured shape
    /// `results.rs` already defined for `DeleteResult`.
    pub fn delete(&self, cmd: DeleteCommand) -> Result<AppOutput<DeleteResult>> {
        let message = cmd
            .message
            .clone()
            .unwrap_or_else(|| format!("delete doogat {}", cmd.id));
        let broken = self.delete_doogat(&cmd.id, &message)?;
        let broken_backlinks: Vec<BrokenBacklink> = broken
            .into_iter()
            .map(|(source_id, source_path)| BrokenBacklink { source_id, source_path })
            .collect();
        let warnings = broken_backlinks
            .iter()
            .map(|b| AppWarning {
                code: BROKEN_BACKLINK,
                message: format!("{} ({}) still references the deleted doogat", b.source_id, b.source_path),
            })
            .collect();
        Ok(AppOutput { value: DeleteResult { broken_backlinks }, warnings })
    }
}
```

`DeleteCommand.message` (`Option<String>`, new field — see Risks) is read
here and actually used for the git commit message, closing the gap the
dispatch-1 review raised: this is the one and only place `cmd.message` is
constructed from, and every other code sample in this section (actor, CLI,
FFI) below sets it explicitly rather than relying on an implicit default, so
the field's behavior is visible at every call site instead of only in this
one function.

### `ddb-core/src/ffi/records.rs` (new, alongside `SchemaApplyReportRecord`)

```rust
#[derive(Debug, uniffi::Record)]
pub struct BrokenBacklinkRecord {
    pub source_id: String,
    pub source_path: String,
}

#[derive(Debug, uniffi::Record)]
pub struct ReadResultRecord {
    pub content: String,       // raw file content — mirrors read_doogat's current contract
    pub warnings: Vec<SchemaWarningRecord>, // reuse: {code, message} shape is identical
}

#[derive(Debug, uniffi::Record)]
pub struct DeleteResultRecord {
    pub broken_backlinks: Vec<BrokenBacklinkRecord>,
    pub warnings: Vec<SchemaWarningRecord>,
}

impl ReadResultRecord {
    pub(super) fn from_output(out: AppOutput<ReadResult>) -> Self {
        Self {
            content: out.value.raw_content,
            warnings: out.warnings.into_iter().map(|w| SchemaWarningRecord {
                code: w.code.to_string(),
                message: w.message,
            }).collect(),
        }
    }
}
impl DeleteResultRecord {
    pub(super) fn from_output(out: AppOutput<DeleteResult>) -> Self { .. }
}
```

`ReadResultRecord`'s `content` field now comes straight from
`ReadResult.raw_content` — the same bytes `read_doogat` has always returned
(matching jink's existing mobile-replica expectations), fetched once inside
`svc.read()` rather than via a second call. `SchemaWarningRecord`
(`{code: String, message: String}`) is reused verbatim — no new per-verb
warning record type.

### `ddb-server/src/schema/mutations/types.rs` (new object types)

```rust
pub(super) fn build_broken_backlink_type() -> Object {
    Object::new("BrokenBacklink")
        .description("A backlink left dangling by a delete because its source still references the removed doogat.")
        .field(simple_field("sourceId", TypeRef::named_nn(TypeRef::ID)))
        .field(simple_field("sourcePath", TypeRef::named_nn(TypeRef::STRING)))
}

pub(super) fn build_delete_result_type() -> Object {
    Object::new("DeleteResult")
        .description("Result of deleting a doogat.")
        .field(simple_field(
            "brokenBacklinks",
            TypeRef::named_nn_list_nn("BrokenBacklink"),
        ))
}
```

Registered in `ddb-server/src/schema/mod.rs` next to the existing
`.register(build_schema_apply_report_type())`/`.register(build_plan_op_report_type())`
calls.

### `ddb-server/src/schema/mutations/operations.rs`

```rust
pub(super) fn build_delete_doogat_field() -> Field {
    Field::new("deleteDoogat", TypeRef::named_nn("DeleteResult"), |ctx| {
        FieldFuture::new(async move {
            let a = ctx.data::<ActorHandle>()?;
            let id = ctx.args.try_get("id")?.string()?.to_string();
            let output = a.delete_doogat(id).await.map_err(|e| to_graphql_error_from_app(e.into()))?;
            forward_warnings(&ctx, &output.warnings);
            Ok(Some(FieldValue::owned_any(delete_result_to_value(&output.value))))
        })
    })
    .argument(InputValue::new("id", TypeRef::named_nn(TypeRef::ID)).description("The ID of the doogat to delete."))
    .description("Delete a doogat by ID. Cascades: removes junction table rows referencing this ID and cleans dangling wikilinks. Returns broken backlinks created by the deletion. All changes in a single atomic commit.")
}

fn delete_result_to_value(result: &DeleteResult) -> GqlValue {
    let backlinks: Vec<GqlValue> = result.broken_backlinks.iter().map(|b| {
        let mut o = IndexMap::new();
        o.insert(Name::new("sourceId"), GqlValue::from(b.source_id.as_str()));
        o.insert(Name::new("sourcePath"), GqlValue::from(b.source_path.as_str()));
        GqlValue::Object(o)
    }).collect();
    let mut obj = IndexMap::new();
    obj.insert(Name::new("brokenBacklinks"), GqlValue::List(backlinks));
    GqlValue::Object(obj)
}
```

### `ddb-server/src/actor/mod.rs`

```rust
pub async fn delete_doogat(&self, id: String) -> ActorResult<AppOutput<DeleteResult>> {
    let doogat_type: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let doogat_type_for_closure = doogat_type.clone();
    self.call(
        EventIntent::Deleted { id: id.clone(), doogat_type },
        ReloadIntent::None,
        move |svc| {
            let resolved = svc.get_doogat_parsed(&id).ok().and_then(|z| z.meta.doogat_type);
            *doogat_type_for_closure.lock().unwrap_or_else(|e| e.into_inner()) = resolved;
            svc.delete(ddb_core::app_contract::DeleteCommand { id: id.clone(), message: None })
        },
    )
    .await
}
```

`message: None` — the server actor has no caller-supplied commit message
input today (GraphQL/REST `deleteDoogat`/`DELETE` take no message argument),
so it keeps using `svc.delete`'s default, identical to today's hardcoded
`format!("delete doogat {id}")` in the actor.

`EventIntent::Deleted` and `ReloadIntent::None` are unchanged — only the return
value swaps from `()` to `AppOutput<DeleteResult>`. Confirm `AppOutput<T>`
already satisfies whatever bound `call<R>`/`ActorResult<R>` requires — it does:
`apply_schema` above already returns `ActorResult<AppOutput<SchemaApplyReport>>`
through the same `self.call(...)` machinery, so no new trait impl is needed.

### `ddb-server/src/read_pool.rs`

```rust
pub async fn get_doogat(&self, id: String) -> Result<AppOutput<ReadResult>> {
    self.with_service(move |svc| svc.read(ReadCommand { id })).await
}
```

(`with_service` / `spawn_blocking` wiring unchanged — mirrors the existing
`get_doogat` body exactly, just calling `svc.read` instead of
`svc.get_doogat_parsed`.)

### `ddb-server/src/rest.rs`

```rust
#[derive(Serialize)]
pub struct BrokenBacklinkJson { pub source_id: String, pub source_path: String }

#[derive(Serialize)]
pub struct DeleteResultJson { pub broken_backlinks: Vec<BrokenBacklinkJson> }

#[derive(Serialize)]
pub struct DeleteResponse { pub data: DeleteResultJson, pub warnings: Vec<WarningJson> }

async fn get_doogat(
    Extension(read_pool): Extension<ReadPool>,
    Path(id): Path<String>,
) -> Result<Json<SingleResponse>, (StatusCode, Json<ErrorBody>)> {
    let output = read_pool.get_doogat(id).await.map_err(rest_error)?;
    let warnings = warnings_to_json(output.warnings);
    Ok(Json(SingleResponse::new(doogat_to_json(&output.value.doogat), warnings)))
}

async fn delete_doogat(
    Extension(actor): Extension<ActorHandle>,
    Path(id): Path<String>,
) -> Result<Json<DeleteResponse>, (StatusCode, Json<ErrorBody>)> {
    let output = actor.delete_doogat(id).await.map_err(rest_error)?;
    let warnings = warnings_to_json(output.warnings);
    let broken_backlinks = output.value.broken_backlinks.into_iter()
        .map(|b| BrokenBacklinkJson { source_id: b.source_id, source_path: b.source_path })
        .collect();
    Ok(Json(DeleteResponse { data: DeleteResultJson { broken_backlinks }, warnings }))
}
```

`DELETE /doogats/{id}` now returns `200 OK` (was `204 NO_CONTENT`) — a body is
required to carry `data`/`warnings`; 204 forbids a response body by HTTP
semantics. This is the REST half of Success Metric 5 ("REST DELETE returns
200 with its result envelope").

### `ddb-core/src/ffi/driver.rs`

```rust
pub fn read_doogat(&self, id: String) -> Result<ReadResultRecord, DdbError> {
    self.with_service(|svc| {
        // One call: `out.value.raw_content` is already the exact bytes
        // `read_doogat` used to return, read once inside `svc.read()`
        // itself — no second file read, no second `ensure_fresh()`.
        let out = svc.read(crate::app_contract::ReadCommand { id }).map_err(DdbError::from)?;
        Ok(ReadResultRecord::from_output(out))
    })
}

pub fn delete_doogat(&self, id: String, message: String) -> Result<DeleteResultRecord, DdbError> {
    self.with_service(|svc| {
        // FFI is the one transport with a caller-supplied commit message
        // today (driver.rs:91 passes `message` straight to `delete_doogat`,
        // which uses it verbatim) — `DeleteCommand.message: Some(message)`
        // preserves that behavior exactly, unlike the actor/CLI/GraphQL/REST
        // call sites, which have no message input and pass `None`.
        let out = svc
            .delete(crate::app_contract::DeleteCommand { id, message: Some(message) })
            .map_err(DdbError::from)?;
        Ok(DeleteResultRecord::from_output(out))
    })
}
```

### `ddb-cli/src/commands/crud.rs`

```rust
pub(crate) fn read(repo: &std::path::Path, id: &str) -> ddb_core::error::Result<()> {
    let svc = DoogatService::open(repo)?;
    let output = svc.read(ddb_core::app_contract::ReadCommand { id: id.to_string() })?;
    // `output.value.raw_content` is the exact bytes `read_doogat` used to
    // return — one file read, one `ensure_fresh()`, no second service call.
    out!("{}", output.value.raw_content)?;
    crate::warnings::write_warnings(&output.warnings, &mut std::io::stderr())
        .unwrap_or_else(|e| eprintln!("warning write failed: {e}"));
    Ok(())
}

pub(crate) fn delete(repo: &std::path::Path, id: &str) -> ddb_core::error::Result<()> {
    let svc = DoogatService::open(repo)?;
    let output = svc.delete(ddb_core::app_contract::DeleteCommand { id: id.to_string(), message: None })?;
    // One rendering path, not two: `output.warnings` already carries one
    // `BROKEN_BACKLINK` AppWarning per entry in `output.value.broken_backlinks`
    // (see `service/delete.rs`), so `write_warnings` alone prints the full
    // broken-backlink detail. The prior manual `eprintln!` loop over
    // `output.value.broken_backlinks` is DROPPED (it would otherwise print
    // the same information twice). This changes the exact stderr wording
    // from today's `"warning: N doogat(s) have broken backlinks..."` to
    // `write_warnings`'s one-line-per-warning format — an accepted,
    // documented wording change (CHANGELOG), not a silent regression; see
    // Test strategy outline, which no longer claims byte-identical stderr.
    crate::warnings::write_warnings(&output.warnings, &mut std::io::stderr())
        .unwrap_or_else(|e| eprintln!("warning write failed: {e}"));
    Ok(())
}
```

CLI `read` previously called `read_doogat` for raw content and `read` for the
contract shape/warnings as two separate service calls; fixed above to a
single `svc.read()` call plus a path-based raw read that reuses its already-
fresh index state (no second `ensure_fresh()`), matching the FFI driver's
`read_doogat` fix in this same section.

## Data flow

**Read:** transport request → `ReadCommand { id }` → `DoogatService::read` →
`ensure_fresh()` (reindex warnings) + `index.resolve_path` + ONE
`repo.read_file` + `parser::parse` of those same bytes → `AppOutput<ReadResult>
{ doogat, raw_content }` → transport renders either `value.doogat` (GraphQL,
REST) or `value.raw_content` (CLI, FFI — the same bytes, already read; no
second file read) and forwards `warnings` through its existing
warning-rendering mechanism (GraphQL extensions / REST `warnings` array / CLI
stderr / FFI record field).

**Delete:** transport request → `DeleteCommand { id }` → `DoogatService::delete`
→ `delete_doogat` (cascade plan, backlink collection, single git commit via
`execute_delete_plan`, mirror eviction — **unchanged** internals) →
`AppOutput<DeleteResult>` wrapping the broken-backlinks vector AND, per entry,
a `BROKEN_BACKLINK` `AppWarning` → transport renders `value.broken_backlinks`
(newly a first-class field everywhere, not a discarded return) and forwards
`warnings` (non-empty whenever a backlink broke; empty on a clean delete).

No new data paths are introduced; this is purely a reshaping of return values
already computed, at the four transport boundaries (CLI, GraphQL, REST, FFI)
plus the two internal relay layers (`ActorHandle`, `ReadPool`) that currently
unwrap/discard them.

## Reuse inventory

- `AppOutput<T>` / `AppWarning` (`app_contract/output.rs`) — reused as-is; no changes.
- `summarize_reindex_warnings` (`app_contract/output.rs`, `pub(crate)`) — reused for `read`'s incidental warnings, same as `apply_schema`.
- `ReadResult`, `DeleteResult`, `BrokenBacklink` (`app_contract/results.rs`) — reused as-is; these are the DTOs the PRD exists to give a first consumer.
- `forward_warnings` (`warning_extension.rs`) — reused as-is for the `deleteDoogat`/`doogat` GraphQL resolvers.
- `warnings_to_json` / `WarningJson` (`rest.rs`) — reused as-is for the new REST delete/read warnings rendering.
- `write_warnings` (`ddb-cli/src/warnings.rs`) — reused as-is for CLI read/delete.
- `SchemaWarningRecord` (`ffi/records.rs`) — reused verbatim as the FFI warning shape for the new `ReadResultRecord`/`DeleteResultRecord` (no new per-verb warning record).
- `SchemaApplyReportRecord::from_output` (`ffi/records.rs:327`) — pattern template for the new `from_output` constructors, not reused code (different source type).
- `simple_field` helper (`schema/mutations/types.rs`) — reused for the new `BrokenBacklink`/`DeleteResult` GraphQL object field declarations.
- `StepOp::ReadDoogat` / `StepOp::DeleteDoogat` (`tests/e2e/conformance/fixture.rs`) — already defined and already interpreted by `driver_cli.rs`/`driver_graphql.rs`; reused as-is by the new parity step and the new `driver_rest.rs`.
- `driver_graphql.rs` — reused as the structural template for the new `driver_rest.rs` (reqwest-based driver mirroring the same `StepOp` match), not reused code.
- Greps tried for an existing REST driver or read/delete result-rendering helper that might already cover this: `rg "driver_rest"` (nothing), `rg "DeleteResultJson|BrokenBacklinkJson"` (nothing), `rg "fn.*delete.*-> *Result<AppOutput"` (nothing — confirms no prior partial implementation exists to build on).

## Alternatives considered

1. **Chosen:** add `svc.read`/`svc.delete` as thin contract-shaped wrappers over the existing `read_doogat`/`get_doogat_parsed`/`delete_doogat`, keep those lower-level methods for their other internal callers, and update each transport's existing single-doogat read/delete call site to use the new wrapper. Smallest diff that satisfies every Success Metric; no internal caller other than the four transports is touched.
2. **Smallest-diff alternative:** route `AppOutput`/warnings only through the four transport-facing call sites (actor `delete_doogat`, `ReadPool::get_doogat`, FFI `read_doogat`/`delete_doogat`, CLI `read`/`delete`) *without* adding `svc.read`/`svc.delete` at all — i.e., construct `ReadResult`/`DeleteResult` ad hoc at each of those four sites directly from `get_doogat_parsed`/`delete_doogat`'s existing return values. Rejected: it would duplicate the "wrap in `ReadResult`/`DeleteResult`, compute warnings" logic four times instead of once, and leaves `DoogatService` — the PRD's own named chokepoint for contract routing (`CLAUDE.md`'s "DoogatService is the sole entry point") — without the verb at all, which the acceptance criteria for Phase 0 explicitly require ("unit tests assert `ReadResult` content...").
3. **Rejected alternative:** derive CLI/FFI's raw text FROM the parsed `ParsedDoogat` by re-serializing it back to Markdown, instead of reading raw bytes at all. Rejected: no serializer from `ParsedDoogat` back to its exact original three-zone Markdown text exists or is in scope here (round-tripping risks losing exact formatting/whitespace a jink mobile replica may depend on). **Chosen instead** (superseding an even earlier draft that issued a second `read_doogat_raw` call after `svc.read()`, which a verification pass flagged as a same-file double-read race risk): `ReadResult` carries `raw_content` as a sibling field to `doogat`, both populated from the ONE read `svc.read()` performs — no re-serialization, no second read.

## Risks & edge cases

- **Resolved in this design, not merely mitigated**: an earlier draft had CLI/FFI issue a second `repo.read_file`/`read_doogat_raw` call to get raw bytes after `svc.read()`'s own read — risking a race against a concurrent write between the two reads (a verification-pass finding) and, before that, risking a double `ensure_fresh()`. `ReadResult` now carries `raw_content` directly (one read inside `svc.read()`, see Interfaces), so there is exactly one file read and one freshness check per `read()` call, for every transport.
- **FFI `delete_doogat`'s existing `message: String` parameter needs a home in `DeleteCommand`**: `commands.rs`'s current `DeleteCommand { pub id: String }` carries no message field, but FFI's `delete_doogat` (`driver.rs:91`) today passes its caller-supplied `message` straight to `delete_doogat`, which uses it verbatim for the git commit. Routing FFI through `svc.delete(DeleteCommand { id, .. })` without a message field would silently drop that caller-supplied text (every delete would get the generic `"delete doogat {id}"` message instead of jink's own). **Resolved**: `DeleteCommand` gains `pub message: Option<String>`, read by `svc.delete` and defaulted to `format!("delete doogat {id}")` when `None` (see Interfaces — `service/delete.rs`). FFI is the only call site that passes `Some(message)`; the actor, CLI, and REST/GraphQL (none of which take a message argument today) pass `None`, matching their current hardcoded-message behavior exactly. One additive field on a command struct with no in-tree consumer yet, so no migration cost.
- **GraphQL `deleteDoogat` return-type change is breaking** (named in PRD Risks) — any existing GraphQL client code expecting `Boolean` breaks. Accepted under the maintainer's 2026-09-05 beta decision (no deprecation window); mitigated by CHANGELOG + `docs/src/technical/server.md` update in the same PRD.
- **REST `DELETE` 204→200 status change is breaking** for any client checking `response.status == 204`. Same maintainer-decision mitigation.
- **Likely next changes after this PRD** (per the roadmap, memory `project_ddb_growth_roadmap`): 00176 (route search through the contract) and 00178 (structured fields input) both explicitly plan to reuse the REST conformance driver this PRD's Phase 2 builds — so `driver_rest.rs`'s `StepOp` match arms should be written generically enough that adding `StepOp::Search`'s REST leg later is a small diff, not a rewrite (mirror `driver_graphql.rs`'s existing per-op match structure exactly, don't special-case read/delete's HTTP methods beyond what `StepOp` already encodes). 00177 (FFI CRUD through contract) will want the same `from_output`-constructor pattern established here for `ReadResultRecord`/`DeleteResultRecord` — keep that pattern copy-paste-able (do not fold it into a generic macro in this PRD; premature for two instances, per the user's simplicity rules, but name the pattern clearly in a doc comment so 00177 can repeat it for create/update's FFI records too).
- **This design boxes in**: making `DeleteCommand.message` optional (rather than, say, always deriving it service-side) keeps FFI's existing message-customization behavior alive for 00177, which will likely want the same for `CreateCommand`/`UpdateCommand` FFI parity — consistent precedent either way.

## Test strategy outline

Per the PRD's Test Strategy and `AGENTS.md`'s Definition of Done:

- **Unit tests** (colocated, `cargo test-ci`): `service/read.rs` — `read()` returns `ReadResult` with the parsed doogat's fields; a read during a corrupt-file reindex window surfaces the `REINDEX_SKIPPED_FILES` warning (mirrors `mock_index_tests.rs`'s existing `apply_schema_facade_includes_exactly_one_reindex_warning_when_files_are_skipped`). `service/delete.rs` — a delete that breaks a backlink yields exactly one `BrokenBacklink` per dangling source in the returned `AppOutput`'s `value.broken_backlinks` **AND** exactly one `AppWarning { code: BROKEN_BACKLINK, .. }` per entry in that same `AppOutput`'s `warnings` (assert the warning channel directly, not just `value` — a regression that always returns `warnings: vec![]` must fail this test even though `broken_backlinks` is still populated); a clean delete (no inbound references) yields an empty `broken_backlinks` vec AND an empty `warnings` vec; `DeleteCommand { message: None }` uses the default message, `Some(msg)` uses the caller's.
- **GraphQL schema tests**: `deleteDoogat` resolves to a `DeleteResult` object (not `Boolean`); a delete with a broken backlink round-trips `brokenBacklinks[0].sourceId`/`sourcePath` AND forwards the `BROKEN_BACKLINK` warning through the extensions channel (assert both the `value` shape and the `forward_warnings` call — a test that only checks `brokenBacklinks` would pass even if `forward_warnings` were never called); a clean delete forwards zero warnings. `doogat(id)` still resolves the unchanged `Doogat` shape (no regression) and a reindex-skip warning surfaces via the extensions channel the same way.
- **REST tests**: `GET /doogats/{id}` response envelope now always carries `warnings: []` when clean (regression: today's hardcoded `vec![]` must still serialize identically for the common case); `DELETE /doogats/{id}` returns `200` with `{"data": {"broken_backlinks": [...]}, "warnings": [{"code": "BROKEN_BACKLINK", "message": "..."}]}` (snake_case keys, matching `rest.rs`'s existing convention), not `204`; a clean delete's `warnings` array is empty and `broken_backlinks` is `[]`.
- **FFI tests** (local bindgen validation, not platform compilation — 00194's scope): `read_doogat` returns `ReadResultRecord` with non-empty `content`; `delete_doogat` on a doogat with an inbound reference returns a `DeleteResultRecord` whose `broken_backlinks` is non-empty AND whose `warnings` is non-empty (not just one or the other); a clean delete's `warnings` is empty; Swift/Kotlin bindings regenerate clean (`dev/bin/build-xcframework`/`build-android` or the local bindgen check, per existing CI step).
- **CLI regression**: `ddb read <id>` stdout is byte-identical to today (same raw content, now sourced from `output.value.raw_content` inside the one `svc.read()` call instead of a separate `svc.read_doogat()` call). `ddb delete <id>` stderr wording CHANGES (accepted, documented in CHANGELOG, not byte-identical): the old hand-rolled `"warning: N doogat(s) have broken backlinks..."` block is replaced by `write_warnings`'s one-`BROKEN_BACKLINK`-line-per-backlink format, since both the structured value and the generic warnings channel are now populated and printing both would duplicate the same information — assert the new format's exact lines instead of the old ones.
- **Conformance harness** (Phase 2, Tier 2 execution): extend the existing `StepOp::ReadDoogat`/`StepOp::DeleteDoogat` fixtures into a read+delete parity step across CLI+GraphQL+REST (new `driver_rest.rs`), asserting identical `ReadResult` fields and the same broken-backlink warning/value across all three. Author in this PRD; execution delegated to CI per the two-tier Definition of Done.
- **Rust e2e scenarios** (Tier 2 authoring, this PRD): read-shape scenario, delete-with-broken-backlink-warning scenario, registered in `tests/e2e/main.rs`.
- **Showboat walkthrough**: delete-surfaces-a-warning, verified only via `dev/bin/safe-showboat-verify`.

## Review log

Dispatch 1 (claude) findings, fixed inline during this dispatch:
- Blocking: `DeleteCommand.message` decision ("takes (a)") wasn't propagated into the Interfaces code samples (service/delete.rs hardcoded the message; actor/CLI/FFI samples omitted the field entirely). Fixed: every sample now constructs `DeleteCommand { id, message: Some(..)/None }` explicitly and `svc.delete` actually reads `cmd.message`.
- Blocking: `delete()` only placed broken backlinks on `value`, never on `warnings`, contradicting the PRD's literal "returns them as AppWarnings" wording and its Phase 0/Critical-scenario acceptance criteria tied to the warnings channel. Fixed: `delete()` now emits a `BROKEN_BACKLINK` `AppWarning` per entry in addition to the structured `value.broken_backlinks`.
- Non-blocking: `read()`'s draft duplicated `get_doogat_parsed`'s resolve/read/parse body instead of delegating. Fixed: extracted a shared `get_doogat_parsed_already_fresh` helper, `read()` calls it.
- Non-blocking: FFI `read_doogat` ran `ensure_fresh()` twice (once via `read_doogat`, once via `read()`). Fixed: FFI now calls `read()` once and derives raw content from `output.value.doogat.path` via `read_doogat_raw` (no second freshness check).
- Question: confirmed read's REST wire shape is additive-only (unlike delete's 204→200 change) — no action needed.

dispatch 1 (claude): cardinal-sin 0, blocker 2, non-blocker 2, question 1

Dispatch 2 (codex) findings, fixed inline during this dispatch:
- Blocking: CLI's `read()` sample still called the legacy `read_doogat` for its primary stdout content (not derived from the contract at all) and called it BEFORE `svc.read()`, so a stale-index rebuild in the first call could consume the reindex warning the second call was supposed to surface. Fixed: CLI `read` now mirrors the FFI fix — one `svc.read()` call, raw content via `read_doogat_raw(&output.value.doogat.path)`, one `ensure_fresh()` total, and the printed content is now actually derived from the `ReadResult` the contract produced.
- Blocking: REST section's prose used camelCase (`brokenBacklinks`) while the actual `#[derive(Serialize)]` structs shown have no `rename_all` and would serialize snake_case — an internal contradiction. Fixed: prose corrected to snake_case throughout, with an explicit citation of the existing REST convention (`Pagination.per_page`/`total_pages`, no rename_all anywhere in `rest.rs`) that snake_case is already the established wire casing, not a new choice.
- Non-blocking: unit/FFI test bullets asserted only `value.broken_backlinks`, which would pass even if `warnings` were always empty (the literal PRD-required channel). Fixed: test strategy bullets now require asserting the `warnings`/`AppWarning` channel directly, not just `value`.
- Non-blocking: CLI delete's test strategy claimed "byte-identical" stderr, which became false once the fix above added a service-level `BROKEN_BACKLINK` warning per backlink on top of the pre-existing manual print loop (double-printing the same info). Fixed: dropped the redundant manual loop in favor of `write_warnings` alone (one rendering path), and the test strategy bullet no longer claims byte-identical output — documents the wording change instead.
- Question: NoSQL HTTP read has no implementation/verification plan in this design despite being marked `Guaranteed`. Fixed: added an explicit "not touched" note to Module placement explaining `nosql_get` already satisfies `Guaranteed` today and is out of this PRD's task list.

dispatch 2 (codex): cardinal-sin 0, blocker 3, non-blocker 2, question 1

Dispatch 3 (codex, verification — ran because dispatch 2 found blockers) findings, fixed inline during this dispatch:
- Blocking (new, raised on re-check): CLI/FFI's raw content still came from a second `read_doogat_raw` call issued after `svc.read()`, so it was not truly derived from the contract's own result and could race a concurrent write between the two reads. Fixed: `ReadResult` gained a `raw_content: String` field populated by `svc.read()`'s single file read (alongside the parsed `doogat`), so CLI and FFI now render `value.raw_content` directly — one read, one `ensure_fresh()`, no race, and the content genuinely comes from the contract call.
- Confirmed resolved (no action needed): the reindex-warning-ordering fix from dispatch 2, and the REST snake_case fix from dispatch 2.
- Non-blocking: "Data flow → Delete" still said delete's warnings are "currently always empty", stale from before the BROKEN_BACKLINK-warning fix landed. Fixed: corrected to describe the now-populated warnings channel.
- Non-blocking: "Module placement → crud.rs" still described the dropped manual `eprintln!` loop as retained alongside `write_warnings`. Fixed: corrected to describe the single `write_warnings`-only rendering path and flagged the wording change as deliberate.
- Non-blocking (new): test strategy referred to "`value.warnings`", but warnings live on `AppOutput`, not on `value`. Fixed: reworded to "the returned `AppOutput`'s `warnings`".

dispatch 3 (codex): cardinal-sin 0, blocker 1, non-blocker 3, question 0

result: ok
