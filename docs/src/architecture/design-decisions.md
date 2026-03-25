# Design Decisions

## Hybrid Git-CRDT Merge

**Decision**: Use Git for >99% of merges; Automerge CRDT for the rest.

**Why**: Git's 3-way merge handles non-overlapping edits perfectly — the common case for a solo user across devices. CRDT handles the rare overlapping edits (character-level body conflicts, field-level metadata changes). This avoids the overhead of full CRDT serialization for every file while preserving human-readable Git history.

**Tradeoff**: Two merge paths create behavioral drift risk. Mitigation: spec defines FR-21a to validate clean merges and re-merge via CRDT if structural issues arise.

## Three-Zone Markdown

**Decision**: Split each doogat into frontmatter (YAML), body (Markdown prose), and reference section (structured fields).

**Why**: Each zone has different merge semantics. Frontmatter fields are independent key-value pairs — field-level CRDT merge is natural. Body text is prose — character-level text CRDT handles concurrent edits. Reference fields are structured data — set-union semantics with ours-wins conflict resolution.

**Tradeoff**: The heuristic reference detection (finding the last `---` where all subsequent non-empty lines match `- key:: value`) is fragile around thematic breaks in body text. The parser handles this by backtracking from the last `---` and validating content patterns.

## ID-Only Filenames

**Decision**: Doogat files are named `{id}.md` where ID is a 14-digit timestamp (`YYYYMMDDHHmmss`).

**Why**: Filenames never change when titles change, so wikilinks (`[[20260226120000]]`) remain stable. Avoids title-to-slug mapping complexity. Follows Doogat philosophy where IDs are the stable identifier.

**Tradeoff**: Filesystem browsing is opaque without the CLI or index. The `search` and `query` commands compensate.

## SQLite Index as Derived Cache

**Decision**: The SQLite index is always rebuildable from Git. It's a read-only cache, not a source of truth.

**Why**: No consistency hazard between Git and the index — Git always wins. Staleness detection is cheap (compare HEAD OID). The index can be safely deleted and rebuilt. Avoids dual-write coordination.

**Tradeoff**: Full rebuild reads and parses every doogat. Acceptable at MVP scale (<5K doogats) but will need incremental indexing for larger collections.

## Git Commits as Sync Checkpoints

**Decision**: Each node stores its `known_heads` (list of HEAD commits it has synced) in `.nodes/{uuid}.toml`, which is Git-tracked.

**Why**: Enables compaction to safely find the greatest common ancestor (GCA) across all nodes. No separate metadata store needed. Other nodes learn about sync progress by fetching the updated `.nodes/` directory.

**Tradeoff**: Stale nodes (offline beyond a threshold) block compaction from advancing past their last known head. This is an unresolved concern for post-MVP.

## No Server Required

**Decision**: All sync happens via Git remotes. No HTTP/REST/GraphQL server in the MVP.

**Why**: Git provides the transport layer (SSH, local paths, bare repos). Adding a server adds complexity, authentication concerns, and deployment burden. The CLI + library API is sufficient for validating the core model.

**Tradeoff**: No web UI or mobile app without building a server layer. This is a post-MVP concern — the architecture supports adding a server that wraps the core library.

## Rust

**Decision**: Core library in Rust with a CLI binary.

**Why**: Memory safety, cross-platform compilation, strong type system, and future FFI bindability (Python, Swift, Kotlin, JS, Go bindings planned post-MVP).

**Tradeoff**: Higher development overhead than scripting languages. Justified by the system's data integrity requirements — CRDT merge correctness and Git operations benefit from Rust's safety guarantees.

## Sparse Index Not Applicable

**Decision**: Drop Git sparse index from the scalability roadmap.

**Why**: DDB indexes all doogats and requires full-clone semantics on every device. Sparse index is coupled to sparse checkout, which conflicts with DDB's "all doogats locally available" contract. The original Phase 2 spec item was formally evaluated during Phase 2 exit and ruled out.

**Alternatives considered**: (1) Git sparse checkout for specific operating modes — rejected because it breaks the full-clone guarantee. (2) Application-level partial index — unnecessary since SQLite FTS5 already serves as the read cache and scales independently of Git's index format.

**What replaces it**: Commit-graph integration (done), incremental reindex (done), and future fsmonitor/file-watcher support address the same large-repo scalability concern through different mechanisms.

## Known Limitations (MVP)

## Broadcast Channel for Subscriptions

**Decision**: Use `tokio::sync::broadcast` (capacity 256) as the event bus for GraphQL subscriptions.

**Why**: Broadcast channels are lock-free, support multiple subscribers, and require zero allocation when no subscribers exist. The actor emits events after successful mutations; each WebSocket subscription creates a receiver that filters events by kind/type. This decouples the mutation path from subscription delivery.

**Tradeoff**: Slow clients that can't keep up will miss events (broadcast receiver lag). Acceptable for MVP — clients can refetch state on reconnect. A future improvement could add a replay buffer or persistent event log.

| Area | Limitation | Post-MVP Plan |
|------|-----------|---------------|
| Clock source | Git commit timestamps, not Lamport/HLC | Add hybrid logical clocks |
| Compaction | Removes all temp files, not history-aware | Implement history-based CRDT compaction |
| Air-gapped sync | Not implemented | Bundle-based sync protocol |
| Binary assets | No conflict resolution strategy | Define binary merge policy |
| Performance | Untested beyond small collections | Benchmark at 5K+ doogats |
| Plugin system | Not implemented | Type-specific behaviors via plugins |
| ID type | `i64` in CLI read/update, `String` in types | Align to `String` everywhere |
