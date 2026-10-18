# REST API

Doogat DB exposes a REST API at `/rest/*` alongside the GraphQL endpoint. Both share the same auth middleware (Bearer token) and actor backend.

## Authentication

All endpoints require `Authorization: Bearer <token>` header. Missing or invalid tokens return `401 Unauthorized`.

## Endpoints

### List / Search Doogats

```
GET /rest/doogats
```

**Query parameters:**

| Param | Type | Description |
|-------|------|-------------|
| `type` | string | Filter by doogat type (list and search) |
| `tag` | string | Filter by tag (list and search) |
| `backlinks` | string | Filter by backlinks of doogat ID (list only) |
| `q` | string | Full-text search (returns search hits instead of doogats) |
| `sort` | string | Sort field: `id`, `title`, `date`, `type`. Prefix with `-` for descending (e.g. `sort=-date`). Default: `date` descending. Invalid fields return 400. (list only) |
| `field.{name}` | string | Filter by inline field value (repeatable for AND logic) (list only) |
| `page` | int | Page number (default: 1) |
| `per_page` | int | Results per page (default: 50). List: values outside 1-200 are clamped. Search: 1-10,000, values outside that range return 400. |

**List response:**

```json
{
  "data": [{ "id": "...", "title": "...", "body": "...", "tags": [], "type": null, "reference_section": "" }],
  "pagination": { "page": 1, "per_page": 50, "total": 100, "total_pages": 2 }
}
```

**Search** (when `q` is provided):

The search branch decodes into the app contract's `SearchCommand` (PRD 00176,
see [app-contract.md](app-contract.md#search-searchcommand-pagerequest-appsearchresult)).
`type` becomes `SearchCommand.types` and `tag` becomes `SearchCommand.tag`.
REST accepts exactly one `type` value, deliberately: the query string carries no
list form, so a multi-type filter stays a GraphQL and FFI capability, where the
argument is already a list. Repeat the search per type to cover several.
A filter that matches nothing returns an empty `data` array, not the unfiltered
result. `backlinks`, `sort`, and `field.{name}` do not apply to search.

Pagination goes through the shared `PageRequest` policy, not the list clamp:

- `page` defaults to 1. A `page` below 1 or a non-integer `page` returns 400
  `VALIDATION_ERROR`.
- `per_page` defaults to 50. A `per_page` outside 1-10,000 or a non-integer
  `per_page` returns 400 `VALIDATION_ERROR`.
- The offset is `(page - 1) * per_page`, computed with an overflow check. An
  overflow returns 400 `VALIDATION_ERROR`.

**Search response:**

```json
{
  "data": [{
    "id": "...",
    "title": "...",
    "path": "ddb/....md",
    "snippet": "...",
    "rank": -1.0,
    "updated_at": "...",
    "tags": ["..."],
    "type": "...",
    "fields": { "key": "value" },
    "created_at": "..."
  }],
  "total_count": 1,
  "warnings": []
}
```

Each hit carries all ten fields of the core `SearchResult`. `type` and
`created_at` are `null` when the doogat has none. `fields` is always present and
is `{}` when the hit has no typed fields. `total_count` is the full match count,
not the page length. `warnings` carries structured index warnings and is `[]`
when there are none.

**Compatibility (PRD 00176):**

- The hit shape widened from four fields (`id`, `title`, `snippet`, `rank`) to
  ten. The change is additive. No field was renamed or removed.
- `type` and `tag` used to be dropped silently on the search branch. They now
  filter the hits.
- Search `per_page` used to be clamped silently to 1-200. It is now validated
  against 1-10,000: `per_page=500` now returns 500 hits, not 200, and
  `per_page=10001` or `per_page=0` now returns 400, not a clamped 200 OK. A
  `page` below 1 used to be treated as 1 and now returns 400. An absent
  `per_page` still gives 50.
- The plain list branch (no `q`) did not change. It keeps the 1-200 clamp.

### Get Doogat

```
GET /rest/doogats/:id
```

Returns `{ "data": { ... } }` or `404` if not found.

### Create Doogat

```
POST /rest/doogats
Content-Type: application/json

{ "title": "...", "body": "...", "tags": ["..."], "type": "...", "fields": { "done": true, "priority": 3 } }
```

Returns `201 Created` with `{ "data": { ... } }`.

`fields` (optional) is a JSON object of typed column values (PRD 00178). Booleans, numbers, strings, lists, and nested objects keep their JSON type. A top-level `null` value omits that key on create and unsets it on update; no literal `null` text is written. A `null` nested inside a list or object is rejected. `"fields": null` or no `fields` member means no fields.

**Direct beta migration, no deprecation window (maintainer decision).** Before PRD 00178 `fields` was a JSON-encoded string (`"fields": "{\"done\":true}"`). That form, and any other non-object value, is now rejected with HTTP 400 `VALIDATION_ERROR`; it is never reparsed. Send the object itself.

Known gap: the cross-interface conformance harness (`tests/e2e/conformance/`) has no REST driver, so the `typed_fields_create` fixture runs only on GraphQL. REST coverage lives in the e2e scenario `tests/e2e/integration_structured_fields_input.rs`. The REST driver is owned by PRD 00175 (stalled) and is a tracked follow-up.

### Update Doogat

```
PUT /rest/doogats/:id
Content-Type: application/json

{ "title": "...", "body": "...", "fields": { "note": null } }
```

All fields are optional (partial update). `fields` follows the create rules above; a `null` value unsets that key. Returns `{ "data": { ... } }`.

### Delete Doogat

```
DELETE /rest/doogats/:id
```

Returns `204 No Content`.

## Error Format

```json
{
  "error": "NOT_FOUND",
  "message": "doogat not found: 20240101120000"
}
```

| HTTP Status | Error Code | Trigger |
|-------------|-----------|---------|
| 400 | `VALIDATION_ERROR` | Invalid input |
| 404 | `NOT_FOUND` | Doogat doesn't exist |
| 422 | `SQL_ERROR` | SQL engine error |
| 500 | `INTERNAL_ERROR` | Unexpected failure |

## Implementation

REST handlers in `ddb-server/src/rest.rs` translate HTTP requests to actor commands. The actor pattern (`ActorHandle`) provides thread-safe access to the Git repo and SQLite index. The same `ActorHandle` serves both GraphQL and REST routes.
