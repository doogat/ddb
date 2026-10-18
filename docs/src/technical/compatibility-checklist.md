# Compatibility Checklist

**Reconstruction notice.** The original §3 (release-readiness and rollback
criteria) of `dev/local/notes/interface-deprecations.md` was deleted and is
unrecoverable — that file was gitignored and never committed. This doc is
re-derived from surviving references in `building-apps.md`, `server.md`,
`transport-policy-inventory.md`, and the conformance harness source. It is not a
verbatim copy of the original text.

**Why this is a separate file.** Response-shape, error-shape, and warning-shape
gates apply equally to every transport's `Guaranteed` capabilities — they are not
specific to GraphQL or the REST API. Embedding this checklist inside `server.md`
or `app-contract.md` would misrepresent its scope.

## Checklist categories

Every high-risk shape change must clear all four gates in its category before
landing.

### Response shape

A change to any `Guaranteed` response field name, type, nesting, or omission on
GraphQL, CLI, or REST is high-risk and requires:

- [ ] A conformance or integration fixture pins the new shape (update the
  existing fixture or add a new assertion; the `crud_baseline` fixture in
  `tests/e2e/conformance/workflows.rs` covers GraphQL and CLI baseline responses).
- [ ] The matching Migration Note in
  `docs/src/guide/building-apps.md` ("Compatibility and Deprecation") is
  updated in the same commit.
- [ ] A CHANGELOG entry lands in the same commit.
- [ ] The change is revertable as a single commit; the revert path is named in
  the PRD task (e.g. "revert: remove `warnings` field from REST create response").

### Error shape

A change to any `Guaranteed` error envelope field, code vocabulary, or HTTP
status on GraphQL, CLI, or REST is high-risk and requires:

- [ ] A conformance or integration fixture pins the new error shape (the
  `crud_baseline` fixture covers GraphQL `extensions.code` and REST
  `{ error, message }` envelopes).
- [ ] The matching Migration Note in
  `docs/src/guide/building-apps.md` ("Compatibility and Deprecation") is
  updated in the same commit.
- [ ] A CHANGELOG entry lands in the same commit.
- [ ] The change is revertable as a single commit; the revert path is named in
  the PRD task.

### Warning shape

A change to any `Guaranteed` warning envelope field, code vocabulary, or
emission path on GraphQL or REST is high-risk and requires:

- [ ] The `warnings_shape_contract` fixture in
  `tests/e2e/conformance/workflows.rs` is updated. Its 6 structural assertions
  pin the GraphQL warning contract: the fixture declares exactly one warning;
  the warning has a non-empty `code`; a non-empty `message`; a non-empty
  structured `fields` map; the fixture targets the GraphQL interface; and the
  fixture id is stable. Any warning-shape change must update every affected
  assertion.
- [ ] The matching Migration Note in
  `docs/src/guide/building-apps.md` ("Compatibility and Deprecation") is
  updated in the same commit.
- [ ] A CHANGELOG entry lands in the same commit.
- [ ] The change is revertable as a single commit; the revert path is named in
  the PRD task.

## High-risk rows

Re-derived from the `Guaranteed` capabilities in the promise/capability matrix
at `docs/src/guide/building-apps.md` ("CRUD baseline"). A shape change is
high-risk when it touches a `Guaranteed` capability on a flagship interface
(GraphQL, CLI, or REST).

