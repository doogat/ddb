# REST API

ZettelDB exposes a REST API at `/rest/*` alongside the GraphQL endpoint. Both share the same auth middleware (Bearer token) and actor backend.

## Authentication

All endpoints require `Authorization: Bearer <token>` header. Missing or invalid tokens return `401 Unauthorized`.

## Endpoints

### List / Search Zettels

```
GET /rest/zettels
```

**Query parameters:**

| Param | Type | Description |
|-------|------|-------------|
| `type` | string | Filter by zettel type |
| `tag` | string | Filter by tag |
| `backlinks` | string | Filter by backlinks of zettel ID |
| `q` | string | Full-text search (returns search hits instead of zettels) |
| `sort` | string | Sort field (reserved for future use) |
| `field.{name}` | string | Filter by inline field value (repeatable for AND logic) |
| `page` | int | Page number (default: 1) |
| `per_page` | int | Results per page (default: 50, max: 200) |

**List response:**

```json
{
  "data": [{ "id": "...", "title": "...", "body": "...", "tags": [], "type": null, "reference_section": "" }],
  "pagination": { "page": 1, "per_page": 50, "total": 100, "total_pages": 2 }
}
```

**Search response** (when `q` is provided):

```json
{
  "data": [{ "id": "...", "title": "...", "snippet": "...", "rank": 1.0 }]
}
```

### Get Zettel

```
GET /rest/zettels/:id
```

Returns `{ "data": { ... } }` or `404` if not found.

### Create Zettel

```
POST /rest/zettels
Content-Type: application/json

{ "title": "...", "body": "...", "tags": ["..."], "type": "..." }
```

Returns `201 Created` with `{ "data": { ... } }`.

### Update Zettel

```
PUT /rest/zettels/:id
Content-Type: application/json

{ "title": "...", "body": "..." }
```

All fields are optional (partial update). Returns `{ "data": { ... } }`.

### Delete Zettel

```
DELETE /rest/zettels/:id
```

Returns `204 No Content`.

## Error Format

```json
{
  "error": "NOT_FOUND",
  "message": "zettel not found: 20240101120000"
}
```

| HTTP Status | Error Code | Trigger |
|-------------|-----------|---------|
| 400 | `VALIDATION_ERROR` | Invalid input |
| 404 | `NOT_FOUND` | Zettel doesn't exist |
| 422 | `SQL_ERROR` | SQL engine error |
| 500 | `INTERNAL_ERROR` | Unexpected failure |

## Implementation

REST handlers in `zdb-server/src/rest.rs` translate HTTP requests to actor commands. The actor pattern (`ActorHandle`) provides thread-safe access to the Git repo and SQLite index. The same `ActorHandle` serves both GraphQL and REST routes.
