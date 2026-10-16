---
default_model: opus
model_tier_rationale: invented predicate - COALESCE-guarded every, SQL three-valued-logic semantics
---

# GraphQL where-filter correctness and completeness v2

> **Provenance:** the LIKE-wildcard escaping fix was added by the 2026-07-16 assessment (F14) and folded into the body's Features/tasks at the 2026-08-14 backlog review (sites re-verified at `8fe51c9`: `filter.rs:1076-1083`, no `ESCAPE` anywhere in filter.rs/relation_filter.rs). The every+NULL rule was pre-decided by the maintainer at the 2026-08-14 review: **NULL fails `every`** (COALESCE-guarded), superseding the delegated in-task decision. The base-field and self-ref completeness items were dropped at the 2026-09-05 review (already documented as out of scope in `server.md:706,:708`).

## Overview

### Problem Statement
PRD 00159 shipped GraphQL relation sub-filters and negation/null operators. Its doubt review surfaced one real correctness bug plus a cluster of completeness, performance, and test-coverage gaps that were deferred:

- **Bug (correctness)**: a per-quantifier explicit GraphQL `null` (`some: null` / `none: null`, e.g. via a null variable) is treated as relation presence/absence instead of being ignored. `build_conditions_into` (`ddb-server/src/filter.rs:671`, non-Object early return at `:678-680`) returns no conditions for a non-Object value, so `some: null` compiles to the no-JOIN `EXISTS(junction)` ("has any relation") and `none: null` to `NOT EXISTS` ("uncategorized") — a silent, surprising result for what should be a no-op filter.
- **3VL footgun (decided 2026-08-14: NULL fails `every`)**: today `every` + a related NULL value satisfies the predicate (`relation_filter.rs:174` emits `NOT EXISTS(... AND NOT(sub))`; SQLite 3VL makes `NOT(sub)` unknown so the row is not excluded), pinned by the 00159 e2e test `every_treats_null_target_column_as_non_counterexample` (`tests/e2e/typed_where_filter.rs:262`). The maintainer adopted Codex's position: NULL FAILS `every` (`COALESCE((sub),0)=0`); this PRD ships the change.
- **Completeness (DECIDED 2026-09-05 backlog review: out of scope)**: base target fields are not inlined into `{Target}RelationFilter` and self-referencing types get no reverse-membership field — both are ALREADY documented as deliberate limitations in `docs/src/technical/server.md:706` (base fields, with the `some: { title: { eq } }` fallback) and `:708` (self-ref, "independent of any field-name collision"). Only the code comment at `ddb-server/src/filter.rs:815-819` (fn `reverse_relations`, skip at `:820-822`) still tells the stale "name collision" story; this PRD corrects that comment and adds nothing else here.
- **Performance**: reverse-membership `EXISTS` does a full junction SCAN because the junction PK is `(parent_id, ref_id)` (`indexer/materialize.rs:97`) and a `ref_id` lookup has no usable index (no `CREATE INDEX` on `ref_id` anywhere).
- **Test coverage**: the `every` quantifier (forward + reverse, vacuous-true + nonempty) is unasserted at the integration/e2e scenario level.

### Target Users
Downstream apps (jink) building structured GraphQL filters over relations; they need null-safe, predictable quantifier semantics and acceptable performance on reverse-membership queries.

### Success Metrics
- `some: null` / `none: null` (and the other quantifiers with explicit null) are IGNORED (no condition emitted), pinned by a variable-based regression test — no more silent presence/absence.
- `every` + NULL follows the decided rule — NULL FAILS `every` (COALESCE-guarded) — pinned by the updated 00159 e2e + docs + a CHANGELOG behavior note.
- `contains`/`startsWith` values with `%`/`_` match literally (escaped + `ESCAPE '\'`), pinned by a regression test that fails on the old code.
- The stale "name collision" comment at `filter.rs:815-819` is corrected to match `server.md:708` (base-field inlining and self-ref reverse membership stay out of scope — already documented at `server.md:706,:708`; decided 2026-09-05).
- Reverse-membership queries use an index on the junction `ref_id` column (no full SCAN), verified by `EXPLAIN QUERY PLAN`.
- `every` forward + reverse, vacuous + nonempty, covered in `tests/e2e/typed_where_filter.rs` (the shell harness is removed by 00171).

## Functional Decomposition

### Capability: Null-safe quantifier semantics
#### Feature: Ignore explicit-null quantifier values
- **Description**: a quantifier whose value is GraphQL `null` emits no SQL condition.
- **Inputs**: a `MembershipFilter`/relation sub-filter with a `GqlValue::Null` sub-value.
- **Outputs**: no condition added for that quantifier.
- **Behavior**: skip `GqlValue::Null` in the `build_forward_relation` (`relation_filter.rs:58`, loop `:79-97`) + `build_reverse_relation` (`:236`, loop `:250-265`) loops before reaching `build_conditions_into` / `emit_membership_exists` (whose empty-conditions branch at `:158-160` is what produces the presence/absence SQL today).

