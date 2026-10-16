# Decision Audit Log: 00169-poison-file-reindex-resilience-v1

PRD: `00169-poison-file-reindex-resilience-v1.md`
Started: 2026-08-16T20:46:19Z
Completed: 2026-08-16T20:46:19Z
Autonomous: 7  |  Deferred: 2  |  Doubts: 0

### [autonomous] 2026-08-16T20:46:19Z

### [autonomous] 2026-08-16T20:46:19Z

### [autonomous] 2026-08-16T20:46:19Z

### [autonomous] 2026-08-16T20:46:19Z

**Decision**: DoogatService::reindex calls self.index.rebuild where self.index is a generic I: IndexPort, so rebuild is a TRAIT method (declared traits.rs:169, impl indexer/ports.rs:138). Routing reindex to the new locked_rebuild needs a new IndexPort method plus its impl, and neither traits.rs nor indexer/ports.rs was in task 7 file allowlist. Widen the allowlist, or drop the reindex leg?

### [autonomous] 2026-08-16T20:46:19Z

**Decision**: service/search.rs::reindex calls rebuild through a generic I: IndexPort, so a pub(crate) inherent Index::locked_rebuild is unreachable. How should reindex reach the locked path?

### [autonomous] 2026-08-16T20:46:19Z

**Decision**: Task 7 routed DoogatService::reindex through Index::locked_rebuild, whose post-lock re-check (check_integrity AND NOT is_stale) short-circuits and returns RebuildReport::default(). Does the explicit ddb reindex command still rebuild?

### [autonomous] 2026-08-16T20:46:19Z

### [deferred] 2026-08-16T20:46:19Z

### [deferred] 2026-08-16T20:46:19Z
