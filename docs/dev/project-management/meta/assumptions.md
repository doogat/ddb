# Assumption ledger — 00184-schema-apply-v2-index-evolution-ffi-zone-parity-v1

## Task 1: Diff unique_together deltas on existing types into AddUniqueConstraint/DropUniqueConstraint PlanOps

Tess (initial):
- DropUniqueConstraint renders exactly `ALTER TABLE "t" DROP UNIQUE ("c1", "c2")`, the spec's "DROP UNIQUE mirror" read literally.
- Where a test compares op columns exactly, the fixture columns are already sorted, so an implementation that keeps the given order and one that sorts both pass. The column order the op carries is not pinned.
- Comparison ignores the order of the constraints in the list as well as the column order inside each one (set-of-sets, pinned by `reordered_constraint_list_is_unchanged`).
- Ordering of unique-constraint ops relative to other ops, or to DropColumn, is not pinned because the spec says nothing about it. (Superseded by the strengthen pass below, which did pin this.)
- A new type (live `None`) still emits only `CreateType`, with no separate AddUniqueConstraint — the create path is existing behavior, unchanged.
- `describe()` for Add and Drop must differ from each other; the spec only says it names the columns and table.
- Did not test live `Some(vec![])` against desired `None` — the spec does not say whether they are equal.

Tess (strengthen, after Devon):
- Duplicate constraints count once (set-of-sets), on both the desired and live side. The design's prose says this, but its sample code does not do it.
- Emitted ops carry each constraint's columns sorted, per the design's `normalize_constraints`; every expected op in the new tests uses column lists already in alphabetical order to avoid depending on this.
- The add/drop relative order within a change (point 1 in Devon's list) checks membership and op count, not the order of the add and the drop — no dependency was pinned there.
- `describe()` lists columns as they are stored in the op, joined by `", "`, per the design's `columns.join(", ")`.

Ivan (implementor):
- When a constraint is declared twice in different column orders, the op uses the first declaration's order; the tests only check the first-declared order.
- When one type both drops and adds a constraint, the drop comes before the add; the tests check membership only, not this order.
- Duplicate columns inside one constraint (e.g. `[day, day]`) are not reduced; no test covers that case.

## Task 2: Execute unique-constraint PlanOps via custom-DDL handlers with a pre-commit duplicate check

Ivan (review-fix retry, MEDIUM finding):
- The finding's "add a test" half is out of reach for this dispatch (the test suite lives in `ddb-core/src/sql_engine/tests`, outside the retry's allowlist, and the contract forbids adding tests); only the code half (excluding NULL rows from the duplicate-row check) was applied. A follow-up test-only task should add the NULL-row regression test Pat's finding asked for.

## Task 3: Integration-test index-evolution apply

Ivan (implementor):
- `apply_schema`'s existing atomic-apply loop wraps EVERY mid-plan op failure as `SCHEMA_APPLY_PARTIAL` (the inner error's text folded into the message), including a single-op plan — not `UNIQUE_VIOLATION` directly. The error-case test asserts `SCHEMA_APPLY_PARTIAL` with a message containing "duplicate", matching this existing wrapping behavior rather than the handler's raw code; task 2's own `UNIQUE_VIOLATION` code is still correct at the `sql_engine` layer, this is only about what `apply_schema` (the service layer this task tests) surfaces to its caller.

## Task 4: Add zone field to FFI ColumnDefRecord matching GraphQL null-when-undeclared semantics

Ivan (implementor):
- `tests/swift/Sources/DoogatDB/ddb_core.swift` was already stale relative to `ddb-core`'s current UniFFI surface before this task (missing `applySchema`, `createStructured`/`updateStructured`, `searchFiltered`, and others) — regenerating it for the `zone` field necessarily also caught up this pre-existing drift. The committed diff is the full regenerated file, not an additive-only `zone` diff; this reflects the real current API surface and is correct to commit as-is rather than hand-trim to only the `zone` lines.