#### Feature: Decided every+NULL rule — NULL fails every
- **Description**: a related NULL FAILS `every` (maintainer decision, 2026-08-14 backlog review): "every X satisfies P" is false when some X's value is unknown, consistent with `some`'s no-match treatment of NULL.
- **Inputs**: an `every` quantifier over a sub-filter that can be NULL on some related rows.
- **Outputs**: `COALESCE`-guarded SQL — `NOT EXISTS(... AND COALESCE((sub), 0) = 0)`-shaped so an unknown sub-predicate excludes the row.
- **Behavior**: implement the decided rule at `relation_filter.rs:174`; update the pinned 00159 e2e test (`typed_where_filter.rs:262`) + `server.md:720`; CHANGELOG notes the behavior change (previously the 3VL form let NULL rows pass).

#### Feature: Escape LIKE wildcards in contains/startsWith
- **Description**: `%`/`_` inside user values stop acting as wildcards in `contains`/`startsWith`/`endsWith`-style operators.
- **Inputs**: the raw bound value pushed into `LIKE` (`filter.rs:1076-1083`, no `ESCAPE` clause today).
- **Outputs**: `LIKE` patterns with `\`, `%`, `_` escaped in the bound value and `ESCAPE '\'` appended.
- **Behavior**: escape at the one site that builds the LIKE pattern; no injection is involved (the value stays a bound parameter) — this fixes silently wrong result sets, e.g. `startsWith: "my_field"` matching `myXfield` in this snake_case-heavy domain.

### Capability: Reverse-membership performance
#### Feature: Junction ref-id index
- **Description**: an index supporting `ref_id` lookups on junction tables.
- **Inputs**: `junction_table_ddl` (`indexer/materialize.rs:92`), the one DDL builder behind both creation paths (`materialize.rs:143` and `sql_engine/ddl.rs:398`).
- **Outputs**: a secondary index on the ref-id column, emitted by the one builder so both paths inherit it.
- **Behavior**: create the index at junction materialization; `EXPLAIN QUERY PLAN` shows index use, not SCAN.

### Capability: Quantifier test coverage
#### Feature: every-quantifier scenarios
- **Description**: integration + e2e coverage of `every` in both directions and both vacuity cases.
- **Inputs**: fixtures with empty and nonempty relation sets.
- **Outputs**: passing scenarios.
- **Behavior**: add forward + reverse `every` (vacuous-true + nonempty) to `tests/e2e/typed_where_filter.rs` (the shell harness is removed by 00171).

## Structural Decomposition

### Repository Structure

```
ddb-server/src/
├── relation_filter.rs   # Maps to: Null-safe quantifier semantics
│                         #   build_forward_relation (:58) / build_reverse_relation (:236); every form at :174
├── filter.rs            # Maps to: Null-safe quantifier semantics (build_conditions_into :671); stale self-ref comment fix (:815-819); Escape LIKE wildcards (:1076-1083)
ddb-core/src/indexer/
└── materialize.rs       # Maps to: Reverse-membership performance (junction_table_ddl :92, called at :143)
ddb-core/src/sql_engine/
└── ddl.rs               # Maps to: Reverse-membership performance (second junction_table_ddl call site, :398)
tests/e2e/
tests/e2e/
└── typed_where_filter.rs # Maps to: Quantifier test coverage (sole scenario home; shell harness removed by 00171)
```

### Module: relation_filter (ddb-server/src/relation_filter.rs)
- **Maps to capability**: Null-safe quantifier semantics
- **Responsibility**: build forward/reverse relation sub-filter SQL.
- **Exports**:
  - `build_forward_relation(...)` / `build_reverse_relation(...)` - null-skip; `every` in the COALESCE-guarded form

### Module: materialize (ddb-core/src/indexer/materialize.rs)
- **Maps to capability**: Reverse-membership performance
- **Responsibility**: `junction_table_ddl` emits the ref-id index; both call sites (`materialize.rs:143`, `sql_engine/ddl.rs:398`) inherit it.
- **Exports**:
  - `junction_table_ddl` - now includes the ref-id index

### Module: filter, DDL, and conformance callers (ddb-server/src/filter.rs; ddb-core/src/sql_engine/ddl.rs; tests/e2e/typed_where_filter.rs)
- **Maps to capability**: Null-safe quantifier semantics; Reverse-membership performance; Quantifier test coverage
- **Responsibility**: escape literal LIKE values, apply null semantics, emit reverse indexes through both creation paths, and pin all quantifier cases.
- **Exports**: existing filter/DDL APIs and typed-where regression fixtures.

## Dependency Graph

### Foundation Layer (Phase 0)
No dependencies - built first.

- **relation_filter (null-skip)**: the correctness fix; independent of the others.

### Core Layer (Phase 1)
- **relation_filter (every-rule)**: Depends on [relation_filter null-skip] (shared code paths).
- **materialize (ref-id index)**: no dependency on the filter changes; independent.

