# Assumptions ledger — 00183-split-oversized-modules-round-2-v1

## 1: Split filter.rs into filter/ submodules

Ivan:
- `SchemaLookup` kept as `pub type` (its actual original visibility) rather than the dispatch text's stated `pub(crate) type`, since the task instructs preserving the exact prior public surface and the prior surface was `pub`.
- `resolve_reference_target`, `resolve_reverse_target`, and `build_tags_filter_condition` widened from private to `pub(super)` (module-tree-internal only) — the minimum visibility needed for the split to compile; no wider than that.
- Dropped the would-be `pub(crate) use reverse::{reverse_relations, ReverseRelation};` re-export from `mod.rs` since nothing in the crate consumes it at that path (confirmed via unused-import warning + clean rebuild after removal); the items remain `pub(crate)` and reachable within the `filter` module tree.

## 5: Sweep group B: split schema/base_types.rs, indexer/graph.rs, parser/mod.rs

Ivan (base_types.rs):
- The old flat file's `extract_body_section` re-export belongs in `to_gql.rs`, since it sits in that file's line range and `to_gql` calls it.
- Unused re-exports may be dropped from `mod.rs` (8 names), because the only alternative is a lint-suppression attribute, which this repo's hooks block. `schema/mod.rs` declares `mod base_types;` privately and nothing imports those names, so no reachable path was lost.
- `tests.rs` needs explicit imports, with a 4-space dedent allowed, since `use super::*` can no longer supply them from the new `mod.rs`.

Ivan (graph.rs):
- The new files import `Index` as `crate::indexer::Index` instead of through `super`, since `super::Index` no longer reaches it from inside `graph/`.
- `StalenessRow` moved to `health.rs` because only the health methods use it.

Ivan (parser/mod.rs):
- Included the doc-comment lines immediately before the briefing's stated ranges (lines 9 and 641), since the briefing's line numbers were off by one.
- Gated the test-only imports (`strip_wikilink`, `MAX_FRONTMATTER_BYTES`, `DoogatMeta`, `Value`) with `#[cfg(test)]` in `mod.rs`, because `tests.rs` is not this task's to edit and relies on `use super::*`.
