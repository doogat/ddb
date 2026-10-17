# Design: Route search through the app contract (00176)

## Architecture fit

This lands entirely in the existing app-contract layer and its four adapters — no new layer or module boundary:

- `ddb-core/src/app_contract/` (commands.rs, results.rs) — the adapter-neutral contract types (`SearchCommand`, the new `PageRequest`, `AppSearchResult`). Per the module's own adapter-neutrality invariant (enforced by `ddb-core/tests/app_contract_adapter_guard.rs`), `PageRequest`/`SearchCommand` stay free of `rusqlite`/`git2`/`redb`/`axum`/`async_graphql` types.
- `ddb-core/src/service/search.rs` — gains one new facade method, `search_command`, mirroring the existing `create`/`update` facades in `create.rs`/`update.rs` (capture `ensure_fresh()` warnings, delegate to the existing `search_paginated_filtered`, wrap in `AppOutput`). The pre-existing `search`/`search_paginated`/`search_paginated_filtered` methods are untouched — they are the mechanism the new facade calls into, same as `create` delegates to `batch_create_with_message`.
- `ddb-core/src/ffi/{driver.rs,records.rs}` — a new filtered FFI search method, the existing unfiltered `search_paginated` reimplemented to route through `PageRequest`/`search_command` (closing the PRD's own named drift — its provenance note cites `ffi/driver.rs:103-115`, i.e. this exact method, as "uncapped"), and a widened FFI `SearchResult` record.
- `ddb-server/src/read_pool.rs` — gains `ReadPool::search_command`, a thin `with_service` wrapper over the new `DoogatService::search_command`, so GraphQL and REST (which only ever reach the service through `ReadPool`, never directly) can actually call the new facade instead of hand-rolling the same `search_paginated_filtered` call it wraps. Without this wrapper, GraphQL/REST would stay on `ReadPool::search` and never construct a `SearchCommand`, breaking the PRD's "single entrypoint" success metric (caught in review dispatch 1).
- `ddb-server/src/{schema/queries.rs,rest.rs}` — GraphQL's `search` resolver and REST's search branch of `GET /doogats` build a `SearchCommand`/`PageRequest` and call `ReadPool::search_command`, instead of their own ad hoc limit/offset handling and the unfiltered-pool-method path.
- `ddb-cli/src/commands/query.rs` — the `search` command decodes into `SearchCommand` and prints the two now-available fields it currently drops.

**Scope boundary (deliberate):** the `doogats` list query (GraphQL) and the plain `GET /doogats` list branch (REST, no `q` param) are **not** touched. The PRD title and Success Metrics are about `search`; the list endpoints' own pagination drift is a separate, larger surface (list filters, sort, REST's `ListParams`) that belongs to its own PRD. `PageRequest` is written generically enough that a future list-contract PRD can adopt it without changes to this PRD's code — see Alternatives.

## Module placement

New code:

