# Design: Declarative schema apply v2 — index evolution and FFI zone parity

## Architecture fit

Two independent capabilities land in the existing declarative-apply and FFI-discovery layers; neither needs a new module or a new layer.

- **Index evolution** extends the existing diff→plan→apply pipeline (`ddb-core/src/schema_diff/{differ,plan}.rs` → `ddb-core/src/service/schema_apply.rs` → `ddb-core/src/sql_engine/`). Today `differ.rs:293-296` pushes a `unique_together` delta straight into `plan.unsupported` instead of emitting a `PlanOp`; this PRD gives it a real op, following exactly the shape already used by `SetSearchKey`/`SetSingleton` (diff → `PlanOp` variant → `render_sql()` → custom-DDL regex interception in `SqlEngine::try_custom_ddl` → a handler that updates the typedef and calls `rematerialize_or_defer`).
- **FFI zone parity** extends `ddb-core/src/ffi/records.rs`'s `ColumnDefRecord`/`From<&ColumnDef>` mapping, the same place `references`/`required` already round-trip. No new discovery path; `list_type_schemas` (`ddb-core/src/ffi/driver.rs`) already calls `TypeSchemaRecord::from(schema)` which already calls `ColumnDefRecord::from` per column — only the record shape and the `From` impl change.

## Module placement

