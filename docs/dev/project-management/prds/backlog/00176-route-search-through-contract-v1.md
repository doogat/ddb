---
default_model: opus
model_tier_rationale: cross-cutting shape change - SearchCommand, PageRequest, and one hit shape across four transports; PageRequest is consumed by 00180
---

# Route search through the app contract; fix cross-transport search drift

**Priority:** P2 · **Roadmap:** Phase 2 (app contract) · **Depends on:** 00172 (closure-message actor)

> **Provenance:** the `PageRequest` policy was added by the 2026-07-16 assessment (L11) and folded into the body's Features/tasks at the 2026-08-14 backlog review. Divergence re-verified at `d77ad58`: GraphQL errors above `MAX_LIMIT` 10,000 yet leaves the `doogats` default unbounded (`base_types.rs:1030-1080`, `queries.rs:95`), REST silently clamps to 200 (`rest.rs:253-256,:278`), NoSQL scan has no limit parameter at all (`utility.rs:132-158`), FFI is uncapped (`ffi/driver.rs:103-115`; `list_doogats` at `:163`). The NoSQL hard cap itself lands in 00180; this PRD defines the shared policy those adapters consume.

## Overview

### Problem Statement
`SearchCommand` (`ddb-core/src/app_contract/commands.rs:85`) and its result DTO `AppSearchResult` (`results.rs:57`) are defined but unconsumed, and in their absence search has drifted into four different products:

- **Filter drift**: GraphQL builds real `SearchFilters { types, tag, where_filters }` (`schema/queries.rs:202-206`) and calls the filtered path (`pool.search(...)` at `queries.rs:209-210` → `search_paginated_filtered`, `read_pool.rs:92`); the filter args are parsed at `queries.rs:149-162`. REST calls the SAME method but hardcodes `SearchFilters::default()` (`rest.rs:305`), silently DROPPING type/tag/where filters. CLI (`ddb-cli/src/commands/query.rs:48`) and FFI (`ddb-core/src/ffi/driver.rs:96-115`) call the unfiltered `search_paginated`, so neither can filter at all.
- **Result-shape drift**: the core `SearchResult` has 10 fields (`ddb-core/src/types/doogat.rs:354-365`: id, title, path, snippet, rank, updated_at, tags, doogat_type, fields, created_at); GraphQL already serializes all 10 (`search_hit_to_value`, `schema/base_types.rs:203-245`), the CLI prints 4 (`query.rs:55-58`). FFI's `SearchResult` record exposes 6 (`ffi/records.rs:132-153`, dropping tags, doogat_type, fields, created_at). REST's `SearchHit` exposes 4 (`rest.rs:129-134`: id, title, snippet, rank). Each transport re-derives a different subset, so porting a query between transports yields a different hit shape and a different filter contract.

The contract names the right things (`SearchCommand` + `AppSearchResult`) but `AppSearchResult.items` is `Vec<ParsedDoogat>` (`results.rs:58`), which carries none of snippet/rank/tags/doogat_type/fields/created_at — it must be redefined as a page of canonical core SearchResult hits, retaining total and query (grounded 2026-09-05); and neither type has a consumer.

### Target Users
Downstream apps (jink) that issue the same search across transports and today maintain per-transport hit-mapping glue and cannot filter on REST/FFI. They need one filter contract and one hit shape everywhere.

### Success Metrics
- `SearchCommand` (extended with the type/tag filters) is the single search entrypoint each adapter (CLI, GraphQL, REST, FFI) decodes into.
- ONE contract `PageRequest` policy (default, max, one over-max rule) replaces the 4 divergent adapter pagination copies; every search-capable adapter decodes through it, and 00180's NoSQL cap consumes it.
- Every transport serializes ONE core hit shape (no per-transport subset); REST and FFI stop dropping fields.
- REST and FFI honor the type/tag filters they currently drop.
- The same query across CLI/GraphQL/REST/FFI returns the same field set and the same filtered results; the PRD 00148 conformance harness asserts it.
- No regression: existing search tests stay green; the wider REST/FFI shape and new filters are additive.

