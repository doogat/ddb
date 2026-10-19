# Design: Split oversized server/core modules (round 2)

## Architecture fit

Pure internal refactor across `ddb-server/src/filter.rs`, `ddb-server/src/schema/queries.rs`,
`ddb-server/src/schema/base_types.rs`, and ten `ddb-core` modules
(`sql_engine/{helpers,dml,ddl,builders}.rs`, `indexer/{materialize,graph,search}.rs`,
`parser/mod.rs`, `crdt_resolver.rs`, `git_ops/mod.rs`), plus a `traits.rs`
exception-disposition task. No layer, crate boundary, or public capability
changes — this follows the exact pattern PRD 00156 already established for
`crud.rs`, `indexer/mod.rs`, `ffi.rs`, and `mutations.rs`: a single `*.rs`
file becomes a `*/` directory with `mod.rs` re-exporting the prior public
surface and sibling files holding the moved bodies.

**Line counts must be re-measured at execution, not trusted from the PRD.**
Measured now (current `master`), several differ substantially from the
PRD's 2026-09-05 snapshot — growth from PRDs 00176-00178 landing since:

| Module | PRD snapshot | Current | Status |
|---|---|---|---|
| `ddb-server/src/filter.rs` | ~3397 | 3397 | unchanged, still oversized |
| `ddb-server/src/schema/queries.rs` | ~842 (fn) | 909 (file) | still oversized |
| `ddb-core/src/sql_engine/helpers.rs` | ~1544 | 1544 | unchanged |
| `ddb-core/src/sql_engine/dml.rs` | 1663 | 1417 | still oversized |
| `ddb-core/src/sql_engine/ddl.rs` | 1654 | 1654 | unchanged |
| `ddb-core/src/indexer/materialize.rs` | 1309 | 1276 | still oversized |
| `ddb-server/src/schema/base_types.rs` | 1113 | **1849** | grew significantly |
| `ddb-core/src/indexer/graph.rs` | 1020 | 1020 | unchanged |
| `ddb-core/src/parser/mod.rs` | 978 | 991 | still oversized |
| `ddb-core/src/sql_engine/builders.rs` | 916 | 916 | unchanged |
| `ddb-core/src/crdt_resolver.rs` | 871 | **1508** | grew significantly |
| `ddb-core/src/git_ops/mod.rs` | 851 | 1135 | grew |
| `ddb-core/src/indexer/search.rs` | 812 | 812 | unchanged |

Every module is still over 800 lines (none dropped below the threshold the
way `sync_manager/mod.rs` did), so the PRD's module list stands unchanged —
but `base_types.rs` and `crdt_resolver.rs` grew enough that each may need
**more than one** split seam to land every resulting file at or under 800
lines. This is a quantity-of-submodules detail, not a scope change: still
one task per PRD-listed module/group, same re-export pattern.

**Noted but out of scope**: `ddb-core/src/indexer/mod.rs` is now 900 lines
(it was the Phase 1 split target of PRD 00156 and has grown past 800 again
since). The PRD does not name it, and this design does not add it — it is a
candidate for a future round-3 sweep, not a silent scope addition here.

## Module placement

All changes are **pure moves**: existing code relocates into new sibling
files under a new `<module>/` directory; no file keeps its old flat path once
split (it becomes `<module>/mod.rs`). No new behavior, no new public items.

