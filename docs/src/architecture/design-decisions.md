# Design Decisions

Architectural choices that constrain the system. New requirements and feature proposals should be checked against these decisions before creating PRDs or writing code. If a proposal conflicts with a decision here, the decision should be revisited and updated first, not silently overridden.

## Product Positioning: ddb Is the Platform, Not jink's Backend

**Decision**: ddb is a standalone backend platform intended for MANY downstream applications over time. jink is the first downstream, not the product; more will follow.

**Why**: The value proposition is a complete, flexible data backend that downstreams consume without reinventing storage, sync, query, or schema logic. Two consequences are deliberate:

- **The many transports (CLI, GraphQL, REST, PgWire, FFI, NoSQL HTTP) are intentional**, not over-engineering. Each future downstream may prefer a different access shape; the surface exists so a new consumer adopts one rather than the platform forcing GraphQL on everyone. That the current downstream (jink) uses only GraphQL is expected at this stage and is NOT evidence the other transports are waste.
- **120% parity / completeness is intentional.** Capabilities ship across the public interfaces so downstreams never hand-roll what the platform should own. "A downstream reimplements X" is treated as a platform gap to close, not a downstream responsibility.

**Do not re-litigate this in an audit, assessment, or refactor.** Reading "one consumer, one transport" as over-engineering is a misread of the strategy; assess and improve WITHIN this positioning, not against it. Revisit only if the multi-downstream thesis is formally abandoned with new evidence.

**Tradeoff**: Real surplus cost today (maintenance, drift risk, conformance burden across six transports) is carried against downstreams that do not yet exist. Accepted deliberately as a platform bet. Mitigation: the app-contract + conformance work keeps the parity cost sub-linear as capabilities grow.

**Reaffirmed 2026-07-13** (purpose-fit critique): no read-only specialization exceptions for any interface. An interface below the CRUD baseline is a parity gap to close (NoSQL HTTP: PRD 00191), never a candidate for a "specialized" carve-out. The remedy for parity cost is finishing the app contract (00171-00177), not shrinking the surface.

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

## Git Remotes as Sync Transport

**Decision**: All sync uses Git remotes (SSH, HTTPS, local paths, bare repos). No custom transport protocol, peer discovery, or LAN sync layer.

**Why**: Git already handles transport, authentication (SSH keys, credential helpers), NAT traversal (via hosted remotes), and incremental transfer (packfiles). The merge/CRDT/HLC conflict resolution layer is transport-agnostic by design - it operates on commits, not connections. Adding a second transport gains nothing that `git remote add` doesn't already provide.

**Tradeoff**: No zero-config device discovery on local networks. Users must configure a Git remote (hosted service, NAS, or local path). For air-gapped scenarios, bundle export/import fills the gap without requiring network infrastructure.

**Rejected alternative - Peer LAN sync (mDNS/Bonjour discovery)**: Evaluated and rejected. mDNS discovery, trust bootstrapping, and a custom exchange protocol add substantial complexity (platform-specific network APIs, firewall handling, iOS/Android background networking restrictions) for a marginal UX improvement over `git remote add ssh://...`. The bundle system already covers the offline transfer case. See also: spec FE-13 (deferred indefinitely).

## Rust

**Decision**: Core library in Rust with a CLI binary.

**Why**: Memory safety, cross-platform compilation, strong type system, and future FFI bindability (Python, Swift, Kotlin, JS, Go bindings planned post-MVP).

**Tradeoff**: Higher development overhead than scripting languages. Justified by the system's data integrity requirements — CRDT merge correctness and Git operations benefit from Rust's safety guarantees.

## Sparse Index Not Applicable

**Decision**: Drop Git sparse index from the scalability roadmap.

**Why**: DDB indexes all doogats and requires full-clone semantics on every device. Sparse index is coupled to sparse checkout, which conflicts with DDB's "all doogats locally available" contract. The original Phase 2 spec item was formally evaluated during Phase 2 exit and ruled out.

**Alternatives considered**: (1) Git sparse checkout for specific operating modes — rejected because it breaks the full-clone guarantee. (2) Application-level partial index — unnecessary since SQLite FTS5 already serves as the read cache and scales independently of Git's index format.

**What replaces it**: Commit-graph integration (done), incremental reindex (done), and future fsmonitor/file-watcher support address the same large-repo scalability concern through different mechanisms.

## Non-Goals

Approaches evaluated and explicitly rejected. If a future requirement conflicts with an item here, revisit the decision with new evidence before proceeding.