## Functional Decomposition

### Capability: Contract-routed search
#### Feature: SearchCommand as the single entrypoint
- **Description**: each transport decodes its search request (query + type/tag filters + pagination) into `SearchCommand` and renders AppOutput<AppSearchResult> over one page.
- **Inputs**: `query`, optional `limit`/`offset`, and the type/tag filters the transport exposes.
- **Outputs**: AppOutput<AppSearchResult> carrying one page with items/total/query.
- **Behavior**: extend `SearchCommand` with the type/tag filters (relation `where` stays GraphQL-specialized per 00159/00182); the service runs `search_paginated_filtered` behind the command; transports serialize the full hit shape and surface warnings.

#### Feature: One canonical hit shape
- **Description**: transports serialize `AppSearchResult` without dropping fields.
- **Inputs**: the core `SearchResult` (10 fields).
- **Outputs**: `AppSearchResult { items: Vec<SearchResult>, total, query }`, where SearchResult is the existing canonical core ten-field hit. Return one page inside AppOutput, with no nested pages.
- **Behavior**: populate the page once from core hits plus total/query metadata; adapters serialize every hit field and retain page metadata without defining duplicate core result types.

#### Feature: One PageRequest policy
- **Description**: pagination policy (default, max, clamp-or-error rule) is defined ONCE in the app contract as part of the `SearchCommand` decode, replacing the 4 divergent adapter copies.
- **Inputs**: the transport's raw `limit`/`offset` (or absence).
- **Outputs**: `PageRequest { limit, offset }` with default limit 50, maximum 10,000, and default offset 0. Limits must be positive; offsets non-negative. Reject zero/negative/over-max limits, negative offsets, and conversion/arithmetic overflow with InvalidInput; never clamp silently.
- **Behavior**: `SearchCommand` carries the normalized `PageRequest`; GraphQL/REST/CLI/FFI decode through it (GraphQL's unbounded `doogats` default and REST's silent 200 clamp both converge on the policy); 00180's NoSQL scan cap consumes the same policy.

### Capability: REST + FFI filter parity
#### Feature: Type/tag filters on REST and FFI
- **Description**: REST and FFI accept and apply type/tag filters instead of `::default()`/unfiltered.
- **Inputs**: type list + tag on the REST query string / FFI method args.
- **Outputs**: filtered hits.
- **Behavior**: REST populates `SearchFilters` from the request (not `::default()`); FFI gains a filtered search method that decodes into `SearchCommand` with filters.

## Structural Decomposition

### Repository Structure

```
ddb-core/src/app_contract/
├── commands.rs   # Maps to: SearchCommand extended with type/tag filters + the one PageRequest policy
├── results.rs    # Maps to: AppSearchResult page over canonical core SearchResult hits, with total/query metadata
ddb-core/src/service/
└── search.rs     # Maps to: search(SearchCommand) -> AppOutput<AppSearchResult>
ddb-core/src/ffi/
├── driver.rs     # Maps to: filtered FFI search decoding into SearchCommand
├── records.rs    # Maps to: FFI hit record widened to the full field set
ddb-server/src/
├── schema/queries.rs   # Maps to: GraphQL search decodes into SearchCommand
└── rest.rs       # Maps to: REST search populates filters + serializes the full hit shape
ddb-cli/src/commands/
└── query.rs      # Maps to: CLI search decodes into SearchCommand
```

### Module: service search (ddb-core/src/service/search.rs)
- **Maps to capability**: Contract-routed search
- **Responsibility**: run `SearchCommand` (with filters) and return `AppOutput<AppSearchResult>`.
- **Exports**: `search(SearchCommand) -> AppOutput<...>`

### Module: ffi (ddb-core/src/ffi/driver.rs, records.rs)
- **Maps to capability**: REST + FFI filter parity; one canonical hit shape
- **Responsibility**: expose a filtered FFI search; widen the FFI hit record to the full field set.
- **Exports**: filtered `search`/`search_paginated`; `SearchResult` record with all 10 fields.