| Capability | Interface | Current shape |
|------------|-----------|---------------|
| Create response | GraphQL | `createDoogat` returns the created object with typed fields; `executeSql(INSERT ...)` returns created id in `SqlResult.message`. |
| Create response | CLI | Prints the new doogat id on stdout; exits 0. |
| Create response | REST | `POST /rest/doogats` returns `{ data, warnings }` JSON envelope. |
| Read (single by id) | GraphQL | `doogat(id: ...)` returns the full object or a `NOT_FOUND` error envelope. |
| Read (single by id) | CLI | `ddb read <id>` prints raw Markdown; not-found returns non-zero exit and stderr message. |
| Read (single by id) | REST | `GET /rest/doogats/:id` returns `{ data, warnings }` JSON envelope. |
| Update response | GraphQL | `updateDoogat` returns the updated object; `executeSql(UPDATE ...)` returns affected-row count. |
| Update response | CLI | `ddb update <id> ...` prints the updated id; SQL `UPDATE` via `ddb query` prints affected-row count. |
| Update response | REST | `PUT /rest/doogats/:id` returns `{ data, warnings }` JSON envelope. |
| Delete response | GraphQL | `deleteDoogat` returns confirmation; cascade cleans child rows atomically. |
| Delete response | CLI | `ddb delete <id>` removes the doogat; SQL `DELETE` via `ddb query` prints affected-row count. |
| Delete response | REST | `DELETE /rest/doogats/:id` returns HTTP 204. |
| List | GraphQL | Typed `<type>s(limit, offset)` queries return typed rows; `executeSql(SELECT ...)` returns `SqlResult` rows. |
| List | CLI | `ddb query "SELECT ..."` returns tabular results on stdout. |
| List | REST | `GET /rest/doogats` returns `{ data: [...], pagination: {...} }`. |
| Search | GraphQL | `search(query: ..., types, tag, where, limit, offset)` returns `SearchConnection` hits with all ten core `SearchResult` fields (`id`, `title`, `path`, `snippet`, `rank`, `updated_at`, `tags`, `type`, `fields`, `created_at`); default limit 50, limit outside 1-10,000 is `VALIDATION_ERROR`. |
| Search | CLI | `ddb search "<query>" [--type T] [--tag G] [--limit N] [--offset N]` prints a `Showing a-b of n results` header, then each hit with all ten fields; default limit 50, offset 0. |
| Search | REST | `GET /rest/doogats?q=...[&type=&tag=&page=&per_page=]` returns `{ data: [...], total_count }`, each hit with all ten fields; `per_page` outside 1-10,000 or `page` below 1 is HTTP 400 `VALIDATION_ERROR`. |
| Search | FFI | `search_paginated` / `search_filtered` return `PaginatedSearchResult` with ten-field `SearchResult` records (`fields` as `Vec<FieldEntry>`); limit 0 or above 10,000 is `DdbError::Validation`. |
| Validation error | GraphQL | HTTP 200 with `{ errors: [{ message, extensions: { code } }] }`; codes include `VALIDATION_ERROR`, `NOT_NULL_VIOLATION`, `UNIQUE_VIOLATION`, `REFERENCES_VIOLATION`, `TYPE_NOT_REGISTERED`. |
| Not-found error | GraphQL | `extensions.code == "NOT_FOUND"` in the error envelope. |
| Not-found error | CLI | Non-zero exit and stderr message on read, update, and delete. |
| Not-found error | REST | HTTP 404 + `{ "error": "NOT_FOUND", "message": "..." }`. |

## Compatibility notes: search through the app contract (PRD 00176)

Search now routes through one `SearchCommand` / `PageRequest` contract on CLI,
GraphQL, REST, and FFI (see
[app-contract.md](app-contract.md#search-searchcommand-pagerequest-appsearchresult)).
The shape changes below are public.

- **REST hit shape widened (additive).** `GET /rest/doogats?q=...` hits went from
  four fields (`id`, `title`, `snippet`, `rank`) to ten (adds `path`,
  `updated_at`, `tags`, `type`, `fields`, `created_at`). No field was renamed or
  removed. `fields` is left out when empty. Revert path: narrow `SearchHit` in
  `ddb-server/src/rest.rs` back to four fields.
- **FFI hit record widened (additive).** `SearchResult` went from six fields to
  ten (adds `tags`, `doogat_type`, `fields: Vec<FieldEntry>`, `created_at`), and
  the new `FieldEntry` record exists. Swift/Kotlin bindings must be regenerated.
- **REST and FFI filters now apply.** REST `type` / `tag` on the search branch
  used to be dropped silently, so the response held unfiltered hits. They now
  filter. A filter that matches nothing returns an empty list. FFI gains
  `search_filtered` with the same filters.
- **REST search over-max behavior: clamp → reject.** Search `per_page` used to be
  clamped silently to 1-200. It is now checked against 1-10,000 and an
  out-of-range value returns HTTP 400 `VALIDATION_ERROR`. A `page` below 1 now
  returns 400 instead of being treated as 1. The explicit maximum rises from 200
  to 10,000, so `per_page=500` now returns up to 500 hits. An absent `per_page`
  still gives 50. The plain list branch (no `q`) keeps its 1-200 clamp.
- **FFI `search_paginated` validation.** A `limit` of 0 or above 10,000 used to
  run without bounds. It now fails with `DdbError::Validation`.
- **Default search limit 20 → 50 on GraphQL and CLI.** An omitted `limit`
  (GraphQL) or `--limit` (CLI) now returns up to 50 hits, not 20.
- **CLI search output and flags.** `ddb search` gains `--type` and `--tag` and now
  prints every hit field (type, rank, tags, created/updated, typed fields).

Fixtures that pin these shapes: `tests/e2e/integration_rest_search_filtered.rs`
(REST filters, hit shape, 400 on out-of-range paging), `tests/e2e/smoke_search.rs`
(CLI filters and 50/0 defaults), the `filtered_search` conformance fixture
(`tests/e2e/conformance/workflows.rs`, CLI and GraphQL), and the FFI tests in
`ddb-core/src/ffi/tests.rs`.

## Correctness fixes override compatibility

Silent-data-loss and invalid-write behaviors must be fixed even when a
downstream consumer depends on them. A thinning task must not preserve such
behavior under a compatibility justification. If a correctness fix requires a
shape change, that change still goes through this checklist — but the fix is
not optional. See also the Correctness-over-compatibility flag in
[Transport Policy Inventory](./transport-policy-inventory.md).
