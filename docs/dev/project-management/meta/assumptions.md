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
