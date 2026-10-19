# Design: GraphQL where-filter correctness and completeness v2

## Architecture fit

All changes land in the existing GraphQL typed-where-filter layer
(`ddb-server/src/filter.rs`, `ddb-server/src/relation_filter.rs`) and its
SQLite materialization DDL (`ddb-core/src/indexer/materialize.rs`, mirrored at
`ddb-core/src/sql_engine/ddl.rs:398`). No new module, no new layer. This is a
GraphQL-only capability (confirmed by the PRD's Transport Impact section);
CLI/REST/FFI/PgWire are unaffected because they never call
`build_forward_relation`/`build_reverse_relation`/`emit_membership_exists`.

## Module placement

All edits to existing files; no new files.

- `ddb-server/src/relation_filter.rs` — null-skip in `build_forward_relation`
  (loop at `:69-100`) and `build_reverse_relation` (loop at `:250-265`);
  COALESCE-guarded `every` form in `emit_membership_exists` (`:172-176`).
- `ddb-server/src/filter.rs` — LIKE-escape helper + call sites in
  `build_operator_condition`'s `"contains"`/`"startsWith"` arms (`:1076-1083`);
  comment fix at `reverse_relations` (`:815-819`).
- `ddb-core/src/indexer/materialize.rs` — `junction_table_ddl` (`:92-99`) gains
  a secondary index statement.
- `ddb-core/src/sql_engine/ddl.rs` — add the `junction_table_index_ddl` call
  right after the existing `junction_table_ddl` call at `:398` (see § Interfaces
  & contracts, item 6); this file IS edited, not test-only.
- `tests/e2e/typed_where_filter.rs` — new scenarios; update
  `every_treats_null_target_column_as_non_counterexample` (`:262-293`) for the
  decided rule.
- `docs/src/technical/server.md` — behavior notes near `:759-771`.
- `CHANGELOG.md` — Fixed/Changed entries.

## Interfaces & contracts

### 1. Null-skip in forward relation (`relation_filter.rs`)

Scope is deliberately limited to the PRD's literal wording: the quantifier
keys (`some`/`none`/`every`) only. In `build_forward_relation`'s loop, add
the guard inside the `QUANTIFIERS` branch, not before the `ID_OPS` dispatch:

```rust
} else if QUANTIFIERS.contains(&key) {
    if matches!(val, GqlValue::Null) {
        continue;
    }
    conditions.push(emit_quantifier_exists(
        parent_col,
        target_table,
        key,
        val,
        ctx,
    )?);
} else {
```

The `ID_OPS` branch (`category: { eq: null }`) and the inline-target-column
branch (`category: { title: null }`) are left untouched: they keep today's
behavior (an id-op `eq`/`neq`/etc. on `null` still compiles to an
always-false `col = ?` NULL-compare via `build_operator_condition`, and an
inlined-column `null` still compiles through the implicit-`some` path to
`no_join_membership_sql`'s vacuous-true `EXISTS`, since the sub-value passed
to `compile_membership_sub_conditions` is itself non-null — a `{ key: null }`
wrapper object, not a null value at the quantifier level). Broadening the
guard to every key (as an earlier draft of this design did) would silently
flip the already-shipped NULL-compare semantics on relation id-ops — a
behavior change outside this PRD's stated scope, with its own test and
CHANGELOG obligations the PRD does not ask for. If that broader null-handling
gap turns out to matter, it is a follow-up PRD's decision, not an
incidental side effect of this one.

### 2. Null-skip in reverse relation (`relation_filter.rs`)

In `build_reverse_relation`'s loop:

```rust
for (key, val) in filter_obj {
    let key = key.as_str();
    if !QUANTIFIERS.contains(&key) {
        continue;
    }
    if matches!(val, GqlValue::Null) {
        continue;
    }
    conditions.push(emit_membership_exists(...)?);
}
```

Same effect: `some: null` / `none: null` / `every: null` on a
`{Type}MembershipFilter` emit no condition.

No change needed in `emit_membership_exists` or
`compile_membership_sub_conditions` for this feature — the guard sits strictly
above them, so `GqlValue::Object` values with real content behave exactly as
today.

### 3. `every` + NULL rule (`relation_filter.rs:172-176`)

Replace:

```rust
let sql = match quantifier {
    "none" => format!("NOT EXISTS ({join_and_where} AND ({sub_sql}))"),
    "every" => format!("NOT EXISTS ({join_and_where} AND NOT ({sub_sql}))"),
    _ => format!("EXISTS ({join_and_where} AND ({sub_sql}))"),
};
```

with:

```rust
let sql = match quantifier {
    "none" => format!("NOT EXISTS ({join_and_where} AND ({sub_sql}))"),
    "every" => format!("NOT EXISTS ({join_and_where} AND COALESCE(({sub_sql}), 0) = 0)"),
    _ => format!("EXISTS ({join_and_where} AND ({sub_sql}))"),
};
```

`COALESCE((sub_sql), 0) = 0` is true when `sub_sql` evaluates to `0`
(false) **or** `NULL` (unknown) — both now count as counterexamples, so a
related row with a NULL sub-predicate value excludes the row from `every`.
When `sub_sql` is `1` (true), `COALESCE` passes it through and the row is not
excluded. This is the one change site for both forward and reverse `every`
(the function is shared), so no duplicate edit is needed.

The vacuous-true form at `no_join_membership_sql` (`:224`, `"every" => "1"`)
is untouched: an empty sub-`where` has no NULL-vs-false ambiguity to guard —
every related row trivially matches, vacuously true, independent of this
rule.

### 4. LIKE-wildcard escaping (`filter.rs`)

Add one private helper near `build_operator_condition`:

```rust
/// Escape SQLite LIKE metacharacters (`\`, `%`, `_`) in a literal value so it
/// matches itself under `LIKE ... ESCAPE '\'`, not as a wildcard pattern.
fn escape_like_literal(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}
```

Change the `"contains"` and `"startsWith"` arms. Both currently push the raw
`value` into `params` and build a string-concatenation LIKE pattern — the
escaping must happen on the **value itself**, not the SQL text, since the
value remains a bound parameter. `contains`/`startsWith` are declared
nullable `String` in the GraphQL schema (`relation_filter.rs` / the
`StringFilter` input), so `title: { contains: null }` is a real, reachable
input on any ordinary (non-relation) `StringFilter` field, not a theoretical
edge case — the arm must handle `GqlValue::Null` explicitly rather than
assume `gql_to_sql` always returns `QueryValue::Text`:

```rust
"contains" => match gql_to_sql(value) {
    QueryValue::Text(s) => {
        params.push(QueryValue::Text(escape_like_literal(&s)));
        Some(format!("{col_ref} LIKE ('%' || ? || '%') COLLATE NOCASE ESCAPE '\\'"))
    }
    other => {
        params.push(other);
        Some(format!("{col_ref} LIKE '%' || ? || '%' COLLATE NOCASE"))
    }
},
"startsWith" => match gql_to_sql(value) {
    QueryValue::Text(s) => {
        params.push(QueryValue::Text(escape_like_literal(&s)));
        Some(format!("{col_ref} LIKE (? || '%') COLLATE NOCASE ESCAPE '\\'"))
    }
    other => {
        params.push(other);
        Some(format!("{col_ref} LIKE ? || '%' COLLATE NOCASE"))
    }
},
```

The `other` arm (reached for `GqlValue::Null`, since `gql_to_sql(Null)` is
`QueryValue::Null` — `filter.rs:1155`) reproduces **today's exact SQL and
param binding**: no escaping, no `ESCAPE` clause, same unparameterized-NULL
`LIKE '%' || ? || '%'` text as before this change. `NULL` bound into that
pattern evaluates to SQL `NULL`, falsy in `WHERE`, so `title: { contains:
null }` keeps matching zero rows exactly as it does today. An earlier draft
of this design instead pattern-matched `QueryValue::Text` and `return
None`'d otherwise — that silently flips `contains: null` from "matches
nothing" to "ignored, matches everything" for every plain `StringFilter`
field in the schema (not just inside a relation filter), an unrequested and
untested behavior change wholly outside this PRD's scope. The `match`-with-
passthrough form above closes that gap: only a non-null string value takes
the new escaped path; every other value (including `null`) is bit-for-bit
unchanged from current behavior.

`COLLATE` is a unary postfix operator in SQLite that binds to the
*immediately preceding expression* — placing `ESCAPE '\'` between the pattern
and `COLLATE NOCASE` (as an earlier draft of this design did) would attach
`COLLATE NOCASE` to the escape-char literal instead of the pattern, silently
regressing `contains`/`startsWith` to case-sensitive matching with no test
catching it (there is no existing case-insensitivity test for these
operators). Parenthesizing the concatenated pattern, putting `COLLATE NOCASE`
directly after it, and `ESCAPE '\'` last avoids that trap. `QueryValue` is
`ddb_core`'s existing bound-value enum already imported in this file (used
throughout `build_operator_condition`); no new type.

