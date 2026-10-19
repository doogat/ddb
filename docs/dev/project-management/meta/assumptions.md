# Assumptions ledger — 00183-split-oversized-modules-round-2-v1

## 1: Split filter.rs into filter/ submodules

Ivan:
- `SchemaLookup` kept as `pub type` (its actual original visibility) rather than the dispatch text's stated `pub(crate) type`, since the task instructs preserving the exact prior public surface and the prior surface was `pub`.
- `resolve_reference_target`, `resolve_reverse_target`, and `build_tags_filter_condition` widened from private to `pub(super)` (module-tree-internal only) — the minimum visibility needed for the split to compile; no wider than that.
- Dropped the would-be `pub(crate) use reverse::{reverse_relations, ReverseRelation};` re-export from `mod.rs` since nothing in the crate consumes it at that path (confirmed via unused-import warning + clean rebuild after removal); the items remain `pub(crate)` and reachable within the `filter` module tree.