### Integration Layer (Phase 2)
- **test coverage + docs**: Depends on [relation_filter, materialize].

## Implementation Phases

### Phase 0: Foundation
**Goal**: fix the real null-handling bug first.

**Tasks**:
- [ ] Skip `GqlValue::Null` in forward + reverse quantifier loops (no deps) - Acceptance: `some: null`/`none: null` (null variable) emit no condition; variable-based regression test green; non-null quantifiers unchanged.
- [ ] Escape `\`, `%`, `_` in the bound value of `contains`/`startsWith` LIKE patterns and append `ESCAPE '\'` (`filter.rs:1076-1083`) (no deps) - Acceptance: `startsWith: "my_field"` no longer matches `myXfield`; a literal `%` in a value matches only `%`; regression tests fail on the old code; existing LIKE tests stay green.

**Exit Criteria**: explicit-null quantifiers are no-ops and LIKE wildcards are escaped, tested.

### Phase 1: Core
**Goal**: ship the every+NULL rule and the perf index.

**Tasks**:
- [ ] Implement the decided every+NULL rule — NULL FAILS `every` via the COALESCE-guarded form; update the pinned 00159 e2e test + docs + CHANGELOG behavior note (depends on: Phase 0) - Acceptance: a related NULL sub-value excludes the row from `every` (forward + reverse); the updated e2e pins it; docs + CHANGELOG record the change from the prior 3VL behavior.
- [ ] Add a junction `ref_id` index in `junction_table_ddl` so both creation paths (`materialize.rs:143`, `sql_engine/ddl.rs:398`) emit it (no deps) - Acceptance: `EXPLAIN QUERY PLAN` on a reverse-membership query shows index use, not SCAN, for a table created through either path.

**Exit Criteria**: every+NULL shipped; reverse queries indexed.

### Phase 2: Integration
**Goal**: lock the behavior with scenario coverage + docs.

**Tasks**:
- [ ] Add `every` forward+reverse, vacuous+nonempty scenarios to the Rust e2e harness (`tests/e2e/typed_where_filter.rs`; the shell harness is removed by 00171) (depends on: Phase 1) - Acceptance: all four cases asserted and green.
- [ ] Update `docs/src/technical/server.md` for null-skip and every+NULL; correct the stale self-ref comment at `filter.rs:815-819` to match `server.md:708`; + a `CHANGELOG.md` entry (Fixed: explicit-null quantifiers are no-ops; LIKE wildcards literal; Changed: every+NULL) (depends on: Phase 1) - Acceptance: docs match shipped behavior; the comment no longer claims a name collision; CHANGELOG entry present.

**Exit Criteria**: scenario coverage + docs landed.

## Test Strategy

**Validation:** per-task local gates are `cargo build`, `cargo clippy --workspace --all-targets`, and `cargo test-ci`, including colocated unit regressions. Author required e2e/integration/conformance scenarios and executable walkthroughs; their heavy execution belongs to Tier 2 CI, not the per-task gate. Retain only CLAUDE.md's conditional e2e run for Claude-routed tasks deleting/replacing code paths. Walkthrough creation uses showboat; any verification uses `dev/bin/safe-showboat-verify`, never the project root. This backlog assumes 00171 has completed the shell-to-Rust harness migration.

### Critical Scenarios
- **Happy path**: `tasks(where: { assignees: { some: { name: { eq: "x" } } } })` → Expected: unchanged, correct EXISTS join.
- **Edge case**: `some: null` via a null variable → Expected: no condition emitted (returns the unfiltered set, not "has any relation").
- **Error case**: `every` over a relation with a NULL sub-value → Expected: the single decided rule (documented + tested), consistent across forward/reverse.

## Risks

- **every+NULL flips pinned behavior**: the decided NULL-fails-`every` rule changes a pinned 00159 test + documented semantics. Mitigation: the decision is recorded (2026-08-14 maintainer call); update test+docs+CHANGELOG together in one task.
- **Index addition changes junction DDL**: existing repos need a reindex to gain the index. Mitigation: index created on materialize/reindex; note in CHANGELOG that `ddb reindex` realizes it.

## Transport Impact

- **GraphQL**: Guaranteed — this is a GraphQL-surface capability; all features land here.
- **CLI / REST / FFI / NoSQL HTTP**: the typed relation where-filter is a GraphQL-specific capability (per 00159's Transport Impact); these interfaces do not expose it. Action: confirm no regression; document that structured relation filtering remains GraphQL-only (SQL/PgWire have raw SQL `EXISTS` as the equivalent).
- **PgWire**: Specialized — raw SQL already expresses `EXISTS`/quantifier logic; not a target for the typed input.
- **Cross-interface conformance plan**: GraphQL integration tests (server crate) + `tests/e2e/typed_where_filter.rs` scenarios pin the behavior (the shell harness is removed by 00171); the perf index is verified via `EXPLAIN QUERY PLAN`. No new cross-interface surface is added.