`ESCAPE '\'` is appended to the pattern literal, not interpolated with the
value — SQLite's `LIKE ... ESCAPE` clause takes a literal escape character,
and `\\` (one backslash, SQL-escaped for the Rust string) is that literal.

### 5. Stale comment fix (`filter.rs:815-819`)

Replace the comment body (keep the `if r.table_name == current.table_name { continue; }` code unchanged):

```rust
// Intentional skip: self-referencing typedefs are excluded from reverse-
// relation discovery by design, independent of any field-name collision
// (docs/src/technical/server.md:761). Forward relation filtering on a
// self-reference still works normally via `build_forward_relation`.
if r.table_name == current.table_name {
    continue;
}
```

### 6. Junction ref-id index (`materialize.rs:92-99`)

```rust
pub fn junction_table_ddl(table_name: &str, col_name: &str) -> String {
    let table = junction_table_name(table_name, col_name);
    let parent_id = junction_parent_id_column(table_name);
    let ref_id = junction_ref_id_column(col_name);
    format!(
        "CREATE TABLE IF NOT EXISTS \"{table}\" (\"{parent_id}\" TEXT NOT NULL, \"{ref_id}\" TEXT NOT NULL, PRIMARY KEY (\"{parent_id}\", \"{ref_id}\")); \
         CREATE INDEX IF NOT EXISTS \"idx_{table}_{ref_id}\" ON \"{table}\" (\"{ref_id}\")"
    )
}
```