### Module: search contract and public adapters (ddb-core/src/app_contract/commands.rs, results.rs; ddb-server/src/schema/queries.rs, schema/base_types.rs, rest.rs; ddb-cli/src/commands/query.rs)
- **Maps to capability**: Contract-routed search; REST + FFI filter parity
- **Responsibility**: define the page and pagination policy once, decode adapter inputs, and preserve all hit fields, page metadata, and warnings.
- **Exports**: SearchCommand, PageRequest, AppSearchResult, and existing search handlers with the unified contract.

## Dependency Graph

### Foundation Layer (Phase 0)
Depends on 00172.

- **SearchCommand (+filters) + service search → AppOutput<AppSearchResult>**: the shared search foundation.

### Core Layer (Phase 1)
- **transport decode/render + REST/FFI filters + widened shapes**: Depends on [service search].

### Integration Layer (Phase 2)
- **conformance + tests + docs + walkthrough**: Depends on [transport decode/render].

## Implementation Phases

### Phase 0: Foundation
**Goal**: make search a contract call.

**Tasks**:
- [ ] Extend SearchCommand with type/tag filters and the shared PageRequest; return AppOutput<AppSearchResult> with items: Vec<core SearchResult>, total, query over search_paginated_filtered (depends on: 00172) - Acceptance: tests pin all ten hit fields and page metadata, applied filters, defaults 50/0, limits 1 and 10,000, and InvalidInput for zero/negative/10,001 limits, negative offsets, or overflow. REST's one-based page/per_page converts with checked (page - 1) * per_page; page below 1 is invalid. Other adapters use checked limit/offset decoding through the same core policy.

**Exit Criteria**: the service returns filtered contract hits.

### Phase 1: Core
**Goal**: route every transport and close the filter/shape gaps.

**Tasks**:
- [ ] Route GraphQL + CLI search through `SearchCommand`; serialize `AppSearchResult` (no field loss) (depends on: Phase 0) - Acceptance: GraphQL/CLI hits carry the full field set; existing filters unchanged.
- [ ] REST: populate `SearchFilters` from the request (replace `SearchFilters::default()` at `rest.rs:305`) and serialize the full hit shape (widen/replace `SearchHit`) (depends on: Phase 0) - Acceptance: a REST search with a type/tag filter returns filtered hits carrying tags/doogat_type/fields/created_at.
- [ ] FFI: add a filtered search decoding into `SearchCommand`; widen the FFI `SearchResult` record to all 10 fields; regenerate bindings (depends on: Phase 0) - Acceptance: an FFI search filters by type/tag and returns the full field set; bindings regenerate clean.

**Exit Criteria**: all four transports share the filter contract and hit shape.

### Phase 2: Integration
**Goal**: pin parity, cover, document.

**Tasks**:
- [ ] Extend the conformance harness's existing `StepOp::Search` (`fixture.rs:58`; runs CLI+GraphQL today with a `{ id title }` selection, `cross_driver_crud.rs:73-95`) with the type/tag filter and a full-field-set assertion: the same query + filter across CLI/GraphQL/REST returns the same hit ids and field set (the harness's REST driver lands in 00175; PRD 00148 — link-ok) (depends on: Phase 1) - Acceptance: the harness asserts identical filtered results + field set.
- [ ] Author the scenarios in the Rust e2e harness (`tests/e2e/`; the shell harness was removed by 00171) — filtered REST/FFI search, full hit shape — update `docs/src/technical/app-contract.md` + `server.md` + `rest-api.md` + `ffi.md` + `indexer.md` + `transport-policy-inventory.md` + CHANGELOG, and add a showboat walkthrough for filtered cross-transport search (depends on: Phase 1) - Acceptance: e2e scenarios green; docs match; walkthrough authored through showboat; CI verification uses the safe wrapper.

**Exit Criteria**: conformance + scenarios + docs + walkthrough landed.

