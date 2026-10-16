# Typed where: filter completeness — negation and relation traversal

## Overview

### Problem Statement
The typed `where:` filter cannot express two common needs.

(a) Negation. There is no `notIn`/`nin` or null operator on scalar or `IDFilter` inputs; `build_operator_condition` (`ddb-server/src/filter.rs:687+`) emits only positive comparisons, and the introspected `IdFilter` exposes no negation. Clients emulate "NOT IN" by fetching all ids and negating client-side.

(b) Relation traversal. A REFERENCES field in `where:` only compares the materialized id on the parent row — `build_column_conditions` (`filter.rs:640-657`) routes through `resolve_column` to a direct `"category" = ?`. So the client must pre-resolve the target id, and there is no way to filter a parent by a related entity's properties (e.g. `links where category.title = "X"`) nor to do reverse/junction membership (`category -> its links`, `some`/`none`/`every`) in one query. The only junction-join generator today is the `_ddb_tags` EXISTS path (`filter.rs:547-578`). PRD 00129 §5 shipped the partial (tag EXISTS + REFERENCES-eq-by-id); this PRD completes it.

### Target Users
Any client filtering a connection by negation, or by a related entity's properties, without N extra round-trips.

### Success Metrics
- `where: { id: { notIn: [...] } }` (and null filters) work.
- `where: { category: { title: { eq: "X" } } }` relation sub-filter works.
- Reverse/junction membership quantifiers `some`/`none`/`every` work in one query (`none` = "uncategorized").
- Downstreams delete client-side id pre-resolution and stop using `search()` for structured filtering.

Motivating downstream evidence (jink): category filtering is routed through `search(query:"category=X")` (conflating with BM25 ranking) or pre-resolved in 2-3 round-trips, and the uncategorized view fetches every link id twice then negates client-side — `frontend/src/routes/library/+page.svelte:33-83` and `:113-135`.

## Functional Decomposition

### Capability: Filter negation
#### Feature: notIn / nin and null operators
Add negation and null operators to scalar and `IDFilter` inputs and their SQL emission.

### Capability: Relation traversal
#### Feature: Forward relation sub-filter
For a REFERENCES column, accept a sub-`where` over the target typedef and emit an EXISTS-over-join condition.
#### Feature: Reverse/junction membership quantifiers
`some`/`none`/`every` over a junction relation, in both directions, using junction-table metadata.

## Structural Decomposition

### Repository Structure
GraphQL filter builder plus junction metadata wiring from ddb-core.

### Module: ddb-server/src/filter.rs
- `build_operator_condition` (687+) and scalar/Id filter input types: negation/null operators.
- `build_where_input` (128-134): emit a relation-filter input for REFERENCES columns instead of bare `IDFilter`.
- `try_build_where_sql` / `build_column_conditions` (494, 640): emit EXISTS-over-junction joins (template: `_ddb_tags` EXISTS at 547-578).

### Module: ddb-core (schema/junction metadata)
Expose junction-table metadata so reverse-direction membership can be resolved.

## Dependency Graph

### Foundation Layer (Phase 0)
Negation/null operators (self-contained).

### Core Layer (Phase 1)
Forward relation sub-filter via EXISTS.

### Integration Layer (Phase 2)
Reverse/junction quantifiers + metadata wiring.

## Implementation Phases

### Phase 0: Foundation
Add `notIn`/`nin` and null operators to scalar/`IDFilter` inputs and `build_operator_condition`. Smallest, independently shippable slice.

### Phase 1: Core
For REFERENCES columns, emit a relation-filter input (sub-`where` over the target typedef); compile to an EXISTS-over-join, reusing the `_ddb_tags` EXISTS template.

### Phase 2: Integration
Add `some`/`none`/`every` quantifiers over junction relations in both directions; wire junction-table metadata from ddb-core. Bound nesting depth.

## Test Strategy

### Critical Scenarios
- `notIn` and null filters on id and scalar columns.
- Relation sub-filter by a target entity's property.
- Reverse membership `some`/`none` (uncategorized = `none`).
- Relation filter combined with `orderBy` + pagination.

## Risks
- Query-plan cost of EXISTS joins on large sets.
- Schema growth: a relation-filter input type per REFERENCES column.
- Nested-relation cycles — bound depth to avoid unbounded recursion.

## Transport Impact
GraphQL typed `where:` gains negation and relation operators. Assess SQL/PgWire parity for negation; relation traversal is a GraphQL-layer compilation over existing joins.