Both call sites (`materialize.rs:143`, `sql_engine/ddl.rs:398`) call
`conn.execute(&junction_table_ddl(...), [])`. `rusqlite::Connection::execute`
runs **one** statement only — it does NOT error on a `;`-joined second
statement, it **silently drops every statement after the first** (verified
empirically: `conn.execute("CREATE TABLE foo (a INTEGER); CREATE INDEX idx_foo ON foo(a)", [])`
returns `Ok(0)` with the table created and the index silently never created).
That is worse than an error for this codebase's "no silent failures" culture
(`AGENTS.md` data-safety invariants) — a `;`-joined `junction_table_ddl`
string would look correct and quietly never create the index. The contract
therefore is: **`junction_table_ddl` returns the `CREATE TABLE` statement
exactly as today** (no `;`-joined second statement), and a new sibling
function is added instead:

```rust
/// Secondary index on a junction table's ref-id column, so reverse-membership
/// EXISTS queries (`WHERE "{ref_id}" = ?`) use an index instead of a full
/// table scan. Always paired with `junction_table_ddl` at both creation
/// sites.
pub fn junction_table_index_ddl(table_name: &str, col_name: &str) -> String {
    let table = junction_table_name(table_name, col_name);
    let ref_id = junction_ref_id_column(col_name);
    format!(
        "CREATE INDEX IF NOT EXISTS \"idx_{table}_{ref_id}\" ON \"{table}\" (\"{ref_id}\")"
    )
}
```

Both call sites change from one `execute` to two:

```rust
self.conn.execute(&junction_table_ddl(&schema.table_name, &col.name), [])?;
self.conn.execute(&junction_table_index_ddl(&schema.table_name, &col.name), [])?;
```

`sql_engine/ddl.rs:398` gets the identical second line added right after its
existing `junction_table_ddl` call. `junction_table_index_ddl` must be
exported from `ddb-core/src/indexer/materialize.rs` (`pub fn`) and added to
`sql_engine/ddl.rs`'s existing `use crate::indexer::materialize::{..., junction_table_ddl}` import list.

## Data flow

1. **Null-skip**: GraphQL request → async-graphql deserializes the `where`
   argument into `GqlValue` → `build_where_sql`/`try_build_where_sql` →
   `build_conditions_into` → `route_field_condition` → `build_forward_relation`
   / `build_reverse_relation`. The null-skip guard sits at the very first
   branch point inside each of those two functions' per-key loops, before any
   SQL fragment is built — so a `null` quantifier value never reaches
   `emit_membership_exists`/`emit_quantifier_exists` and contributes zero
   strings to the `conditions` Vec (same empty-contribution shape as any other
   filter key that resolves to nothing).
2. **every+NULL**: unchanged data flow; only the SQL text `emit_membership_exists`
   builds for the `"every"` arm changes. SQLite evaluates `COALESCE` at query
   time against the live `_r{n}`-aliased joined row.