## Test Strategy

**Validation:** per-task local gates are `cargo build`, `cargo clippy --workspace --all-targets`, and `cargo test-ci`, including colocated unit regressions. Author required e2e/integration/conformance scenarios and executable walkthroughs; their heavy execution belongs to Tier 2 CI, not the per-task gate. Retain only CLAUDE.md's conditional e2e run for Claude-routed tasks deleting/replacing code paths. Walkthrough creation uses showboat; any verification uses `dev/bin/safe-showboat-verify`, never the project root. This backlog assumes 00171 has completed the shell-to-Rust harness migration.

### Critical Scenarios
- **Happy path**: `type=project` search on GraphQL and REST → Expected: identical hit ids, each hit carrying all 10 fields.
- **Edge case**: an FFI search with a tag filter → Expected: filtered hits (not the old unfiltered set), full field shape.
- **Error case**: a REST search with a type filter that matches nothing → Expected: an empty hit list (not a silently-unfiltered full result from `::default()`).

## Risks

- **Public response-shape change**: REST `SearchHit` and FFI `SearchResult` widen. Mitigation: additive fields; compatibility note in `rest-api.md`/`ffi.md`/`compatibility-checklist.md` + CHANGELOG; keep existing field names/types stable.
- **REST filter surface is new input**: adding type/tag query params to REST search is a request-shape addition. Mitigation: optional params; absent = current behavior; document.
- **Relation `where` filters stay GraphQL-only**: `SearchCommand` carries type/tag (Guaranteed everywhere); the typed relation where-filter remains a GraphQL specialization (per 00159/00182). Mitigation: scope `SearchCommand` filters to type/tag; document the where-filter boundary.
- **FFI binding regen**: widened record. Mitigation: regenerate + smoke Swift/Kotlin.
- **`service/search.rs:15` already has `pub fn search(&self, query: &str)`**: the contract entrypoint needs a distinct name or replaces it. Mitigation: design decides the identifier; this PRD does not pin it.
- **REST over-max behavior changes** (silent clamp → `InvalidInput`, decided 2026-09-05): a public behavior change. Mitigation: compat note in `rest-api.md`/`compatibility-checklist.md` + CHANGELOG; REST's absent-limit default remains 50; GraphQL's formerly unbounded default becomes 50 and REST's explicit maximum rises from 200 to 10,000; document these changes and update fixtures.

## Transport Impact

- **CLI**: Guaranteed — `ddb query`/search decodes into `SearchCommand`; full hit shape (type/tag filters exposed where the CLI surfaces them).
- **GraphQL**: Guaranteed — the search resolver decodes into `SearchCommand`; unchanged filters, full hit shape.
- **REST**: Guaranteed — `GET /rest/doogats?q=` (the REST search dispatch at `rest.rs:289-291`; there is no `/rest/search` route — `type`/`tag` are already parsed into `ListParams` at `:271-272` and then discarded on the search branch) populates filters from the request and serializes the full hit shape (the two drift fixes); an over-max `per_page` is rejected with `InvalidInput` instead of silently clamped (compat note).
- **FFI**: Guaranteed — filtered `search`/`search_paginated` decode into `SearchCommand`; record widened to all 10 fields.
- **PgWire**: Specialized — full-text/tag search is expressed via SQL `SELECT`/FTS predicates; the typed `SearchCommand` is not its surface.
- **NoSQL HTTP**: Specialized — its `nosql_scan_type`/`nosql_scan_tag` document-scan endpoints are the native equivalent; the FTS `SearchCommand` is not layered onto the read-only scan surface.
- **Cross-interface conformance plan**: the PRD 00148 conformance harness runs the same query + type/tag filter across CLI/GraphQL/REST and asserts identical hit ids and field set; an FFI test pins the filtered FFI search + widened shape; smoke/integration + e2e cover REST/FFI filtering. jink impact: REST/FFI search gain the filters jink needs, and one hit shape deletes jink's per-transport search-hit mapping glue.