- `ddb-server/src/filter.rs` → `ddb-server/src/filter/{mod.rs,where_build.rs,junction.rs,reverse.rs,tests.rs}` (per the PRD's own Repository Structure tree — reused verbatim as the split boundary, see § Reuse inventory).
- `ddb-server/src/schema/queries.rs` → stays a single file (909 lines, not a directory split); `build_query_fields` decomposed internally into named per-field-group helper functions it calls, bringing the top-level function under 50 lines. If the file itself still exceeds 800 lines after decomposition, split into `schema/queries/{mod.rs, builders.rs}` using the same re-export pattern — re-measure after decomposition, before deciding.
- `ddb-core/src/sql_engine/helpers.rs` → `ddb-core/src/sql_engine/helpers/{mod.rs,regexes.rs,inline_zone.rs,tests.rs}` (per the PRD's tree).
- `ddb-core/src/sql_engine/dml.rs` → `ddb-core/src/sql_engine/dml/{mod.rs, ...}` — exact sibling file names decided per the pure-move seams found in the file at execution time (e.g. by statement kind: insert/update/delete/validation), since the PRD does not pin this one's internal shape.
- `ddb-core/src/sql_engine/ddl.rs` → `ddb-core/src/sql_engine/ddl/{mod.rs, ...}` — same rule.
- `ddb-core/src/indexer/materialize.rs` → `ddb-core/src/indexer/materialize/{mod.rs, ...}` — same rule.
- `ddb-server/src/schema/base_types.rs` → `ddb-server/src/schema/base_types/{mod.rs, ...}` — same rule; given its growth to 1849 lines, expect 2-3 sibling files, not one.
- `ddb-core/src/indexer/graph.rs` → `ddb-core/src/indexer/graph/{mod.rs, ...}` — same rule.
- `ddb-core/src/parser/mod.rs` → stays `parser/mod.rs` as the re-export root, with new siblings under `ddb-core/src/parser/` (it is already a directory, unlike the others) — same rule.
- `ddb-core/src/sql_engine/builders.rs` → `ddb-core/src/sql_engine/builders/{mod.rs, ...}` — same rule.
- `ddb-core/src/crdt_resolver.rs` → `ddb-core/src/crdt_resolver/{mod.rs, ...}` — same rule; given growth to 1508 lines, expect 2 sibling files.
- `ddb-core/src/git_ops/mod.rs` → stays `git_ops/mod.rs` as the re-export root, with new siblings under `ddb-core/src/git_ops/` (already a directory) — same rule.
- `ddb-core/src/indexer/search.rs` → `ddb-core/src/indexer/search/{mod.rs, ...}` — same rule.
- `ddb-core/src/traits.rs` — edited in place (no split; it is 601 lines, under the cap), to record the `sql_conn` exception disposition per the PRD's decided-at-execution rule.
- `docs/src/architecture/design-decisions.md` — edited for the `sql_conn` disposition record.
- `docs/src/technical/walkthrough.md` / other module docs — edited only if they reference a module boundary that moved.

## Interfaces & contracts

**The one re-export contract every split obeys (verbatim-ready, applies to
every module in § Module placement except `queries.rs` and `traits.rs`):**

1. The original flat file `path/to/module.rs` is replaced by a directory
   `path/to/module/` containing `mod.rs` plus one or more sibling `*.rs`
   files holding the moved code.
2. `mod.rs` re-exports **every** item the original file exposed, at its
   **original visibility** (`pub`, `pub(crate)`, `pub(super)`) and **original
   path** (`crate::module::Item` must still resolve identically after the
   split — callers and tests outside the module are not touched). Use
   `pub use submodule::*;` where the submodule's items are all meant to be
   visible at the old path, or named `pub use submodule::{Item1, Item2};`
   re-exports when only some items should surface — match whichever the
   original file's own visibility already implied (if `Item1` was `pub` and
   `Item2` was private, the split must preserve that, not flatten both to
   `pub`).
3. The colocated `#[cfg(test)] mod tests { ... }` block moves into its own
   sibling file (named `tests.rs` where the PRD's tree names it explicitly —
   `filter/tests.rs`, `helpers/tests.rs` — and analogously named for the
   other modules) and is declared from `mod.rs` as `#[cfg(test)] mod tests;`
   exactly as it is declared today, just pointing at the new file instead of
   an inline block.
4. No function, struct, enum, trait, const, or impl block changes shape,
   name, signature, or body during the move — this is verified per-item by
   diffing the moved body against the pre-split file (the 00156-style
   verification named in the PRD's Success Metrics and Risks).
5. Every resulting production `*.rs` file (tests files excluded, per the
   PRD's own "800-line file guideline (rubric R13)" framing, which the PRD's
   Problem Statement explicitly scopes to "production modules") is <= 800
   lines.

**`filter.rs` split (verbatim from the PRD's own Repository Structure and
Module sections, the one module whose internal shape the PRD pins exactly):**

```
ddb-server/src/filter/
├── mod.rs              # re-exports the prior filter.rs public surface
├── where_build.rs      # core where-clause building
├── junction.rs         # tag/junction/EXISTS handling
├── reverse.rs          # reverse-relation discovery
└── tests.rs            # the colocated test module
```

Exports (verbatim from the PRD's Module section): "the same items `filter.rs`
exported today (e.g. `pub fn try_build_where_sql` at `:625`, `pub(crate)
struct WhereCtx` at `:579`, ...), re-exported unchanged at their current
visibility." Re-verify these two line numbers at execution — this design was
drafted before any split landed and the file may have moved since the PRD's
2026-09-05 snapshot (re-measurement in § Architecture fit shows `filter.rs`
itself is unchanged at 3397 lines, so `:625`/`:579` are likely still
accurate, but confirm before relying on them). Note `ddb-server/src/relation_filter.rs`
(a separate, already-split-out file, not part of this PRD) already holds the
forward/reverse relation-filter EXISTS emitters — do not re-move that
code into the new `filter/junction.rs`/`filter/reverse.rs`; those new
siblings hold code that is CURRENTLY inside `filter.rs` itself (the
tag-filter/EXISTS-building and reverse-relation-discovery code that calls
into `relation_filter.rs`, not `relation_filter.rs`'s own contents).

**`helpers.rs` split (verbatim from the PRD's Repository Structure):**

```
ddb-core/src/sql_engine/helpers/
├── mod.rs           # re-exports the prior helpers.rs surface
├── regexes.rs        # the cached re_* helpers
├── inline_zone.rs     # strip_inline_zones family
└── tests.rs           # the colocated test module
```

**`queries.rs` decomposition (verbatim from the PRD's Module section):**
`build_query_fields(...)` keeps its exact signature (unchanged, per the PRD's
Exports line) and becomes a thin composer under 50 lines, calling new
private per-field-group helper functions extracted from its current body
(~842 lines at the PRD's snapshot; re-measure at execution). Field
registration order inside the composer must match the original body's
order exactly — the Risks section below explains why.

**`traits.rs` / `sql_conn` disposition (verbatim from the PRD's Feature
description):** inventory `SqlBackend::sql_conn`'s current callers (the
exception sits at `traits.rs:68-77` per the PRD's provenance note — reconfirm
the line range at execution). Remove the exception only if the post-split
caller inventory permits a scope-preserving change; otherwise retain it and
record, in both `traits.rs`'s doc comment and `docs/src/architecture/design-decisions.md`,
the exact remaining callers, the boundary debt, and that ownership of the
removal is deferred to the next evolution assessment — do not invent a
successor PRD or ticket number.

**The ten remaining round-2 sweep modules** (`dml.rs`, `ddl.rs`,
`materialize.rs`, `base_types.rs`, `graph.rs`, `parser/mod.rs`,
`builders.rs`, `crdt_resolver.rs`, `git_ops/mod.rs`, `search.rs`) follow the
same re-export contract (items 1-5 above); the PRD deliberately does not pin
their internal sibling-file boundaries ("exact submodule names decided per
split" — PRD Repository Structure note), so each task's implementor chooses
the split seam from the file's own natural groupings (e.g. by SQL statement
kind, by schema concern, by CRDT resolution strategy) and must still satisfy
items 2-5 above, which ARE pinned. The PRD's own task grouping (group A:
`dml`+`ddl`+`materialize`; group B: `base_types`+`graph`+`parser`; group C:
`builders`+`crdt_resolver`+`git_ops`+`search`) is reused as-is (see § Reuse
inventory) — this design does not regroup them.

## Data flow

Behavior-preserving by construction: no runtime data flow changes. The only
"flow" affected is Rust's own module resolution — `use` paths inside the
crate that refer to `crate::module::Item` (or the equivalent relative path)
resolve identically after the split because `mod.rs` re-exports every item
at its original path. External callers (other crates, FFI, GraphQL schema
construction, SQL execution) observe zero difference: same compiled symbols,
same generated GraphQL schema (verified by the existing schema-snapshot /
integration tests per the PRD's Test Strategy), same SQL text emitted by the
sql_engine modules.

## Reuse inventory

- **PRD 00156's pure-move split pattern** (`ddb-core/src/service/crud.rs` →
  `crud/`, `ddb-core/src/indexer/mod.rs` → its current multi-file shape
  confirmed live at `ddb-core/src/indexer/{mod.rs,materialize.rs,search.rs,
  filter.rs,cascade.rs,rebuild.rs,schema_version.rs,resolve.rs,graph.rs,
  ports.rs}`, `ffi.rs`, `mutations.rs`) — the exact `mod.rs`-re-export
  directory shape this design reuses verbatim for every module in scope here.
  Confirmed by reading the live `ddb-core/src/indexer/` directory listing.
- **The PRD's own Repository Structure tree** — reused as the pinned split
  boundary for `filter.rs` and `helpers.rs` (the two modules the PRD names
  exact sibling files for); reused as the pinned task grouping for the
  round-2 sweep's three groups (A/B/C).
- `nothing found, greps tried:` — searched for any existing automated
  "module splitter" tooling (`rg -i "split.*module|extract.*submodule" dev/bin ddb-core/src ddb-server/src`,
  `rg "mod\.rs.*re-export" -i docs/src`) — zero hits beyond PRD 00156's own
  prose description and this PRD's provenance note; confirmed every split so
  far (and this one) is done by hand, file by file, with `cargo build` +
  `cargo clippy` + the crate's test suite as the only oracle. No reusable
  script to point implementors at.

## Alternatives considered

1. **Smallest-diff: split only the three originally-flagged modules
   (`filter.rs`, `queries.rs`, `helpers.rs`) and leave the ten round-2
   modules for a future PRD.** Rejected: the PRD's own Problem Statement
   says these three were "deferred each time as out-of-scope-for-a-feature-
   PRD" across four prior review cycles — splitting only three again just
   repeats that deferral pattern for the other ten, which the 2026-07-16
   assessment already decided to stop doing by writing this PRD. The smaller
   scope is a real option a planner could still choose (e.g. if context
   budget forces a split), but it does not close what this PRD asks for.
2. **One task per module (13 tasks) instead of the PRD's three grouped
   sweep tasks (A/B/C) for the round-2 ten, plus the three named-module tasks.**
   Rejected: the PRD explicitly groups the round-2 sweep into three tasks
   "so the plan stays under the 15-task loop ceiling" (PRD Feature
   description) — ungrouping would recreate the exact problem (PRD 00077's
   28-task stall, cited in `plan-tasks`'s own rationale) this PRD's authors
   already designed around. This design reuses the PRD's grouping unchanged.
3. **Chosen: reuse PRD 00156's re-export pattern uniformly across all 13
   modules, split seams pinned where the PRD pins them (`filter.rs`,
   `helpers.rs`) and left to the implementor's judgment where it doesn't
   (the round-2 ten), one task per PRD-listed unit (3 named modules +
   3 grouped-sweep tasks + the `sql_conn` decision + verification + docs).**
   This is the smallest design that still satisfies every PRD success
   metric; it does not invent new structure beyond what pure-move splitting
   requires.

## Risks & edge cases

- **`base_types.rs` and `crdt_resolver.rs` grew well past their PRD-snapshot
  sizes** (1113→1849, 871→1508). A single extra sibling file may not be
  enough to bring every resulting file under 800 lines for these two —
  budget for 2-3 siblings, not the PRD's implied 1-2, and re-measure after
  each cut rather than assuming one split suffices.
- **`build_query_fields` field-registration order must be preserved exactly.**
  GraphQL field registration order can affect introspection output and
  schema-snapshot tests even when no field's shape changed — the PRD's Risks
  section already names this; this design adds no new mitigation beyond
  what the PRD states (preserve order, rely on schema-snapshot tests), since
  none is needed for a mechanical extraction.
- **Likely next changes after this PRD**: (1) `ddb-core/src/indexer/mod.rs`
  is now 900 lines and will likely need its own round-3 split — this design
  deliberately does not touch it, so a future PRD inherits a clean slate,
  not a half-split file. (2) The `sql_conn` exception, if retained rather
  than removed, is explicitly flagged in the PRD as an "audit gap for later
  evolution assessment" — this design preserves that framing rather than
  resolving it prematurely. (3) `queries.rs`'s decomposition may or may not
  need a directory split depending on its post-decomposition size — this
  design leaves that decision to be made with current numbers, not
  pre-committed either way.
- **Edge case — a module's "natural" split seam doesn't exist.** If one of
  the ten round-2 modules turns out to have no clean internal grouping (all
  its functions are mutually interdependent with no cohesive subset), the
  implementor should still produce a directory split along *any* boundary
  that keeps every resulting file under 800 lines and preserves re-exports —
  cohesion is a nice-to-have for readability, not a hard PRD requirement (the
  PRD's bar is line count + zero behavior change, not module elegance).

## Test strategy outline

- **Per split** (colocated + `cargo test-ci`, Tier 1): `cargo build` and
  `cargo clippy --workspace --all-targets` must stay clean; the crate's
  existing fast test suite (already colocated with the moved code, or
  exercising it indirectly) must stay green with no assertion changes beyond
  import paths. For `filter.rs`'s split specifically, the PRD names "the
  00159 where-filter tests" as the acceptance oracle — run those explicitly.
  For `helpers.rs`, run the `sql_engine` test suite explicitly (the PRD's
  stated acceptance).
- **Per-split behavior-preservation check** (PRD Phase 1 task): for each
  split, diff the moved function/struct/impl bodies against the pre-split
  file at the same commit range (the 00156-style verification) — this is a
  manual/`git diff`-based check per split, not a new automated test; the task
  description should say exactly which `git diff` invocation proves it (e.g.
  `git show <pre-split-sha>:path/to/module.rs` vs the new sibling file's
  content, diffed with whitespace-insensitive comparison for re-indentation
  only).
- **`build_query_fields` decomposition**: the existing schema-snapshot /
  GraphQL integration tests are the oracle (PRD's stated acceptance: "the
  generated GraphQL schema is byte-identical") — no new test authored, reuse
  what exists and confirm it still passes.
- **Full-suite gate** (PRD Phase 2, Tier 1 + the conditional Tier 2 run):
  `cargo test-ci` + `cargo clippy --workspace --all-targets` across the
  whole workspace once all splits land; per this repo's `CLAUDE.md`, this
  PRD deletes/replaces existing code paths (every split moves and deletes
  the old flat file), so `cargo test -p ddb-e2e` is also run per that
  conditional rule, not skipped.
- **`sql_conn` disposition**: no test — it is a documentation/decision task
  (PRD's own framing: "record... with a remaining exit gap," not "implement
  a fix"). Verification is a `docs/src/architecture/design-decisions.md`
  read-back confirming the disposition and caller list are both present and
  accurate against a fresh `rg "sql_conn"` sweep at execution time.

## Review log