3. **LIKE escaping**: for a string value, the raw GraphQL value is escaped in
   Rust before being pushed as a bound parameter, and the SQL pattern text
   gains the `ESCAPE '\'` clause; SQLite applies the escape character when
   matching the bound parameter against the column. For a non-string value
   (`null`), the `match` passthrough pushes it unescaped with the unchanged
   pre-existing pattern text, so that path's data flow is bit-for-bit what it
   is today.
4. **Index**: created once per junction table at materialization time
   (`drop_and_create_materialized_table` → `create_junction_tables`, and the
   SQL-engine DDL path for `ALTER TABLE ... ADD COLUMN ... REFERENCES`). Reads
   go through the existing `EXISTS (SELECT 1 FROM "{junction}" WHERE
   "{ref_id}" = ?)`-shaped SQL already emitted by `build_reverse_relation`; no
   query-text change, just a faster plan via the new index.

## Reuse inventory

- `QueryValue` (ddb_core, already imported in `filter.rs` via `gql_to_sql`'s
  return type) — reused for the escaped bound parameter; no new value type.
- `junction_table_name`/`junction_parent_id_column`/`junction_ref_id_column`
  (`ddb_core::indexer`, already imported in both `relation_filter.rs` and
  `materialize.rs`) — reused unchanged for the index name and column name.
- `gql_to_sql`'s existing `QueryValue` variants (already handled throughout
  `build_operator_condition`) — reused via the `match` passthrough in the
  LIKE arms so every non-string value (including `null`) keeps its current
  binding and SQL text unchanged, rather than inventing a new code path for
  it.
- `nothing found, greps tried:` — searched for an existing LIKE-escaping
  helper (`rg "ESCAPE" ddb-server/src ddb-core/src`, `rg "escape.*like|like.*escape" -i ddb-server/src ddb-core/src`,
  `rg "fn escape" ddb-server/src ddb-core/src`) — zero hits beyond this PRD's
  own description; confirmed no prior art to reuse. Searched for an existing
  junction secondary index (`rg "CREATE INDEX" ddb-core/src/indexer ddb-core/src/sql_engine`)
  — hits only `create_unique_indexes`/`create_singleton_lock_index` (unrelated
  constraint indexes, not ref-id lookups), confirming the gap the PRD
  describes and that no existing index builder can be reused verbatim (its
  shape — one index per junction, keyed on the single ref-id column — is
  novel enough to warrant the new sibling function in §6 rather than
  generalizing an existing one).

## Alternatives considered

1. **Smallest-diff: regex/string-replace LIKE escaping inline at the call
   site, no helper function.** Rejected only on grounds of duplication: the
   `contains` and `startsWith` arms need the identical three-character escape,
   and a shared `escape_like_literal` is one line smaller in total diff than
   inlining it twice, with no added abstraction cost (it is a pure, private,
   single-purpose function, not a new public API surface).
