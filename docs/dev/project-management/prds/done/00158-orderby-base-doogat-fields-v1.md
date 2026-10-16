# Expose base Doogat fields in typed-query orderBy

## Overview

### Problem Statement
Per-type `orderBy` inputs are built only from materialized typed columns. `build_order_by_input` (`ddb-server/src/filter.rs:160-169`) iterates only `schema.columns` and injects nothing else, and `build_order_sql` (`filter.rs:176-204`) drops any field absent from `schema.columns` via `resolve_column`. By contrast `build_where_input` (`filter.rs:128-134`) injects the base Doogat fields `id`/`title`/`tags`. The asymmetry means a typed connection cannot be ordered by the base Doogat fields `created_at` or `updated_at` (nor `id`). A typedef can be sorted by `title` only because a downstream explicitly declared a `title` column; uniform base-field injection would make ordering predictable across every type.

### Target Users
Any GraphQL client paginating a typed connection by recency (created/updated) — the most common list ordering. Every downstream that renders a reverse-chronological list hits this.

### Success Metrics
- `orderBy: { created_at: DESC }` and `{ updated_at: DESC }` accepted on every typed connection, producing correctly ordered results.
- Ordering composes with server-side `limit`/`offset`, so clients paginate without fetching the whole table.
- At least one downstream (jink) deletes its client-side sort-and-paginate fallback.

Motivating downstream evidence (jink): the library list defaults to an `updatedAt` sort but, lacking server support, fetches the entire link table (`limit: undefined`) and sorts+paginates in JS — `frontend/src/routes/library/+page.svelte:155-181`, `frontend/src/lib/graphql/operations/links.ts:347-380`. This is a correctness/scaling defect, not just an ergonomic one.

## Functional Decomposition

### Capability: Base-field ordering
#### Feature: created_at / updated_at / id sort keys
Inject `created_at`, `updated_at`, and `id` into every per-type `orderBy` input, collision-guarded against typedef columns of the same name.
#### Feature: Pagination correctness under base-field sort
`build_order_sql` resolves the injected base keys to a stable SQL `ORDER BY` so `limit`/`offset` paginate deterministically.

## Structural Decomposition

### Repository Structure
Localized to the GraphQL filter/order builder; no schema-storage changes if existing base columns back the keys.

### Module: ddb-server/src/filter.rs
- `build_order_by_input` (160-169): inject base sort keys after the column loop, mirroring `build_where_input` (128-134).
- `build_order_sql` / `resolve_column` (176-204): resolve base keys, not only `schema.columns`.

## Dependency Graph

### Foundation Layer (Phase 0)
Decide the SQL backing for each base key.

### Core Layer (Phase 1)
Key injection + order-SQL resolution.

### Integration Layer (Phase 2)
Schema/introspection exposure + tests.

## Implementation Phases

### Phase 0: Foundation
Determine the SQL backing for each base key. ddb ids are timestamp-derived, so `id ASC` already approximates creation order — decide whether `created_at` aliases id-ordering or a stored column. Confirm whether `updated_at` is materialized anywhere; if not, identify or add a sortable source.

### Phase 1: Core
Inject `created_at`/`updated_at`/`id` into `build_order_by_input`; teach `resolve_column`/`build_order_sql` to map them to their backing expressions.

### Phase 2: Integration
Expose the keys via introspection; add coverage; document the new sort keys.

## Test Strategy

### Critical Scenarios
- Order each typed connection by `created_at`/`updated_at` ASC and DESC; verify order.
- Paginate a base-field sort across pages; verify no gaps, dupes, or instability.
- Name collision: a typedef that declares its own `created_at` column still resolves unambiguously.

## Risks
- Name collision with user-declared columns (guard via `sanitize_field_name`).
- `updated_at` may have no materialized backing source yet, expanding scope beyond the builder.

## Transport Impact
GraphQL typed connections gain base-field `orderBy`. SQL `ORDER BY` and PgWire operate on the underlying tables and already reach base columns; GraphQL is the surface that lacked them.