| Area | Non-Goal | Why | Alternative |
|------|----------|-----|-------------|
| Sync transport | Peer LAN discovery (mDNS/Bonjour) | Git remotes already provide transport + auth. Custom discovery adds platform-specific complexity with no functional gain. | `git remote add` (SSH, HTTPS, local path) |
| Sync transport | Custom sync protocol (libp2p, etc.) | Git packfiles are already efficient incremental transfer. A second protocol doubles the attack/failure surface. | Git fetch/push |
| Scalability | Git sparse checkout / sparse index | DDB requires full-clone semantics on every device. Sparse checkout breaks the "all doogats locally available" contract. | Commit-graph, incremental reindex, fsmonitor |
| Multi-user | Real-time collaborative editing | CRDT merge is designed for async multi-device sync, not live cursors. Real-time adds WebSocket/OT complexity for a single-user system. | Async sync with conflict resolution |

## Per-Parent Batch Loading (Not Page-Level DataLoader)

**Decision**: REFERENCES fields resolve via per-parent batch calls, not a page-level DataLoader.

**Why**: Each parent item calls `get_doogats_batch(ids)` with its own reference IDs. With 20 items on a page, that's ~20 batch calls rather than the 1 call a true DataLoader would make. However, SQLite is in-process with no network round-trips, so the difference is microseconds. A page-level DataLoader requires async-graphql shared request state and deferred resolution, adding substantial complexity for no measurable gain at personal scale.

**Tradeoff**: Fetching 50 items with relations issues ~50 queries instead of 3. Acceptable because SQLite in-process queries take <1ms each. Revisit only if profiling shows relation resolution as a bottleneck at >10K doogats.

## Broadcast Channel for Subscriptions

**Decision**: Use `tokio::sync::broadcast` (capacity 256) as the event bus for GraphQL subscriptions.

**Why**: Broadcast channels are lock-free, support multiple subscribers, and require zero allocation when no subscribers exist. The actor emits events after successful mutations; each WebSocket subscription creates a receiver that filters events by kind/type. This decouples the mutation path from subscription delivery.

**Tradeoff**: Slow clients that can't keep up will miss events (broadcast receiver lag). Acceptable for MVP — clients can refetch state on reconnect. A future improvement could add a replay buffer or persistent event log.

## Mobile Model: Full Replica via FFI Git Sync

**Decision** (2026-07-13): A mobile device participates in the distribution as a full git replica. `DoogatDriver` gains fetch/push/sync over HTTPS-token remotes (PRD 00190); bundle export/import remains the offline fallback. Hosted thin-client access is not the mobile model; it may layer on later behind the server-hardening work (00179) if a downstream needs it.

**Why**: The product statement is "distributed thanks to git, mobile included", and the existing decisions already commit to full-clone semantics on every device. A hosted thin client would trade away offline-first and reintroduce the cache-and-sync layer git exists to provide.

**Tradeoff**: On-device libgit2 credentials, host-scheduled sync (no background magic on iOS), and full-clone storage cost on phones. Accepted; the operating envelope is documented with the FFI sync work.

## PgWire Is a Guaranteed Write Surface (Non-Transactional)

**Decision** (2026-07-13): PgWire keeps DML/DDL. Its writes ride the app contract and the unified error policy (00172/00179) like every other transport. The interface documents its actual semantics: each statement commits independently; there is no rollback after commit and no isolation levels (Postgres transaction verbs are not honored — the docs state precisely how BEGIN/COMMIT/ROLLBACK behave).

**Rejected alternative**: gating PgWire to SELECT-only as a "specialized read interface". Rejected with the general no-specialization ruling above.

**Tradeoff**: PG-protocol clients that assume transactional semantics (ORMs, migration tools) can misbehave; carried deliberately, mitigated by documented semantics and unified, redacted errors.

## Read Freshness: Committed = Visible, Watch Mode Opt-In

**Decision** (2026-07-13): Served reads enforce "committed = visible" via a cheap HEAD-oid staleness probe (no more blanket `skip_stale_check`); a `_typedef` arriving via sync triggers a GraphQL schema reload; an opt-in watch mode (`ddb serve --watch` / `ddb watch`) absorbs external edits into commits after a debounce. A per-transport consistency contract page documents every guarantee. (PRD 00189.)

**Why**: The premise is that the markdown files are the database, so a change committed outside ddb must be served without waiting for an unrelated write. Uncommitted edits are not yet data — unless the operator opts into watch mode, which turns saves into commits rather than indexing uncommitted state.

**Rejected alternative**: indexing the uncommitted working tree directly. Rejected because it breaks the "index derived strictly from git truth" invariant that makes the index safely disposable.

## Known Limitations

| Area | Limitation | Plan |
|------|-----------|------|
| Plugin system | No type-specific behaviors via plugins | Type-driven behavior hooks |
| Subscriptions | Slow clients miss broadcast events | Replay buffer or persistent event log |