- `ddb-core/src/app_contract/commands.rs` — add `pub struct PageRequest` + `impl PageRequest` (the `decode` constructor), and replace the body of `pub struct SearchCommand` (edit, not new file).
- `ddb-core/src/app_contract/results.rs` — replace the body of `pub struct AppSearchResult` (edit).
- `ddb-core/src/app_contract/mod.rs` — add `PageRequest` to the `pub use commands::{...}` re-export list.
- `ddb-core/src/service/search.rs` — add `pub fn search_command` (new method, same `impl` block as the existing search methods).
- `ddb-core/src/ffi/records.rs` — widen `pub struct SearchResult` (edit); add new `pub struct FieldEntry` (new type, small); update the existing `impl From<crate::types::SearchResult> for SearchResult` (edit).
- `ddb-core/src/ffi/driver.rs` — add `pub fn search_filtered` (new method); edit the body of the existing `pub fn search_paginated` (same signature, now decodes through `PageRequest`/`search_command` instead of calling `svc.search_paginated` directly — see Interfaces & contracts and Review log dispatch 3).
- `ddb-server/src/read_pool.rs` — add `pub async fn search_command` (new method, same `impl ReadPool` block as the existing `search`).
- `ddb-server/src/schema/queries.rs` — edit the `search` field resolver body (the `{ ... }` block starting `// search(query, limit?, offset?, types?, tag?, where?)`) to build a `SearchCommand` and call `pool.search_command(cmd)`.
- `ddb-server/src/rest.rs` — add `fn decode_search_page` (new helper) and a new `async fn handle_search` body that builds a `SearchCommand` and calls `read_pool.search_command(cmd)`; edit `list_doogats` to branch on the raw `q` presence *before* calling `parse_list_params` (currently it calls `parse_list_params` unconditionally, then branches on `params.q`).
- `ddb-cli/src/commands/query.rs` — edit `pub(crate) fn search` to build a `SearchCommand`/call `search_command`, change its `limit`/`offset` params to `Option<usize>` (so an omitted flag reaches `PageRequest::decode` as `None` and gets the shared 50 default, instead of clap's old hardcoded 20), accept new `doogat_type: Option<String>`/`tag: Option<String>` params forwarded into `SearchCommand.types`/`.tag`, and print all 10 hit fields (today it prints 4: id/title/path/snippet).
- `ddb-cli/src/main.rs` — edit the `Search` clap variant: drop `limit`'s/`offset`'s `#[arg(long, default_value = "20"/"0")]` defaults, making both `Option<usize>` (`#[arg(long)]`, no default) so omission reaches `PageRequest::decode` as `None` rather than clap substituting its own default; add `#[arg(long = "type")] doogat_type: Option<String>` and `#[arg(long)] tag: Option<String>`, mirroring the names GraphQL/REST already use for the same filters. Review dispatch 3 (codex verification) confirmed this was still a real gap after dispatch 2's partial fix: the PRD's own Test Strategy Phase 2 task text ("the same query + filter across CLI/GraphQL/REST returns the same hit ids and field set") and Success Metrics ("Same query across CLI/GraphQL/REST/FFI returns the same field set and filtered results") both require CLI filter parity, which the Transport Impact table's "where the CLI surfaces them" parenthetical does not override — dispatch 2's rejection of this same finding was wrong.

No files are deleted. No new top-level modules.

## Interfaces & contracts

### `ddb-core/src/app_contract/commands.rs`

```rust
/// Shared pagination policy, decoded once from a transport's raw
/// (possibly absent) limit/offset. Replaces the 4 divergent adapter
/// pagination copies for search; a future list-contract PRD may reuse it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageRequest {
    pub limit: usize,
    pub offset: usize,
}

impl PageRequest {
    /// Applied when the transport omits `limit`.
    pub const DEFAULT_LIMIT: usize = 50;
    /// Rejected, not clamped, when a transport's `limit` exceeds this.
    pub const MAX_LIMIT: usize = 10_000;

    /// Decode a transport's raw limit/offset into a validated `PageRequest`.
    ///
    /// - `limit` `None` -> `DEFAULT_LIMIT` (50).
    /// - `limit` `Some(l)` with `l <= 0` or `l > MAX_LIMIT` (10,000) ->
    ///   `Err(DoogatError::Validation(..))`.
    /// - `offset` `None` -> `0`.
    /// - `offset` `Some(o)` with `o < 0` -> `Err(DoogatError::Validation(..))`.
    /// - Either value failing `i64 -> usize` conversion -> `Err(DoogatError::Validation(..))`.
    ///   Bounded in practice by `MAX_LIMIT` and by each adapter's own
    ///   overflow-checked decode (e.g. REST's checked `(page-1)*per_page`),
    ///   but checked explicitly here too rather than assumed safe.
    ///
    /// Never clamps silently — every out-of-range input is a hard error.
    pub fn decode(
        limit: Option<i64>,
        offset: Option<i64>,
    ) -> crate::error::Result<Self> { /* ... */ }
}

/// Command to run a full-text search against the SQLite FTS5 index.
/// `query` is passed through the search-query normalizer. `types`/`tag`
/// mirror `crate::types::SearchFilters` (`where_filters` stays a GraphQL
/// specialization per 00159/00182 and is not part of this command).
#[derive(Debug, Clone)]
pub struct SearchCommand {
    pub query: String,
    pub page: PageRequest,
    pub types: Option<Vec<String>>,
    pub tag: Option<String>,
}
```

`PageRequest::decode`'s error type is `crate::error::DoogatError::Validation(String)` — the same variant REST's existing `parse_list_params` already uses for its own param errors (`rest.rs:229,246,255`), so the new path introduces no new error code and classifies as `AppErrorCategory::InvalidInput` / HTTP 400 via the existing `VALIDATION_ERROR` policy row (`app_contract/error.rs`).

### `ddb-core/src/app_contract/results.rs`

```rust
/// Returned by a search operation. `items` is a page of the canonical core
/// `SearchResult` (10 fields: id, title, path, snippet, rank, updated_at,
/// tags, doogat_type, fields, created_at) — not `ParsedDoogat`, which lacks
/// snippet/rank/tags/doogat_type/fields/created_at.
#[derive(Debug, Clone)]
pub struct AppSearchResult {
    pub items: Vec<crate::types::SearchResult>,
    pub total: usize,
    pub query: String,
}
```

`ddb-core/src/app_contract/mod.rs`: add `PageRequest` to the existing `pub use commands::{...}` line.

### `ddb-core/src/service/search.rs`

```rust
/// App facade entrypoint: run a search from a `SearchCommand`. Mirrors
/// `DoogatService::create`/`update` (capture `ensure_fresh` warnings, delegate
/// to the existing filtered search, wrap in `AppOutput`). Named distinctly
/// from the pre-existing `search(&self, query: &str)` (service/search.rs:15),
/// which stays for its existing callers (tests, FFI's unfiltered `search`,
/// `ffi/driver.rs:98`).
pub fn search_command(&self, cmd: SearchCommand) -> Result<AppOutput<AppSearchResult>> {
    let reindex_warnings = self.ensure_fresh()?;
    let filters = SearchFilters {
        types: cmd.types,
        tag: cmd.tag,
        where_filters: None,
    };
    let result = self.search_paginated_filtered(
        &cmd.query,
        cmd.page.limit,
        cmd.page.offset,
        &filters,
    )?;
    Ok(AppOutput {
        value: AppSearchResult {
            items: result.hits,
            total: result.total_count,
            query: cmd.query,
        },
        warnings: summarize_reindex_warnings(reindex_warnings).into_iter().collect(),
    })
}
```

Needs `use crate::app_contract::{AppOutput, AppSearchResult, SearchCommand, summarize_reindex_warnings};` added to this file's existing `use crate::types::{..., SearchFilters, ...}` (already imported).

### `ddb-core/src/ffi/records.rs`

```rust
/// One extra-field key/value pair. UniFFI has no first-class map support in
/// this crate (no prior Record here uses one); a repeated-pair record is the
/// zero-new-dependency shape, consistent with every other FFI record here
/// being flat scalars/Vecs.
#[derive(uniffi::Record)]
pub struct FieldEntry {
    pub key: String,
    pub value: String,
}

/// FFI-safe search result — widened to all 10 core `SearchResult` fields
/// (was 6: id, title, path, snippet, rank, updated_at; adds tags,
/// doogat_type, fields, created_at).
#[derive(uniffi::Record)]
pub struct SearchResult {
    pub id: String,
    pub title: String,
    pub path: String,
    pub snippet: String,
    pub rank: f64,
    pub updated_at: String,
    pub tags: Vec<String>,
    pub doogat_type: Option<String>,
    pub fields: Vec<FieldEntry>,
    pub created_at: Option<String>,
}

impl From<crate::types::SearchResult> for SearchResult {
    fn from(r: crate::types::SearchResult) -> Self {
        Self {
            id: r.id,
            title: r.title,
            path: r.path,
            snippet: r.snippet,
            rank: r.rank,
            updated_at: r.updated_at,
            tags: r.tags,
            doogat_type: r.doogat_type,
            fields: r
                .fields
                .unwrap_or_default()
                .into_iter()
                .map(|(key, value)| FieldEntry { key, value })
                .collect(),
            created_at: r.created_at,
        }
    }
}
```

`PaginatedSearchResult`'s own `From` impl (`records.rs:162-169`) is unchanged — it already maps `hits` element-wise via `Into::into`, so widening the element `From` is the only edit needed.

### `ddb-core/src/ffi/driver.rs`

```rust
/// Filtered, paginated FFI search. Unlike `search`/`search_paginated`
/// (unfiltered, kept as-is for existing callers), this decodes into
/// `SearchCommand`/`PageRequest` and returns the full 10-field hit shape.
/// `limit`/`offset` are `Option<u32>`; `None` -> `PageRequest`'s own default
/// (50) / `0`. An out-of-range `Some(limit)` or the type/tag combination
/// failing validation surfaces as `DdbError` (existing `From<DoogatError>`).
pub fn search_filtered(
    &self,
    query: String,
    limit: Option<u32>,
    offset: Option<u32>,
    types: Option<Vec<String>>,
    tag: Option<String>,
) -> Result<PaginatedSearchResult, DdbError> {
    self.with_service(|svc| {
        let cmd = SearchCommand {
            query,
            page: PageRequest::decode(
                limit.map(i64::from),
                offset.map(i64::from),
            )?,
            types,
            tag,
        };
        let output = svc.search_command(cmd)?;
        Ok(crate::ffi::records::PaginatedSearchResult {
            hits: output.value.items.into_iter().map(Into::into).collect(),
            total_count: output.value.total as u64,
        })
    })
}
```

Existing `search_paginated` (`driver.rs:103-115`) is reimplemented, same signature, to close the PRD's own named drift (its provenance note cites this exact method as "uncapped"):

```rust
pub fn search_paginated(
    &self,
    query: String,
    limit: u32,
    offset: u32,
) -> Result<PaginatedSearchResult, DdbError> {
    self.with_service(|svc| {
        let cmd = SearchCommand {
            query,
            page: PageRequest::decode(Some(i64::from(limit)), Some(i64::from(offset)))?,
            types: None,
            tag: None,
        };
        let output = svc.search_command(cmd)?;
        Ok(crate::ffi::records::PaginatedSearchResult {
            hits: output.value.items.into_iter().map(Into::into).collect(),
            total_count: output.value.total as u64,
        })
    })
}
```

Every existing caller keeps compiling against the same `(query, limit, offset) -> Result<PaginatedSearchResult, DdbError>` signature; behavior changes only for the out-of-range `limit`/`offset` values this PRD exists to stop accepting silently (a `limit` of 0 or over 10,000, which previously ran unbounded, now errors). The bare unfiltered `search(&self, query: &str) -> Vec<SearchResult>` (no limit/offset param at all, `driver.rs:96-101`) is NOT touched — it has no pagination to validate, so it is not an instance of the named drift.

(`with_service<T, F: FnOnce(&DoogatService) -> Result<T, DdbError>>` (`driver.rs:25-33`) already returns `Result<T, DdbError>` directly — no trailing `.map_err(DdbError::from)` after the call, matching every existing method in this file (`create_doogat` at `:66-71`, `search_paginated` at `:103-115`); a redundant identity `.map_err` here would be a `clippy::useless_conversion` hit against the mandatory Tier-1 `cargo clippy --workspace --all-targets` gate. `PageRequest::decode(..)?` and `svc.search_command(cmd)?` both produce `DoogatError`, converted to `DdbError` via the closure's own return type and the existing `impl From<DoogatError> for DdbError` (`ffi/records.rs:72`), same as every other closure here.) Needs `SearchCommand`, `PageRequest` added to this file's existing `use crate::app_contract::...`-style import (today it imports result/record types from `crate::types`/local records; add `use crate::app_contract::{PageRequest, SearchCommand};`).

### `ddb-server/src/read_pool.rs`

```rust
/// Thin wrapper over `DoogatService::search_command`, same pattern as the
/// existing unfiltered `search` method above. GraphQL and REST reach the
/// service only through `ReadPool` (never directly), so this wrapper is what
/// lets them actually call the new facade instead of hand-rolling
/// `search_paginated_filtered` the way `search` above does.
pub async fn search_command(
    &self,
    cmd: ddb_core::app_contract::SearchCommand,
) -> Result<ddb_core::app_contract::AppOutput<ddb_core::app_contract::AppSearchResult>> {
    self.with_service(move |svc| svc.search_command(cmd)).await
}
```

(`Result<T>` here is this file's existing local alias, `std::result::Result<T, DoogatError>` — `search_command`'s `crate::error::Result<AppOutput<AppSearchResult>>` is the same `DoogatError`-erroring type, so no new conversion is needed, matching `search`/`get_doogat`/every other method in this `impl ReadPool` block.)

### `ddb-server/src/schema/queries.rs` — `search` resolver

Replace the body from `let limit = validate_limit(&ctx)?.unwrap_or(20) as usize;` through the `pool.search(q, limit, offset, filters)` call with:

```rust
let q = ctx.args.try_get("query")?.string()?.to_string();
let raw_limit = ctx.args.get("limit").and_then(|v| v.i64().ok());
let raw_offset = ctx.args.get("offset").and_then(|v| v.i64().ok());
let page = PageRequest::decode(raw_limit, raw_offset).map_err(to_graphql_error)?;
// types/tag/where_filters extraction unchanged (lines 149-200); where_filters
// stays GraphQL-only (per 00159/00182) and is applied below, outside SearchCommand.
let normalized = search_query::normalize(&q);
let cmd = SearchCommand { query: q, page, types: types.clone(), tag: tag.clone() };
let output = pool.search_command(cmd).await.map_err(to_graphql_error)?;
// `where_filters` (GraphQL-only, not part of SearchCommand) still needs applying:
// construct SearchFilters{ types, tag, where_filters } and call pool.search(...)
// instead of pool.search_command(cmd) whenever where_filters is Some(_); the
// SearchCommand/search_command path above covers the type/tag-only case, which
// is every other call. (Named explicitly here, not glossed over: this is the
// one GraphQL-only fork the plan-tasks task for this resolver must implement.)
let result = ddb_core::types::PaginatedSearchResult {
    hits: output.value.items,
    total_count: output.value.total,
};
```

Default limit for GraphQL `search` changes from 20 to `PageRequest::DEFAULT_LIMIT` (50) — see Risks. The pre-existing `validate_limit`/`validate_offset`/`check_limit`/`check_offset` helpers (`base_types.rs:1034-1052`) are untouched and keep serving every other resolver (`doogats`, `sql`, etc.) — this PRD does not migrate those.

**`where_filters` fork, spelled out precisely (plan-tasks must not drop this):** `SearchCommand` carries only `types`/`tag` (per Risks' scoped boundary). When the GraphQL caller supplies `where` filters, the resolver must NOT call `pool.search_command` (which has no slot for them) — it falls back to building `SearchFilters { types, tag, where_filters }` directly and calling the existing `pool.search(q, page.limit, page.offset, filters)`, exactly as today, just with `page.limit`/`page.offset` now coming from `PageRequest::decode` instead of `validate_limit`/`validate_offset`. When `where` is absent (`where_filters: None`), the resolver takes the `SearchCommand`/`pool.search_command` path shown above. Both branches use the same decoded `page`, so the limit/offset policy is unified either way; only the filter-shape routing forks.

### `ddb-server/src/rest.rs`

New helper, replacing `handle_search`'s `limit`/`offset` params with raw-request decoding (so search's pagination stops sharing `parse_list_params`'s 1–200 clamp):

```rust
/// Decode a REST search request's `page`/`per_page` into a `PageRequest`,
/// via the shared core policy (no REST-specific clamp). `page` is 1-based;
/// `offset = checked (page - 1) * per_page`. `page < 1` or the multiply
/// overflowing is `DoogatError::Validation`, surfaced as 400.
fn decode_search_page(
    raw: &std::collections::HashMap<String, String>,
) -> RestResult<ddb_core::app_contract::PageRequest> {
    let page: i64 = match raw.get("page") {
        Some(v) => v.parse().map_err(|_| {
            rest_error(DoogatError::Validation(format!("invalid page value '{v}'; must be a positive integer")))
        })?,
        None => 1,
    };
    if page < 1 {
        return Err(rest_error(DoogatError::Validation(format!("page must be >= 1, got {page}"))));
    }
    let per_page: Option<i64> = match raw.get("per_page") {
        Some(v) => Some(v.parse().map_err(|_| {
            rest_error(DoogatError::Validation(format!("invalid per_page value '{v}'; must be an integer")))
        })?),
        None => None,
    };
    let per_page_for_offset = per_page.unwrap_or(ddb_core::app_contract::PageRequest::DEFAULT_LIMIT as i64);
    let offset = per_page_for_offset
        .checked_mul(page - 1)
        .ok_or_else(|| rest_error(DoogatError::Validation(format!(
            "per_page ({per_page_for_offset}) * (page - 1) ({}) overflow", page - 1
        ))))?;
    ddb_core::app_contract::PageRequest::decode(per_page, Some(offset)).map_err(rest_error)
}

async fn handle_search(
    read_pool: ReadPool,
    q: String,
    page: ddb_core::app_contract::PageRequest,
    doogat_type: Option<String>,
    tag: Option<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorBody>)> {
    let cmd = ddb_core::app_contract::SearchCommand {
        query: q,
        page,
        types: doogat_type.map(|t| vec![t]),
        tag,
    };
    let output = read_pool.search_command(cmd).await.map_err(rest_error)?;
    let hits: Vec<SearchHit> = output.value.items.into_iter().map(SearchHit::from).collect(); // widened below
    Ok(Json(serde_json::to_value(SearchResponse { data: hits, total_count: output.value.total }).unwrap()))
}
```

REST's search branch has no `where`-filter concept (unlike GraphQL), so it always takes the `SearchCommand`/`search_command` path — no fork needed here, unlike the GraphQL resolver above.

`list_doogats` is reordered to check `raw_params.get("q")` **before** calling `parse_list_params` (today it calls `parse_list_params` unconditionally at `rest.rs:287`, then branches on `params.q` at `:289`):

```rust
async fn list_doogats(
    Extension(read_pool): Extension<ReadPool>,
    Query(raw_params): Query<std::collections::HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorBody>)> {
    if let Some(q) = raw_params.get("q").cloned() {
        let page = decode_search_page(&raw_params)?;
        let doogat_type = raw_params.get("type").cloned();
        let tag = raw_params.get("tag").cloned();
        return handle_search(read_pool, q, page, doogat_type, tag).await;
    }
    let params = parse_list_params(&raw_params)?;
    fetch_paginated_list(read_pool, params).await
}
```

`parse_list_params`/`fetch_paginated_list`'s existing 1–200 clamp is untouched — it now only ever runs for the no-`q` (plain list) branch, exactly as before this PRD for that branch. `SearchHit` widens (replacing the current 4-field struct, `rest.rs:128-134`):

```rust
#[derive(Serialize)]
struct SearchHit {
    id: String,
    title: String,
    path: String,
    snippet: String,
    rank: f64,
    updated_at: String,
    tags: Vec<String>,
    #[serde(rename = "type")]
    doogat_type: Option<String>,
    #[serde(skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    fields: std::collections::BTreeMap<String, String>,
    created_at: Option<String>,
}

impl From<ddb_core::types::SearchResult> for SearchHit {
    fn from(r: ddb_core::types::SearchResult) -> Self {
        Self {
            id: r.id, title: r.title, path: r.path, snippet: r.snippet, rank: r.rank,
            updated_at: r.updated_at, tags: r.tags, doogat_type: r.doogat_type,
            fields: r.fields.unwrap_or_default(), created_at: r.created_at,
        }
    }
}
```

(REST is already serde-JSON-native, so `fields` stays a real `BTreeMap` here — unlike the FFI record, no UniFFI constraint applies.)

### `ddb-cli/src/commands/query.rs`

```rust
pub(crate) fn search(
    repo: &std::path::Path,
    query: &str,
    limit: Option<usize>,
    offset: Option<usize>,
    doogat_type: Option<String>,
    tag: Option<String>,
) -> ddb_core::error::Result<()> {
    let svc = DoogatService::open(repo)?;
    let page = ddb_core::app_contract::PageRequest::decode(
        limit.map(|l| l as i64),
        offset.map(|o| o as i64),
    )?;
    let cmd = ddb_core::app_contract::SearchCommand {
        query: query.to_string(),
        page,
        types: doogat_type.map(|t| vec![t]),
        tag,
    };
    let output = svc.search_command(cmd)?;
    if output.value.items.is_empty() {
        outln!("no results")?;
    } else {
        let start = page.offset + 1;
        let end = page.offset + output.value.items.len();
        outln!("Showing {start}-{end} of {} results", output.value.total)?;
        for r in &output.value.items {
            let type_suffix = r.doogat_type.as_deref().map(|t| format!(" [{t}]")).unwrap_or_default();
            outln!("[{}] {}{} ({})", r.id, r.title, type_suffix, r.path)?;
            outln!("  {}", r.snippet)?;
            outln!("  rank: {:.4}  updated: {}", r.rank, r.updated_at)?;
            if let Some(created) = &r.created_at {
                outln!("  created: {created}")?;
            }
            if !r.tags.is_empty() {
                outln!("  tags: {}", r.tags.join(", "))?;
            }
            if let Some(fields) = &r.fields {
                if !fields.is_empty() {
                    let rendered = fields.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join(", ");
                    outln!("  fields: {rendered}")?;
                }
            }
        }
    }
    Ok(())
}
```

All 10 `SearchResult` fields are now printed (id, title, type via `type_suffix`, path, snippet, rank, updated_at, created_at, tags, fields) — review dispatch 2 (codex) correctly flagged the earlier sketch for dropping rank/updated_at/fields/created_at, which contradicted this doc's own Data-flow claim that no transport drops a field. `main.rs`'s `Search` clap variant changes `limit`/`offset` to `Option<usize>` with no default (`#[arg(long)] limit: Option<usize>`), so an omitted flag reaches `PageRequest::decode` as `None` and gets the shared `DEFAULT_LIMIT` (50) — review dispatch 2 also flagged that clap's old hardcoded `default_value = "20"` would otherwise keep CLI's default silently divergent from the other three transports.

## Data flow

1. A transport parses its own request shape (HTTP query string, GraphQL arguments, CLI clap args, FFI method params) into raw `Option<i64>`/`String` primitives.
2. The transport calls `PageRequest::decode(raw_limit, raw_offset)` — the ONE place limit/offset get validated and defaulted. A validation failure returns immediately as that transport's error shape (`AppError`/`DoogatError` → HTTP 400 / GraphQL error / CLI stderr / `DdbError`).
3. The transport builds a `SearchCommand { query, page, types, tag }` and calls `DoogatService::search_command(cmd)` — directly for CLI/FFI (which hold a `DoogatService` already), or via the new `ReadPool::search_command` wrapper for GraphQL/REST (which only ever reach the service through `ReadPool`). GraphQL's `where`-filter case is the one exception: it has no `SearchCommand` slot, so it builds `SearchFilters{types, tag, where_filters}` directly and calls the pre-existing `ReadPool::search` instead (see the `search` resolver's Interfaces & contracts entry) — every other call, across all four transports, goes through `SearchCommand`.
4. `search_command` calls `ensure_fresh()` (capturing reindex warnings), builds `SearchFilters` from `cmd.types`/`cmd.tag`, and delegates to the existing `search_paginated_filtered(query, page.limit, page.offset, &filters)` → indexer FTS5 query → `PaginatedSearchResult { hits: Vec<SearchResult>, total_count }`.
5. `search_command` wraps that into `AppOutput<AppSearchResult>` (`items = hits`, `total = total_count`, `query` echoed back), with any `REINDEX_SKIPPED_FILES` warning attached.
6. Each transport serializes `AppOutput<AppSearchResult>.value.items` through its own hit shape — GraphQL's `search_hit_to_value` (already 10 fields, unchanged), REST's new `SearchHit::from`, FFI's widened `SearchResult::from`, CLI's print loop — none drop a field; `warnings` is surfaced per transport's existing `AppWarning` convention (GraphQL/REST already do this for create/update; CLI/FFI's own warning surfacing is out of scope here — search's facade is additive, so the transports that do not yet forward `AppOutput.warnings` on other verbs are not newly obligated to do so by this PRD; only the field-dropping and filter-dropping are closed).

## Reuse inventory

- `crate::types::SearchFilters` / `SearchFieldFilter` / `SearchFieldOp` (`ddb-core/src/types/doogat.rs:396-414`) — already the exact types/tag/where shape; `SearchCommand.types`/`.tag` map straight onto `SearchFilters.types`/`.tag`. Reused as-is, not redefined.
- `DoogatService::search_paginated_filtered` (`service/search.rs:30-40`) — already does filtered pagination against the index; the new facade is a thin AppOutput wrapper over it, not a reimplementation.
- `crate::app_contract::{AppOutput, AppWarning, summarize_reindex_warnings}` (`app_contract/output.rs`) — the exact envelope `create`/`update` already use; `search_command` reuses it verbatim rather than inventing a parallel search-specific envelope.
- `DoogatError::Validation` (`error.rs:60-61`) and its `AppErrorCategory::InvalidInput`/`VALIDATION_ERROR` policy row (`app_contract/error.rs:72-76`) — reused for `PageRequest::decode`'s errors rather than minting a new error code; REST's existing `parse_list_params` already constructs this exact variant for its own param errors (`rest.rs:229-233,246-249,254-258`), so this is the established idiom, not a new one.
- `search_query::normalize`/`validate_and_compile` — unchanged; still the sole place query-string validity is decided (empty-query `BadRequest`, malformed syntax), `SearchCommand` does not re-validate the query string itself.
- Greps tried (reuse sweep for "pagination"/"page"/"limit clamp"): `rg -n "clamp\(" ddb-server ddb-core` (REST per_page, GraphQL nowhere), `rg -n "MAX_LIMIT|DEFAULT_LIMIT" ddb-core ddb-server` (only `base_types.rs`'s `MAX_LIMIT`, confirming no existing shared `PageRequest`-like type), `rg -n "struct.*Page(Request|Params)" ddb-core ddb-server ddb-cli` (nothing found — confirms this is new, not a rename of an existing type).

## Alternatives considered

1. **Unify REST's list AND search pagination under `PageRequest` in one pass** (touch `parse_list_params`/`fetch_paginated_list` too, raising the plain-list per_page ceiling from 200 to 10,000 and switching its clamp to a hard error). Rejected for this PRD: it is a real behavior change to an endpoint (`GET /doogats` without `q`) that this PRD's title, Success Metrics, and Transport Impact table never mention, and it would couple an unrelated list-pagination-parity PRD's scope into this one. `PageRequest` is written generically so that future PRD is a drop-in, not a rewrite.
2. **Smallest-diff version: leave `SearchCommand.limit`/`.offset` as bare `Option<usize>` (today's shape) and inline a `default().min(MAX).max(1)`-style clamp at each call site**, skipping `PageRequest` entirely. Rejected: this is exactly the 4-divergent-copies problem restated with one extra command field; it does not close the "never clamp silently" Success Metric (REST's silent 200 clamp is the named drift to fix), and 00180's NoSQL cap has nothing shared to consume. **Chosen design is one struct larger than this (`PageRequest`) — the extra size buys a single, independently testable decode function with one validation story (reject, never clamp) that 5 call sites (4 transports + 00180) share instead of re-deriving.**
3. **Give `search_command` a different name scheme, e.g. rename the legacy unfiltered `search`/`search_paginated` and take the bare `search` name for the new facade.** Rejected: `search(&str)`/`search_paginated` have real callers today (service tests, CLI's own unit tests via `svc.search(...)`, `ffi/driver.rs:96-115`'s unfiltered methods) that are out of this PRD's scope to touch; renaming them to free up the name is a larger, unrelated diff for a cosmetic win. `search_command` is unambiguous and matches the `*Command` naming already used by the type it takes.

## Risks & edge cases

- **Public response-shape change** (REST `SearchHit`, FFI `SearchResult`): both widen additively (new fields only, no renames/removals) — existing clients ignoring unknown fields are unaffected; FFI binding regeneration is required (Phase 1 task already covers Swift/Kotlin regen+smoke).
- **GraphQL `search`'s default limit changes from 20 to 50** (`PageRequest::DEFAULT_LIMIT`): a caller relying on the undocumented 20-default now gets up to 50 hits instead. Documented as a compat note (Phase 2 docs task) alongside the already-planned REST-200→10,000 note.
- **REST's search-branch pagination now rejects instead of clamping, AND the ceiling rises** (`per_page` out of `[1, 10_000]`, or a non-positive `page`): a client sending `per_page=500` today silently got clamped to 200; it is now honored in full (500 ≤ 10,000). A client sending `per_page=10001` today also got silently clamped to 200; it now gets a 400 instead. This is the PRD's named Risk ("REST over-max behavior changes") and is scoped to the search branch only (see Module placement's Scope boundary) — the plain list branch keeps its current `[1,200]` silent clamp. (Corrected in review dispatch 2: an earlier draft of this bullet used `per_page=500` as the now-rejected example, which is wrong since 500 is within the new 10,000 ceiling.)
- **Likely next changes this design should not box in:**
  1. A follow-up list-contract PRD unifying `doogats`/`GET /doogats` (no `q`) pagination onto the same `PageRequest` — enabled, not blocked, by keeping `PageRequest` generic (no `SearchCommand`-specific fields on it).
  2. 00180's NoSQL scan cap consuming `PageRequest` directly for its own limit — `PageRequest::decode`/`DEFAULT_LIMIT`/`MAX_LIMIT` are `pub`, reachable from `ddb-server`'s nosql module with no further exposure needed.
  3. A relation `where`-filter widening beyond GraphQL (per 00159/00182) — `SearchCommand` deliberately has no `where_filters` field, so adding one later is additive, not a breaking signature change.
- **FFI's `FieldEntry` repeated-pair shape**: if a future PRD needs richer typed field values (not just strings) in an FFI search hit, `FieldEntry.value: String` will need revisiting (today's core `SearchResult.fields` is already `BTreeMap<String, String>`, so this is not a new limitation introduced here — it inherits the core type's existing string-only shape).
- **The bare unfiltered core/FFI `search(&self, query: &str)` (no limit/offset param) stays outside `SearchCommand`/`PageRequest` by design, not by omission.** It has no pagination to validate and is not named in the PRD's own drift description (which cites `ffi/driver.rs:103-115` — `search_paginated`, not `search` — as the uncapped method). `search_paginated` itself, by contrast, IS reimplemented to route through `PageRequest`/`search_command` (see Interfaces & contracts, FFI section) — review dispatch 3 (codex verification) correctly caught that dispatch 2's rejection of "legacy FFI search methods bypass the contract" conflated these two methods; the fix routes the one the PRD actually names.

## Test strategy outline

Per-task local gate stays `cargo build` / `cargo clippy --workspace --all-targets` / `cargo test-ci`, colocated unit tests included (per CLAUDE.md / Test Strategy in the PRD). Author (not locally execute) the e2e/conformance scenarios; their execution is Tier 2 CI.

- **`PageRequest::decode` unit tests** (colocated in `commands.rs`, `#[cfg(test)]`): defaults (both `None`), both bounds (`limit` 1 and 10,000 accepted), every rejection (`limit` 0, negative, 10,001; `offset` negative), and that a successful decode's `limit`/`offset` round-trip exactly.
- **`search_command` unit tests** (colocated in `service/search.rs` or its existing test module): all 10 hit fields present and unmodified from the index's `SearchResult`; `types`/`tag` filters applied (reuses existing `search_paginated_filtered` filter test fixtures); `REINDEX_SKIPPED_FILES` warning surfaces when `ensure_fresh` reports skips (mirrors `update`'s existing warning test).
- **REST**: `decode_search_page` unit tests (page<1, per_page out of range, overflow on an extreme `page`×`per_page`); an integration-style test asserting a `type`/`tag`-filtered REST search returns filtered hits carrying `tags`/`doogat_type`/`fields`/`created_at` (the PRD's own Phase 1 acceptance).
- **GraphQL**: existing search tests extended to assert the 50-default and the unchanged filter behavior.
- **FFI**: a test for `search_filtered` filtering by type/tag and returning the full field set; a test that the reimplemented `search_paginated` now rejects an out-of-range `limit` (0, or over 10,000) instead of running unbounded; binding regen + Swift/Kotlin smoke (existing convention for FFI shape changes).
- **CLI**: a test that `--type`/`--tag` filter CLI search results (exercising the new flags end to end); a test that omitting `--limit`/`--offset` reaches `PageRequest`'s 50/0 defaults rather than clap's old 20/0.
- **Conformance harness** (Phase 2 task, already scoped in the PRD): extend `StepOp::Search` with the type/tag filter and a full-field-set assertion across CLI/GraphQL/REST — now implementable end to end since CLI has the filter flags to drive it.
- **e2e** (Rust harness, `tests/e2e/`): filtered REST/FFI search scenarios, full hit shape — per the PRD's own Phase 2 task; author only, Tier 2 executes.
- **Critical scenarios** (from the PRD, unchanged): happy path (`type=project` search, GraphQL+REST identical ids+full fields), edge case (FFI tag filter returns filtered not unfiltered hits), error case (REST type filter matching nothing returns an empty list, not an unfiltered fallback).

## Review log

Dispatch 1 (claude) findings and resolution:

- BLOCKER (fixed): GraphQL/REST never actually routed through `SearchCommand`/`search_command` — both called `ReadPool::search`/`pool.search` directly, hand-rolling the same `search_paginated_filtered` call the new facade wraps, contradicting the PRD's "single entrypoint" success metric. Fixed by adding `ReadPool::search_command` and rewriting both the GraphQL resolver and REST's `handle_search` to build a `SearchCommand` and call it (GraphQL's `where`-filter case is the one documented exception, since `SearchCommand` has no slot for it).
- NON-BLOCKER (fixed): the new FFI `search_filtered` had a redundant trailing `.map_err(DdbError::from)` after `with_service(...)` (already `Result<T, DdbError>`), inconsistent with every other method in the file and a likely `clippy::useless_conversion` hit. Removed.
- QUESTION (fixed): `decode_search_page`'s overflow error message said "page * per_page overflow" but the actual computed expression is `per_page * (page - 1)`; and `PageRequest::decode`'s doc comment overstated i64→usize conversion safety on 32-bit targets. Both corrected.

dispatch 1 (claude): cardinal-sin 0, blocker 1, non-blocker 1, question 1

Dispatch 2 (codex) findings and resolution — codex's sandbox could not read source files this run (shell/file-reader startup blocked), so its review is against the design text only; findings were still checked against the actual contract described in this doc:

- QUESTION (acknowledged, not a design defect): codex flagged that it could not verify findings against source. Noted; the findings below were still cross-checked against this doc's own content (which is self-consistent enough to judge without re-reading the repo for most of them), and nothing here hinges on a source-level fact codex couldn't check.
- BLOCKER (fixed): CLI's search printed only 6 of 10 `SearchResult` fields (id, title, type, path, snippet, tags), contradicting Data flow step 6's claim that no transport drops a field. Fixed: CLI now prints all 10 (adds rank, updated_at, created_at, fields).
- BLOCKER (fixed): CLI's `limit`/`offset` kept clap's old hardcoded defaults (20/0) instead of becoming `Option<usize>`, so an omitted CLI flag never reached `PageRequest::decode` as `None` and silently stayed on 20 while GraphQL/REST/FFI moved to 50 — a real, if minor, instance of exactly the "4 divergent defaults" problem this PRD exists to close. Fixed: `limit`/`offset` are now `Option<usize>` in both the CLI command function and the `main.rs` clap variant.
- BLOCKER (rejected, with rationale, not fixed): "CLI cannot execute the required filtered workflow" (no type/tag CLI flags). Rejected: the PRD's own Transport Impact table says CLI exposes filters "where the CLI surfaces them" — today it doesn't, and this PRD doesn't add CLI filter flags (Module placement already states this explicitly, citing the same PRD line). Adding CLI filter flags is a larger, separate UI change this PRD never asked for; the conformance harness task targets CLI/GraphQL/REST field-set parity, not CLI filter parity.
- BLOCKER (rejected, with rationale, not fixed): "Existing public FFI search methods bypass the contract and cap" (legacy unfiltered `search`/`search_paginated` kept as-is). Rejected: recorded as its own Risks bullet above — this is the same already-justified decision as Alternative 3 (core's own legacy `search(&str)` is likewise left untouched), not an oversight.
- NON-BLOCKER (fixed): the Risks bullet's `per_page=500` example was wrong — 500 is within the new 10,000 ceiling, so it is now honored in full, not rejected. Corrected the example to use 10,001 for the now-rejected case and 500 for the now-honored-in-full case.

dispatch 2 (codex): cardinal-sin 0, blocker 4, non-blocker 1, question 1

Dispatch 3 (codex verification, ran without source access — judged from doc self-consistency against the PRD text) re-raised both blockers dispatch 2 had rejected, and this time the finding held up on closer reading:

- BLOCKER (fixed, dispatch 2's rejection reversed): CLI filter scope. Dispatch 2 rejected "CLI needs type/tag filters" citing the Transport Impact table's "where the CLI surfaces them" parenthetical. Dispatch 3 pointed out that both the Success Metrics ("Same query across CLI/GraphQL/REST/FFI returns the same field set and filtered results") and the Phase 2 conformance task's own text ("the same query + filter across CLI/GraphQL/REST returns the same hit ids and field set") require CLI filter parity, and the Transport Impact parenthetical doesn't override that. Fixed: added `--type`/`--tag` CLI flags, forwarded into `SearchCommand`.
- BLOCKER (partially fixed, dispatch 2's rejection reversed in part): "legacy FFI/service search methods bypass the contract." Dispatch 3's finding conflated two different pre-existing methods; splitting them resolved which half is real: `search_paginated` (`ffi/driver.rs:103-115`) is the exact method the PRD's own provenance note names as "uncapped" — it is reimplemented in this design to route through `PageRequest`/`search_command`, closing that drift for real while keeping its signature. The bare unfiltered `search(&str)` (no limit/offset at all) is not an instance of this drift (nothing to validate) and correctly stays untouched, same as core's `search(&str)`. GraphQL's `where`-filter branch still calls `pool.search` directly rather than `search_command`, but it still decodes through `PageRequest` first — the shared *pagination policy* success metric holds on that branch; only the single-struct-entrypoint convenience is traded off for a filter shape (`where_filters`) the PRD itself scopes out of `SearchCommand` (per 00159/00182). Recorded as a kept, narrower exception, not re-opened.

dispatch 3 (codex): cardinal-sin 0, blocker 2, non-blocker 0, question 0