- **Edit** `ddb-core/src/schema_diff/differ.rs` — in `diff_type_attributes`, replace the `unique_together` branch (lines 293-296) that pushes to `plan.unsupported` with add/drop `PlanOp` emission (new function `diff_unique_together`, called from `diff_type_attributes`).
- **Edit** `ddb-core/src/schema_diff/plan.rs` — add `PlanOp::AddUniqueConstraint { table: String, columns: Vec<String> }` and `PlanOp::DropUniqueConstraint { table: String, columns: Vec<String> }`; extend `render_sql`, `kind`, `describe`, `affected_table`, `is_destructive` (drop is destructive, matching `DropColumn`/`RenameColumn`'s treatment — see Risks).
- **New file** `ddb-core/src/sql_engine/ddl/unique_ops.rs` (sibling to `zone_ops.rs`, same `impl<'a> SqlEngine<'a>` pattern) — `handle_add_unique_constraint` / `handle_drop_unique_constraint`.
- **Edit** `ddb-core/src/sql_engine/helpers/regexes.rs` — add `re_add_unique()` / `re_drop_unique()` matching the two new `render_sql()` outputs.
- **Edit** `ddb-core/src/sql_engine/mod.rs` — two new arms in `try_custom_ddl` (same pattern as the existing six), dispatching to the new handlers.
- **Edit** `ddb-core/src/sql_engine/ddl/mod.rs` — `pub(crate) mod unique_ops;` declaration.
- **Edit** `ddb-core/src/ffi/records.rs` — add `pub zone: String` to `ColumnDefRecord`; populate it in `From<&ColumnDef>`.
- **No change** to `ddb-core/src/ffi/driver.rs` — `list_type_schemas` already flows through `TypeSchemaRecord::from` → `ColumnDefRecord::from`; the zone appears automatically once the record/mapping change lands.
- **No change** to `ddb-core/src/indexer/materialize/tables.rs` or `schema_helpers.rs` — the materializer already renders `unique_together` from the typedef (`{table}_unique_{cols.join("_")}` index name, `ddb-core/src/sql_engine/ddl/create.rs:329-349`, mirrored by the materializer for full rebuilds); updating the typedef and calling `rematerialize_or_defer` is sufficient for self-healing on reindex, per the 00161 contract.

## Interfaces & contracts

### `PlanOp` (ddb-core/src/schema_diff/plan.rs)

```rust
pub enum PlanOp {
    // ...existing variants unchanged...
    AddUniqueConstraint { table: String, columns: Vec<String> },
    DropUniqueConstraint { table: String, columns: Vec<String> },
}
```

- `render_sql()`:
  - `AddUniqueConstraint { table, columns }` → `format!("ALTER TABLE {} ADD UNIQUE ({})", quote_ident(table), columns.iter().map(|c| quote_ident(c)).collect::<Vec<_>>().join(", "))`
  - `DropUniqueConstraint { table, columns }` → `format!("ALTER TABLE {} DROP UNIQUE ({})", quote_ident(table), columns.iter().map(|c| quote_ident(c)).collect::<Vec<_>>().join(", "))`
  - Never ends with `;` (matches every other arm's contract).
- `kind()`: `"add_unique_constraint"` / `"drop_unique_constraint"`.
- `describe()`: `format!("add unique constraint ({}) on {table}", columns.join(", "))` / the `drop` mirror.
- `affected_table()`: add both new variants to the existing match arm's `table` binding (same style as every other variant).
- `is_destructive()`: `DropUniqueConstraint` returns `true` (alongside `DropColumn`/`RenameColumn`); `AddUniqueConstraint` is additive, stays `false`. This makes an unguarded drop participate in the existing `plan.has_destructive() && !cmd.allow_destructive` gate in `schema_apply.rs:84` — no new gate needed.

### `differ.rs`

Replace lines 293-296 with:

```rust
diff_unique_together(desired, current, plan);
```

New function:

```rust
/// Diff `unique_together` on an existing type: one `AddUniqueConstraint` per
/// desired constraint absent from current, one `DropUniqueConstraint` per
/// current constraint absent from desired. Order-independent per constraint
/// (a constraint is a `Vec<String>` of column names, compared as a set via
/// a sorted clone — `["a","b"]` and `["b","a"]` are the same constraint).
fn diff_unique_together(desired: &TableSchema, current: &TableSchema, plan: &mut SchemaPlan) {
    let table = &desired.table_name;
    let desired_sets: Vec<Vec<String>> = normalize_constraints(&desired.unique_together);
    let current_sets: Vec<Vec<String>> = normalize_constraints(&current.unique_together);

    for cols in &desired_sets {
        if !current_sets.contains(cols) {
            plan.ops.push(PlanOp::AddUniqueConstraint {
                table: table.clone(),
                columns: cols.clone(),
            });
        }
    }
    for cols in &current_sets {
        if !desired_sets.contains(cols) {
            plan.ops.push(PlanOp::DropUniqueConstraint {
                table: table.clone(),
                columns: cols.clone(),
            });
        }
    }
}

/// `unique_together: Option<Vec<Vec<String>>>` → sorted-per-constraint sets,
/// empty constraints dropped (mirrors the `cols.is_empty()` skip already in
/// `ddl/create.rs:332` and `plan.rs`'s `render_create_type` path).
fn normalize_constraints(raw: &Option<Vec<Vec<String>>>) -> Vec<Vec<String>> {
    raw.as_ref()
        .map(|cs| {
            cs.iter()
                .filter(|c| !c.is_empty())
                .map(|c| {
                    let mut sorted = c.clone();
                    sorted.sort();
                    sorted
                })
                .collect()
        })
        .unwrap_or_default()
}
```

This replaces the whole-field `desired.unique_together != current.unique_together` comparison (which was order- and dedup-sensitive) with a set-of-sets comparison, so re-apply with the same logical constraints in a different YAML order is correctly a no-op.

### `handle_add_unique_constraint` / `handle_drop_unique_constraint` (new file `ddb-core/src/sql_engine/ddl/unique_ops.rs`)

**Pre-commit duplicate check (replaces the original "let CREATE UNIQUE INDEX fail inside rematerialize" plan — see Review log dispatch 1, cardinal sin).** `handle_commit` (`ddb-core/src/sql_engine/transaction.rs:31-81`) flushes the buffered git writes (`commit_batch`, line 63) **before** running the buffered `rematerialize_type` calls (lines 70-71); a rematerialize failure there propagates out of `commit_transaction()` with the git commit already permanent and no rollback of it (`Drop` only cleans up the SQLite savepoint side). That means letting a duplicate-row UNIQUE violation surface inside the deferred rematerialize — the original plan — would commit a typedef to git declaring a `unique_together` that was never actually materialized, with no distinguishing error and no self-heal. This is a pre-existing gap in `commit_transaction()` shared by every op that defers through `rematerialize_or_defer` (e.g. `SetSingleton`), but THIS PRD is the first to introduce an op whose rematerialize can routinely fail on pre-existing data (the PRD's own named Error Case), so it must not inherit that gap silently. Fixing `commit_transaction()` itself is out of scope (blast radius: every deferred-rematerialize op, not just this PRD's). Instead, `handle_add_unique_constraint` checks for duplicates **before** calling `update_typedef` — i.e. before anything enters the transaction buffer at all — so a violation is caught while the per-op loop in `schema_apply.rs`'s existing `SCHEMA_APPLY_PARTIAL`/`rollback_transaction()` path is still the active guard (nothing has been buffered yet for this op, so the rollback has nothing of this op's to undo, and no git write happens).

```rust
impl<'a> SqlEngine<'a> {
    pub(in crate::sql_engine) fn handle_add_unique_constraint(
        &mut self,
        table_name: &str,
        columns: &[String],
    ) -> Result<SqlResult> {
        let mut schema = self.load_schema(table_name)?;
        let mut sorted = columns.to_vec();
        sorted.sort();
        let mut constraints = schema.unique_together.unwrap_or_default();
        if constraints.iter().any(|c| {
            let mut s = c.clone();
            s.sort();
            s == sorted
        }) {
            return Ok(SqlResult::Ok(format!(
                "unique constraint ({}) already present on {table_name}",
                columns.join(", ")
            )));
        }
        // Fail BEFORE any git/typedef write: a duplicate-row violation caught
        // here never enters the transaction buffer, so the caller's existing
        // per-op rollback (schema_apply.rs's SCHEMA_APPLY_PARTIAL path) is
        // still in control and no git commit has happened yet. Letting SQLite
        // discover the violation later, inside commit_transaction()'s deferred
        // rematerialize (after the git commit lands), is NOT safe — see the
        // paragraph above.
        let col_list = columns
            .iter()
            .map(|c| format!("\"{c}\""))
            .collect::<Vec<_>>()
            .join(", ");
        let dup_count: i64 = self.index.sql_conn().query_row(
            &format!(
                "SELECT COUNT(*) FROM (SELECT 1 FROM \"{table_name}\" GROUP BY {col_list} HAVING COUNT(*) > 1)"
            ),
            [],
            |row| row.get(0),
        )?;
        if dup_count > 0 {
            return Err(DoogatError::Structured {
                code: codes::UNIQUE_VIOLATION,
                message: format!(
                    "cannot add unique constraint ({}) on {table_name}: {dup_count} duplicate group(s) exist; remove duplicates and retry",
                    columns.join(", ")
                ),
                context: vec![],
            });
        }
        constraints.push(columns.to_vec());
        schema.unique_together = Some(constraints);
        self.update_typedef(table_name, &schema)?;
        // Safe now: no further duplicate-row failure is possible, so the
        // deferred rematerialize_type (run after commit_transaction's git
        // commit) only ever rebuilds successfully.
        self.rematerialize_or_defer(table_name)?;
        Ok(SqlResult::Ok(format!(
            "unique constraint ({}) added on {table_name}",
            columns.join(", ")
        )))
    }

    pub(in crate::sql_engine) fn handle_drop_unique_constraint(
        &mut self,
        table_name: &str,
        columns: &[String],
    ) -> Result<SqlResult> {
        let mut schema = self.load_schema(table_name)?;
        let mut sorted = columns.to_vec();
        sorted.sort();
        let mut constraints = schema.unique_together.unwrap_or_default();
        let before = constraints.len();
        constraints.retain(|c| {
            let mut s = c.clone();
            s.sort();
            s != sorted
        });
        if constraints.len() == before {
            return Ok(SqlResult::Ok(format!(
                "unique constraint ({}) already absent on {table_name}",
                columns.join(", ")
            )));
        }
        schema.unique_together = if constraints.is_empty() { None } else { Some(constraints) };
        self.update_typedef(table_name, &schema)?;
        self.rematerialize_or_defer(table_name)?;
        Ok(SqlResult::Ok(format!(
            "unique constraint ({}) dropped on {table_name}",
            columns.join(", ")
        )))
    }
}
```

Both handlers follow `handle_set_singleton`/`handle_drop_singleton`'s exact shape (`ddb-core/src/sql_engine/ddl/zone_ops.rs:133-172`): load typedef, mutate the relevant field, `update_typedef`, `rematerialize_or_defer`. No direct `CREATE UNIQUE INDEX`/`DROP INDEX` SQL is issued by the handler — `rematerialize_type` (`ddb-core/src/indexer/materialize/tables.rs`) already rebuilds the table from the typedef it just wrote, including the unique index, via the same code path `ddl/create.rs:329-349` exercises for `CREATE TABLE`. This is also what makes drop-then-reindex self-healing per the 00161 contract (Key Invariant: "the derived index rebuilt/altered on apply; self-heals on reindex").

Idempotency (PRD Success Metric: "re-apply is an idempotent no-op"): the differ's set-of-sets comparison means a re-apply with the same `unique_together` produces an empty plan — these handlers' own idempotent "already present/absent" branches are a defensive second layer, exercised only if `apply_schema` is ever called with a hand-built plan.

### Regexes (`ddb-core/src/sql_engine/helpers/regexes.rs`)

Two new regexes, same construction pattern as `re_set_singleton`/`re_drop_singleton` (anchor `^ALTER TABLE`, case-insensitive, table name in a quoted-or-bare capture group):

```rust
pub fn re_add_unique() -> &'static Regex {
    // ALTER TABLE "tbl" ADD UNIQUE (col1, col2)  — quoted cols, comma+space joined by render_sql
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"(?i)^\s*ALTER\s+TABLE\s+(?:"([^"]+)"|(\w[\w-]*))\s+ADD\s+UNIQUE\s*\(([^)]+)\)\s*;?\s*$"#).unwrap()
    })
}

pub fn re_drop_unique() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"(?i)^\s*ALTER\s+TABLE\s+(?:"([^"]+)"|(\w[\w-]*))\s+DROP\s+UNIQUE\s*\(([^)]+)\)\s*;?\s*$"#).unwrap()
    })
}
```

Flexible whitespace and an optional trailing `;`, matching the real `re_set_singleton`/`re_drop_singleton` pattern (`ddb-core/src/sql_engine/helpers/regexes.rs:63-80`) rather than the stricter single-space/no-semicolon form the first draft used — a direct `ALTER TABLE t ADD UNIQUE (a, b);` typed through the raw-SQL surface (same way `SET SINGLETON;` is directly invocable per `zone_ops.rs:125-132`'s doc comment) must match this regex too, not just the apply-pipeline's own `render_sql()` output which never emits a trailing `;` (pinned by `plan.rs`'s `render_sql_has_no_trailing_semicolon` test).

Column-list capture group 3 is split on `", "` and each piece unquoted (strip surrounding `"`) before being passed to the handler — mirrors how `re_set_zone`'s multi-group capture is consumed in `try_custom_ddl`, just with a split instead of a single `.as_str()`.

### `try_custom_ddl` (`ddb-core/src/sql_engine/mod.rs`)

Two new arms inserted after the existing `re_drop_singleton` arm, same shape:

```rust
if let Some(caps) = re_add_unique().captures(sql) {
    let table = caps.get(1).or(caps.get(2)).expect("regex guarantees group 1 or 2").as_str();
    let columns = parse_quoted_column_list(caps.get(3).expect("regex guarantees group 3").as_str());
    return Ok(Some(self.handle_add_unique_constraint(table, &columns)?));
}
if let Some(caps) = re_drop_unique().captures(sql) {
    let table = caps.get(1).or(caps.get(2)).expect("regex guarantees group 1 or 2").as_str();
    let columns = parse_quoted_column_list(caps.get(3).expect("regex guarantees group 3").as_str());
    return Ok(Some(self.handle_drop_unique_constraint(table, &columns)?));
}
```

`parse_quoted_column_list(s: &str) -> Vec<String>` is a small private helper in `unique_ops.rs` (or inlined in `mod.rs` next to `try_custom_ddl` if under 10 lines): `s.split(", ").map(|c| c.trim_matches('"').to_string()).collect()`.

### `ColumnDefRecord` (`ddb-core/src/ffi/records.rs`)

```rust
#[derive(uniffi::Record)]
pub struct ColumnDefRecord {
    pub name: String,
    pub data_type: String,
    pub references: Option<String>,
    pub required: bool,
    pub zone: Option<String>,
}

impl From<&crate::types::ColumnDef> for ColumnDefRecord {
    fn from(c: &crate::types::ColumnDef) -> Self {
        Self {
            name: c.name.clone(),
            data_type: c.data_type.clone(),
            references: c.references.clone(),
            required: c.required,
            zone: c.zone.as_ref().map(zone_token),
        }
    }
}

fn zone_token(zone: &crate::types::Zone) -> String {
    match zone {
        crate::types::Zone::Frontmatter => "frontmatter",
        crate::types::Zone::Body => "body",
        crate::types::Zone::Reference => "reference",
    }
    .to_string()
}
```

**Corrected from the first draft** (Review log dispatch 1, blocking finding): the actual GraphQL schema-introspection mapping is `column_def_to_value` (`ddb-server/src/schema/base_types/to_gql.rs:345-355`, NOT the row-value-extraction function `extract_typed_field` near line 448 that the first draft cited by mistake): `c.zone.as_ref().map(|z| GqlValue::from(zone_str(z))).unwrap_or(GqlValue::Null)` — GraphQL returns **null** for a column with no declared zone, it does NOT default to `"frontmatter"`. `ColumnDefRecord.zone` must therefore be `Option<String>` with no default applied, mirroring `column_def_to_value` exactly (`map`, no `unwrap_or` fallback value) rather than `extract_typed_field`'s unrelated row-reading default. This is additive and non-breaking per the PRD's own framing ("additive optional field") — this corrected version actually matches that framing, whereas the first draft's non-null `String` with a hardcoded default did not.

## Data flow

**Index evolution:** `apply_schema(cmd)` → `SchemaDoc::from_yaml` → `describe_type` per type (live `TableSchema`, including live `unique_together`) → `schema_diff::diff(&doc, &live)` → `diff_type_attributes` → `diff_unique_together` emits `PlanOp::{Add,Drop}UniqueConstraint` into `plan.ops` → (if not dry-run, not empty, destructive-gate checked) `self.begin_transaction()` → for each op, `self.execute_sql(&op.render_sql())` → `try_custom_ddl` regex-matches the `ALTER TABLE ... UNIQUE (...)` text → `handle_{add,drop}_unique_constraint` → `update_typedef` (buffers the typedef git write inside the open transaction) → `rematerialize_or_defer` (buffers the table name; real rebuild happens when the transaction's buffered rematerializations drain after `commit_transaction()` flushes the single git commit) → `commit_transaction()` runs the deferred `rematerialize_type` calls, which read the now-current typedef and rebuild the SQLite table + its unique index from the git-committed data.

**FFI zone parity:** `list_type_schemas()` (ddb-core/src/ffi/driver.rs, unchanged) → loads each registered type's `TableSchema` (already carries `ColumnDef.zone: Option<Zone>` — PRD's own framing, nothing new to compute) → `TypeSchemaRecord::from(schema)` → `ColumnDefRecord::from(&col)` per column → the new `zone` field is populated from the same `ColumnDef.zone` GraphQL and the typedef YAML already read.

## Reuse inventory

- `ddb-core/src/sql_engine/ddl/zone_ops.rs` (`handle_set_singleton`/`handle_drop_singleton`/`handle_search_key`) — the exact handler shape (load typedef → mutate → `update_typedef` → `rematerialize_or_defer`) this PRD's two new handlers copy.
- `ddb-core/src/sql_engine/helpers/regexes.rs` (`re_set_singleton`, `re_drop_singleton`, `re_set_search_key`) — the custom-DDL regex pattern (anchored `ALTER TABLE`, quoted-or-bare table capture) this PRD's two new regexes copy.
- `ddb-core/src/sql_engine/mod.rs::try_custom_ddl` — the dispatch chain; two new arms appended, nothing restructured.
- `ddb-core/src/sql_engine/ddl/create.rs:329-349` — the existing `CREATE UNIQUE INDEX IF NOT EXISTS "{table}_unique_{cols}"` naming/rendering the materializer already applies on `CREATE TABLE`; `rematerialize_type` reuses the identical path on rebuild, so the new ops need no new index-naming logic of their own.
- `ddb-core/src/service/schema_apply.rs:84` (`plan.has_destructive() && !cmd.allow_destructive`) — the existing destructive-op gate; `DropUniqueConstraint::is_destructive() == true` is the only hook this PRD needs into it.
- `ddb-core/src/ffi/records.rs` `ZoneRecord`/`From<Zone>` (lines 453-465) — an existing FFI-safe zone enum for a *different* record (frontmatter extra-field values). Not reused directly: `ColumnDefRecord.zone` is a plain `String` to byte-match GraphQL's own string contract (`to_gql.rs::zone_str`), not the enum. Searched for an existing `zone_str`/`zone_token`-shaped helper reusable across crates — none exists (`ddb-server`'s `zone_str` is private and ddb-core cannot depend on ddb-server), so `zone_token` is a small new private fn, not a cross-crate reuse.
- `ddb-core/src/ffi/driver.rs::list_type_schemas` → `TypeSchemaRecord::from` → `ColumnDefRecord::from` — the existing discovery call chain; no new entry point.
- Searches run (verb/noun synonyms): `rg "unique_together"`, `rg "UniqueConstraint|UNIQUE INDEX"`, `rg "SCHEMA_UNSUPPORTED_CHANGE"`, `rg "zone" ddb-server/src/schema` (found `zone_str`/`to_gql.rs`), `rg "ColumnDefRecord|ZoneRecord" ddb-core/src/ffi`, `rg "re_set_singleton|re_drop_singleton|re_set_search_key"`, `rg "rematerialize_or_defer|rematerialize_type"`. All confirmed the integration points above; nothing found that already does index-evolution diffing or FFI zone exposure (consistent with the PRD's own grounding note).

## Alternatives considered

1. **(chosen) Extend the existing diff/plan/apply pipeline with two new `PlanOp` variants + two new regex-dispatched handlers**, following the `SetSingleton`/`SetSearchKey` precedent exactly. Smallest diff that stays inside the existing single-commit-atomic, self-healing-on-reindex contract; no new abstractions.
2. **Smallest-diff variant: a single `PlanOp::AlterUniqueConstraint { table, add: Vec<Vec<String>>, drop: Vec<Vec<String>> }` carrying both directions in one op**, rendered as one compound `ALTER TABLE ... ADD UNIQUE (...), DROP UNIQUE (...)` statement. Rejected: every other multi-effect diff in this codebase (columns, zones) already emits one `PlanOp` per atomic change, and a compound op would need its own bespoke dry-run `describe()`/report rendering instead of reusing the existing per-op `PlanOpReport` one-line-per-op output (`SchemaApplyReport::from_plan`). Two ops is not meaningfully bigger and stays uniform with the rest of the plan model.
3. **Model `unique_together` as a true secondary-index system** (named indexes, `CREATE INDEX`/`DROP INDEX` surfaced to SQL, independent of `unique_together`). Rejected outright — the PRD's own Risk/Scope section is explicit that Doogat DB has no general secondary-index concept and `unique_together` is the only declarable uniqueness form; inventing one here would be scope creep the PRD forbids.

## Risks & edge cases

- **Index migration on data with existing duplicates** (PRD Risk): resolved structurally, not left to the rematerialize step. `commit_transaction()` (`ddb-core/src/sql_engine/transaction.rs:31-81`) commits buffered git writes (line 63) BEFORE running deferred `rematerialize_type` calls (lines 70-71) with no rollback of the git commit on a later failure — a pre-existing gap in the generic transaction path, shared by every op that uses `rematerialize_or_defer`, out of scope to fix generically here. `handle_add_unique_constraint` avoids ever exercising that gap by running an explicit `GROUP BY ... HAVING COUNT(*) > 1` duplicate check against the live materialized table BEFORE calling `update_typedef` — i.e. before this op buffers anything into the transaction. A duplicate-row violation returns a structured `UNIQUE_VIOLATION` error (reusing the existing error-code vocabulary, Key Invariant 26) while the caller's existing per-op `SCHEMA_APPLY_PARTIAL`/`rollback_transaction()` path in `schema_apply.rs` is still the active guard and nothing has reached git. See `## Interfaces & contracts`.
- **Atomicity with the derived index** (PRD Risk): covered structurally — same pattern as `SetSingleton`, already proven by that feature's own tests.
- **FFI record shape change** (PRD Risk): additive field on an existing `uniffi::Record`; confirmed non-breaking because UniFFI records use named-field construction in generated Swift/Kotlin, and this PRD does not add the field to any FFI *function signature* (input), only to an output record.
- **Scope** (PRD Risk): `diff_unique_together` only ever reads/writes `TableSchema.unique_together`; no new `PlanOp` variant touches any other index-like concept.
- **Next likely changes after this PRD** (to avoid boxing in):
  1. A future PRD giving `unique_together` evolution a CLI/GraphQL-level dry-run preview with per-constraint add/drop breakdown — the one-`PlanOp`-per-constraint design (Alternative 1 over 2) already supports this without rework, since `SchemaApplyReport::from_plan` already reports one row per op.
  2. A future general secondary-index feature (explicitly out of scope here) would need its own `PlanOp` family — this design does not reuse or overload `AddUniqueConstraint`/`DropUniqueConstraint` for that, so it does not block or need renaming later.
  3. FFI zone parity is one column attribute of several the PRD framing calls "cross-interface introspection parity" — `allowed_values`, `default_value`, `search_boost`, `on_delete` are already on `ColumnDef` but not on `ColumnDefRecord`. This PRD does not add them (out of scope), but the `zone_token`-style small-conversion-function pattern this PRD establishes is the template a follow-up parity PRD would reuse per field.

## Test strategy outline

Per-task local gate: `cargo build`, `cargo clippy --workspace --all-targets`, `cargo test-ci` (colocated unit tests included).

- **Phase 0 (differ unit tests, colocated in `differ.rs`)**: `unique_together` add emits `AddUniqueConstraint`; drop emits `DropUniqueConstraint`; a same-set-different-order `unique_together` (e.g. desired `[["a","b"]]` vs current `[["b","a"]]`) diffs to an empty plan (no-op); a type with no `unique_together` change on either side touches no new op.
- **Phase 1 (schema_apply integration tests, colocated in `schema_apply.rs`, following the existing `apply_hyphenated_unique_together_succeeds` test's shape)**: applying a desired schema that adds a `unique_together` to an EXISTING type (not `CREATE TABLE`) migrates the live type and the materialized unique index actually rejects a duplicate insert afterward; re-apply of the identical desired schema is `applied: false` / empty plan (idempotent); dropping a `unique_together` then re-adding it round-trips; adding a `unique_together` to a type that already holds duplicate rows surfaces a structured, non-silent error (not a panic, not a quiet skip) — this is the PRD's own named Error Case scenario.
- **Phase 2 (new e2e, Rust harness per CLAUDE.md/AGENTS.md — the shell harness is retired)**: a `tests/e2e/` scenario applying a `unique_together` migration via CLI `ddb schema apply` on an existing type, asserting the duplicate-rejection behavior end to end; a GraphQL `applySchema` scenario asserting the same through the server; an FFI unit test asserting `list_type_schemas()`'s returned `zone` matches the same type's GraphQL introspection value for a type with at least one `body` and one `reference` column (not just the default `frontmatter`, so the non-default arms of `zone_token` are exercised).
- **Regression guard**: existing 00161 apply tests (CREATE-time `unique_together`) and 00160 zone tests (GraphQL/YAML) must stay green unmodified — this PRD only adds code paths, it does not touch their behavior.
- Add a colocated unit test asserting the pre-commit duplicate check: adding a `unique_together` against a table with a duplicate-row group returns `UNIQUE_VIOLATION` and the typedef is left unchanged (no git write at all, not merely "rolled back").
- Add an FFI unit test asserting `ColumnDefRecord.zone` is `None` for a column with no declared zone (not `Some("frontmatter")`), matching `column_def_to_value`'s null behavior, alongside the positive case for each of the three declared zones.

## Review log

Dispatch 1 (claude) findings and resolution:

- **[cardinal sin, FIXED]** No rollback plan for the stateful add-unique-constraint write: `commit_transaction()` commits git before running deferred `rematerialize_type`, with no rollback of the git commit on a later rematerialize failure — the original design let a duplicate-data UNIQUE violation surface there, which would have committed a typedef declaring an index that was never materialized. Fixed by moving the duplicate check to an explicit pre-commit `GROUP BY ... HAVING COUNT(*) > 1` query inside `handle_add_unique_constraint`, before `update_typedef` — see `## Interfaces & contracts` and `## Risks & edge cases`.
- **[blocking, FIXED]** `ColumnDefRecord.zone` was designed as non-null `String` defaulting to `"frontmatter"`, citing the wrong GraphQL function (`extract_typed_field`'s row-value default, not the schema-introspection mapping). The actual mapping, `column_def_to_value`, returns GraphQL `null` for an undeclared zone. Fixed: `zone: Option<String>`, `None` for `ColumnDef.zone == None`, matching `column_def_to_value` exactly.
- **[non-blocking, recorded]** `diff_unique_together`/`normalize_constraints` sorts each constraint's columns for comparison, and the sorted (not originally-declared) order is what gets persisted back into the typedef by the handler — a user-declared `[b, a]` round-trips as `[a, b]` in the YAML and the index name. Semantically harmless and stays idempotent, but is a silent reordering of user-authored YAML never called out as a side effect. Not fixed in this design; acceptable for v1, worth a one-line doc note or a future "preserve declared order for storage, sort only for comparison" tweak if a downstream diffs typedef YAML directly.
- **[question, recorded]** `parse_quoted_column_list`'s naive `split(", ")` + `trim_matches('"')` does not undo `quote_ident`'s `"` → `""` escaping and would mis-split a column name containing the literal substring `", "`. No existing single-column custom-DDL op needed this. Scoped assumption for this design: typedef column names are plain identifiers (letters/digits/hyphen/underscore, per every existing example in the codebase, e.g. `long-desc`), so neither case arises in practice; not fixed, flagged for implementation-time attention if that assumption ever changes.

dispatch 1 (claude): cardinal-sin 1, blocker 1, non-blocker 1, question 1

Dispatch 2 (claude-fallback — codex unavailable this run: its sandbox denied its own filesystem reads before any tool call, `sandbox_apply: Operation not permitted`; degraded per the skill's Claude-fallback rule) findings and resolution:

- Independently re-verified both dispatch-1 fixes against the real source (not the doc's own narration) and confirmed both hold: the pre-commit duplicate check genuinely avoids the `commit_transaction()` git-before-rematerialize gap, and `ColumnDefRecord.zone: Option<String>` genuinely matches `column_def_to_value`'s null behavior.
- **[non-blocker, FIXED]** The two new regexes (`re_add_unique`/`re_drop_unique`) were stricter (single literal space, no optional `;`) than every existing custom-DDL regex they claim to copy (`re_set_singleton`/`re_drop_singleton` use `\s+`/`\s*;?\s*$`). Harmless for the apply pipeline itself (`render_sql()` never emits a trailing `;`, pinned by its own test), but a direct hand-typed `ALTER TABLE t ADD UNIQUE (a, b);` through the raw-SQL surface — the same way `SET SINGLETON;` is documented as directly invocable — would fall through to the generic `sqlparser` path and fail with a confusing parse error instead of structured handling. Fixed: both regexes now use the same flexible-whitespace, optional-trailing-semicolon pattern as the rest of the custom-DDL family.
- The two items dispatch 1 already recorded as non-blocker/question (silent column-order normalization; the quoted-column-list parser's assumption that identifiers never contain `, ` or embedded `"`) were re-examined and carried over unchanged — no new information, correctly scoped as-is.

dispatch 2 (claude-fallback): cardinal-sin 0, blocker 0, non-blocker 2, question 1