2. **Single `junction_table_ddl` returning a `;`-joined multi-statement
   string, kept on plain `execute`.** Rejected: `execute` only runs the first
   statement in a `;`-joined string and silently drops the rest (verified
   empirically, see § Interfaces & contracts item 6) — this would ship with
   the index silently never created, exactly the kind of silent failure this
   codebase's invariants rule out. Switching to `execute_batch` instead was
   also considered and rejected: it changes error-handling semantics at the
   call site (a later statement's failure is not guaranteed to surface the
   same way as today's single-statement `?`-propagation) for no benefit over
   two explicit `execute` calls, which keep each site's existing
   `?`-propagation identical and are no larger in rendered SQL.
3. **Chosen: null-skip as an early `continue` inside each loop, `every` as a
   `COALESCE` rewrite of the existing format string, LIKE escaping as a tiny
   pure helper, index as a sibling DDL function.** All four are the smallest
   change that closes the exact gap named in the PRD; nothing here boxes in
   future quantifier additions (`QUANTIFIERS` is still the single source of
   truth) or future junction DDL changes (both creation paths still funnel
   through `materialize.rs`).

## Risks & edge cases

- **The new junction index is not retrofitted onto already-materialized
  tables.** It is created only by `create_junction_tables`/the `ddl.rs` ALTER
  path, both of which run at materialization/column-add time — a junction
  table that already exists on disk from before this change keeps scanning
  until the next full reindex. This is acceptable under this project's
  "SQLite index is always rebuildable from Git" invariant (a missing index is
  a performance regression, not data loss), but is worth stating explicitly
  so no one assumes the perf fix is retroactive without a rebuild.
- **`every`+NULL flips a pinned test.** `every_treats_null_target_column_as_non_counterexample`
  (`tests/e2e/typed_where_filter.rs:262`) currently asserts `linkNull` (NULL
  color) is INCLUDED. Under the new rule it must be EXCLUDED. The task that
  ships the `COALESCE` change must, in the same commit, flip that assertion
  (and ideally rename the test — its name literally asserts the old 3VL
  behavior; the Phase 1 task owns the rename too, since leaving a
  rule-matching-named-test with a rule-violating name is worse than today's
  state).
- **Likely next changes after this PRD**: (1) base-target-field inlining and
  self-ref reverse membership were explicitly decided out of scope
  (2026-09-05) — a future PRD may revisit either; this design does not touch
  `relation_filter_input`'s inlining logic or `reverse_relations`' self-ref
  skip, so neither is boxed in. (2) The reverse-membership perf fix here is
  junction-side only; a future PRD could add covering indexes on the
  materialized target table itself if `EXPLAIN QUERY PLAN` still shows a scan
  on the JOIN side — the new index only covers the junction's `ref_id`
  lookup, not the `target_table` JOIN, which already has its own PK on `id`.
  (3) `escape_like_literal` could later be reused for a hypothetical `endsWith`
  operator if one is added — the PRD's Overview mentions
  `contains`/`startsWith`/`endsWith`-style operators but only `contains` and
  `startsWith` exist in `ID_OPS`/`build_operator_condition` today; no
  `endsWith` arm exists to fix, so none is added by this design (scope is the
  two operators that exist).
- **Edge case — empty string value to `contains`/`startsWith`.** `""` escapes
  to `""`; `LIKE '%' || '' || '%' ESCAPE '\'` is `LIKE '%%'`, unchanged
  behavior (matches everything), same as today.
- **Edge case — `null` under `_and`/`_or` list items.** Out of scope: the
  PRD's null-skip targets quantifier/id-op/inlined-column values inside a
  relation filter object, not top-level logical-combinator list items, which
  already have their own handling in `build_logical_combinator`
  (`filter.rs:1020-1038`, unchanged by this design).

## Test strategy outline

- **Unit** (colocated, `cargo test-ci`): a focused test on
  `build_forward_relation`/`build_reverse_relation` asserting a
  `GqlValue::Null` value under `some`/`none`/`every` (forward) and
  `some`/`none`/`every` (reverse) yields an empty `Vec<String>`; a unit test
  on `escape_like_literal` for `%`, `_`, `\`, and a literal containing all
  three plus normal characters.
- **Integration/e2e** (`tests/e2e/typed_where_filter.rs`, Tier 2 — authored
  now, executed in CI):
  - `some_none_null_variable_is_ignored` (or similar): a GraphQL query using a
    `$v: CategoryWhere` variable bound to JSON `null`, asserting the result
    set equals the unfiltered query (not "has any relation" / "has no
    relation").
  - Flip `every_treats_null_target_column_as_non_counterexample`'s
    `linkNull` assertion to excluded; rename to
    `every_treats_null_target_column_as_counterexample` (or similar) in the
    same task.
  - `every_vacuous_true_on_empty_relation_forward` /
    `..._reverse`: a row with zero related rows under `every` is included
    (vacuous true), both directions.
  - `every_nonempty_all_match_forward` / `..._reverse`: a row whose every
    related row matches the sub-filter is included, both directions.
  - `contains_literal_percent_and_underscore_not_wildcard`: insert a row with
    a literal `%`/`_` in the searched column plus a decoy row that would
    match if the characters were treated as wildcards; assert only the
    literal-match row is returned.
  - `contains_startswith_stay_case_insensitive`: regression-pin that
    `contains`/`startsWith` still match case-insensitively after the
    `ESCAPE`/`COLLATE` ordering change (no existing test covers this today;
    the COLLATE/ESCAPE clause order is exactly the kind of mistake that would
    silently regress it — see § Interfaces & contracts item 4).
  - `contains_null_value_still_matches_nothing`: `title: { contains: null }`
    on a plain `StringFilter` field (not inside a relation filter) still
    returns zero rows, unchanged from today — pins that the new `match`
    arm's non-`Text` passthrough (§ Interfaces & contracts item 4) really did
    preserve existing NULL behavior instead of silently becoming a no-op.
  - `EXPLAIN QUERY PLAN` assertion: issue a raw SQL
    `EXPLAIN QUERY PLAN SELECT 1 FROM "{junction}" WHERE "{ref_id}" = ?`
    (or drive it via a reverse-membership GraphQL query and inspect via the
    SQL engine's own `EXPLAIN` path if the e2e harness already exposes raw
    SQL execution — it does, per other scenarios in this file using
    `exec_sql`) and assert the plan text contains `USING INDEX` / does not
    contain `SCAN`, for a junction table created through the materialize.rs
    path. A second assertion (or a `ddb-core` integration test) covers the
    `sql_engine/ddl.rs:398` ALTER-COLUMN-ADD path so both creation paths are
    verified, matching the PRD's explicit "both creation paths inherit it"
    success metric.
- **Docs/CHANGELOG**: covered as Phase 2 tasks per the PRD; not re-specified
  here beyond noting the exact lines to touch (§ Module placement).

## Review log

dispatch 1 (claude): cardinal-sin 0, blocker 3, non-blocker 0, question 1

Fixed blockers: ESCAPE-before-COLLATE breaks case-insensitive LIKE matching
(reordered to `LIKE (...) COLLATE NOCASE ESCAPE '\'`); internal contradiction
over whether `sql_engine/ddl.rs` needs editing (corrected Module placement to
match § Interfaces & contracts item 6 — it is edited); null-skip guard
silently broadened beyond the PRD's literal `some`/`none`/`every` scope,
flipping unrelated id-op/inline-column NULL semantics with no test coverage
(narrowed the guard to the `QUANTIFIERS` branch only).

Recorded, not fixed (question): whether the junction-index fix should also
retrofit existing on-disk junction tables — addressed as a stated risk
instead (§ Risks & edge cases), consistent with this project's rebuildable-
index invariant; no action needed now.

dispatch 2 (claude-fallback): cardinal-sin 0, blocker 2, non-blocker 1, question 1

codex (dispatch 2's assigned reviewer) was unavailable this run: its sandbox
denied both of its file-reading tools (`sandbox-exec: sandbox_apply:
Operation not permitted`), so it returned no findings instead of a review;
degraded to a fresh Claude subagent per the Claude-fallback rule.

Fixed blockers: the `execute()`-errors-on-multi-statement rationale for the
junction-index fix was factually wrong (rusqlite silently drops statements
after the first instead of erroring — corrected the rationale text and the
Alternatives-considered entry to match); the LIKE-escape rewrite's
non-`Text` fallback was reachable (nullable `String` GraphQL input) and
silently flipped `contains`/`startsWith: null` from "matches nothing" to
"ignored" on ordinary (non-relation) `StringFilter` fields, outside this
PRD's scope (rewrote both arms as a `match` with a passthrough branch that
reproduces today's exact SQL/param binding for every non-`Text` value,
including `null`; added two e2e scenarios pinning case-insensitivity and the
preserved null behavior).

Recorded, not fixed (non-blocker): the same scope-creep root cause as
dispatch 1's narrowed-guard fix (`category: { eq: null }` inside a relation
filter) was independently flagged here too; already resolved by dispatch 1's
fix, so no separate action.

Recorded, not fixed (question): whether `tests/e2e/typed_where_filter.rs`
already exposes raw-SQL `exec_sql` for the `EXPLAIN QUERY PLAN` scenario — it
does (confirmed by dispatch 1's reviewer reading the same file, see the
`setup_every_fixture`/`exec_sql_then_sleep` helpers read during design
drafting); no change needed.

dispatch 3 (claude-fallback): cardinal-sin 0, blocker 0, non-blocker 1, question 0

codex was unavailable again this run (same sandbox file-read denial as
dispatch 2); degraded to a fresh Claude subagent per the Claude-fallback
rule. It independently re-verified every line-numbered claim in this doc
against the live code (the LIKE-arm `other`-branch byte-for-byte match, the
null-skip guard's QUANTIFIERS-only scoping, the `every` COALESCE diff
target, the `ddl.rs` module-placement consistency, and the stale-comment
fix) and found no cardinal sins or blockers.

Recorded, not fixed (non-blocker): `COLLATE NOCASE` on the LIKE pattern is
redundant — SQLite's `LIKE` already case-folds ASCII independent of
collation — harmless but worth knowing it is belt-and-suspenders, not
load-bearing, so a future reader doesn't assume removing it would break
case-insensitivity.

result: ok
