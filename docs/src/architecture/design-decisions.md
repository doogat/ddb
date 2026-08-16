# Design Decisions

Architectural choices that constrain the system. New requirements and feature proposals should be checked against these decisions before creating PRDs or writing code. If a proposal conflicts with a decision here, the decision should be revisited and updated first, not silently overridden.

## Product Positioning: ddb Is the Platform, Not jink's Backend

**Decision**: ddb is a standalone backend platform intended for MANY downstream applications over time. jink is the first downstream, not the product; more will follow.

**Why**: The value proposition is a complete, flexible data backend that downstreams consume without reinventing storage, sync, query, or schema logic. Two consequences are deliberate:

- **The many transports (CLI, GraphQL, REST, PgWire, FFI, NoSQL HTTP) are intentional**, not over-engineering. Each future downstream may prefer a different access shape; the surface exists so a new consumer adopts one rather than the platform forcing GraphQL on everyone. That the current downstream (jink) uses only GraphQL is expected at this stage and is NOT evidence the other transports are waste.
- **120% parity / completeness is intentional.** Capabilities ship across the public interfaces so downstreams never hand-roll what the platform should own. "A downstream reimplements X" is treated as a platform gap to close, not a downstream responsibility.

**Do not re-litigate this in an audit, assessment, or refactor.** Reading "one consumer, one transport" as over-engineering is a misread of the strategy; assess and improve WITHIN this positioning, not against it. Revisit only if the multi-downstream thesis is formally abandoned with new evidence.

**Tradeoff**: Real surplus cost today (maintenance, drift risk, conformance burden across six transports) is carried against downstreams that do not yet exist. Accepted deliberately as a platform bet. Mitigation: the app-contract + conformance work keeps the parity cost sub-linear as capabilities grow.

**Reaffirmed 2026-07-13** (purpose-fit critique): no read-only specialization exceptions for any interface. An interface below the CRUD baseline is a parity gap to close (NoSQL HTTP: PRD 00192), never a candidate for a "specialized" carve-out. The remedy for parity cost is finishing the app contract (00172-00178), not shrinking the surface.

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

**Superseded in part** (2026-08-16): the ID-only filename rule stands; the 14-digit-only ID shape is superseded by the next entry.

## ID Format: UTC Timestamp Plus Per-Node Suffix

**Decision** (2026-08-16, PRD 00170): Doogat IDs keep the 14-digit `YYYYMMDDHHmmss` prefix but mint it from **UTC** instead of local time, followed by a 6-digit zero-padded sub-second field and an 8-character lowercase-hex per-node discriminator (sourced the way `HlcClock` sources its node id, from `.git/ddb-node`, truncated by `truncate_node`), never a bare local counter. Full shape: `<14 UTC digits><6 sub-second digits><8 hex node chars>` = **28 characters**, matching `^[0-9]{20}[0-9a-f]{8}$`, e.g. `20260816143012042317a1b2c3d4`. The sub-second field is a monotonic counter seeded from the microsecond component of the same clock read, not a literal clock reading: when a candidate would not be strictly greater than this process's previous mint, the field increments; only an overflow past `999999` waits for the next UTC second. This entry records the shape; the format change itself ships in a follow-up implementation PRD.

**Why**: The defect is identity correctness, not throughput. Under local-time, second-resolution minting, two devices can independently mint the *identical* ID with zero coordination (and a DST fold repeats a second on one device); the collision is only caught reactively at merge time by `resolve_add_add_collision` + `derive_content_id`. UTC plus a per-node discriminator moves that collision from routine to residual — but **not to zero, and this record must not claim otherwise**. The discriminator is `truncate_node` (`ddb-core/src/hlc.rs:123`-`:125`), the first 8 non-dash characters of the node UUID: **32 bits of node identity, not the whole UUID**. Two distinct node UUIDs whose first 8 non-dash characters coincide produce an *identical* node field — probability 2^-32 per node pair, and by the birthday bound roughly even odds that some such pair exists once a population reaches ~77,000 nodes. Two such nodes then collide on a full ID only if they also mint in the same UTC second *and* land on the same sub-second slot, so an actual duplicate ID is rarer still. The scheme is therefore **collision-resistant at mint, not collision-free**: `resolve_add_add_collision` + `derive_content_id` remain wired as the backstop for this residual case as well as for legacy IDs, and must not be retired as dead code once this ships.

**Why not a wider discriminator**: widening the node field, or hashing the whole UUID into it, would shrink the residual further, but it would leave ddb with two different renderings of node identity — the ID's and `HlcClock`'s — which is exactly the drift this decision avoids. Reusing `truncate_node` keeps one node-identity concept; the residual stays bounded, backstopped, and recorded rather than hidden. Revisit if a deployment ever approaches the ~10^4-node range, where the pairwise risk stops being remote (~1% at 10,000 nodes).

**Deciding factor — downstream ID parsing.** Both live options fix identity correctness equally well, so the choice turned on migration surface alone: the chosen shape keeps a digit-only 14-character prefix, so every stored `[[20260226120000]]` wikilink and any consumer that prefix-parses the ID keeps working unchanged. The one downstream that exists today was checked rather than assumed: jink treats the doogat id as an opaque GraphQL `ID` scalar (`frontend/src/lib/graphql/generated.ts:740` for the `doogat(id:)` lookup argument, `:1036` for a returned id) and nowhere slices it, length-checks it, or parses a date out of it. So the migration surface this decision protects is **not** a demonstrated jink break — it is ddb's own surfaces (`DoogatId::is_valid_shape`, and the schema wording at `ddb-server/src/schema/queries.rs:68` and `:70`, which is where jink's generated `14-digit` doc comment at `generated.ts:642` comes from), stored wikilink legibility, and the unknown future downstreams ddb is positioned to serve. All of the exact-14-digit assertions in play are ddb's own to update.

**Rejected — HLC-derived identity**: `Hlc::to_string()` renders `{wall_ms}-{counter:04}-{node}` (e.g. `1755000000000-0042-a1b2c3d4`), which breaks every digit-based parser with no salvageable digit-only prefix, costs the date legibility that wikilinks rely on, and does not zero-pad `wall_ms`, so raw lexical sort is not structurally guaranteed the way a fixed-width stamp is. Its collision story is no stronger in kind: `Hlc`'s `node` field is the same `truncate_node` output, so it carries the identical 32-bit residual described above. Identity correctness is a wash between the two, and migration surface decides.

**Rejected — keep second-resolution local time**: overruled by the maintainer pre-decision of 2026-07-13. Keeping it means permanently accepting cross-device collision and the DST fold as designed behavior, with reactive merge-time repair as the only defense. It lost on identity correctness; throughput was never the deciding factor.

**Throughput — two different numbers; do not conflate them.**

- *ID mints*: today the mint spin-waits for the wall second to advance, so exactly one candidate string exists per second and minting caps at **~1 mint/sec/process**. The chosen shape removes that wall-clock throttle entirely; the estimate (**unbenchmarked**) is **10^4-10^5 ID mints/sec/process**, bound by candidate computation plus an in-memory existence check, with no per-mint disk write. HLC-derived identity would land at an estimated **10^3-10^4 mints/sec/process** if it reused the persisted `HlcClock`, which writes and renames `.git/ddb-hlc` on every tick.
- *End-to-end doogat creates*: **unchanged at roughly 1 per second — this decision does not improve it.** Creating a doogat writes a git commit per doogat, and that commit, not the ID mint, is the binding cost. `docs/src/technical/performance.md:87` measures a single doogat create at **971.97 ms** (mobile FFI baseline, Darwin/arm64, debug build), and `:95` states plainly that "Create latency is dominated by git commit per doogat (~1s each)". Deleting the mint's sleep does not touch that commit.

So the honest answer to "can a downstream bulk-create faster than ~1 doogat/sec once this ships?" is **no**. What changes is which component is the ceiling: the mint stops throttling, and a 10-row SQL `INSERT` stops spanning 10 wall-clock seconds of ID space, but per-doogat commit cost still governs end-to-end creates. Lifting that needs a separate change to the write path (amortizing many doogats into one commit), which neither this decision nor its follow-up implementation PRD makes.

**Tradeoff**: IDs get longer, so `DoogatId::is_valid_shape`'s "exactly 14 ASCII digits" invariant must be relaxed and every exact-length validator updated. New IDs also read as UTC, not local wall time. One implementation constraint falls out of the chosen source: nothing constrains `.git/ddb-node` to hex or to any length (`ddb-core/src/git_ops/hlc_clock.rs:42`-`:46` accepts any non-empty value) and `truncate_node` returns a *shorter* string when given fewer than 8 non-dash characters, so the mint must normalize and validate the node field before it reaches an ID — otherwise it can emit a value its own `^[0-9]{20}[0-9a-f]{8}$` validator rejects. Evidence: `dev/local/notes/00170-id-format-blast-radius.md`.

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
| Data import | Foreign-system importers (Notion, Obsidian, Evernote converters) | Doogats are plain markdown and batch create verbs exist; format-specific conversion is downstream domain knowledge, not platform storage/sync/query logic (2026-07-13). | Downstream-owned converters over batch create verbs, or write `.md` files and reindex |

## Persisted CRDT State: Write-Only Scaffolding

**Finding** (verified PRD 00165): `ResolvedFile.fm_crdt_bytes` is written by `sync_manager::write_fm_crdt_files` to `.crdt/temp/{oid}_{id}_fm.crdt`, re-compacted by `compaction` (load-to-re-save only), and wiped wholesale by `GitRepo::open`. No production path loads it back to influence resolution — it is write-only, currently-unread scaffolding.

**Decision**: Retain the write path but document it as currently-unconsumed scaffolding. A future sync-resume / continuity feature would be its only justified consumer. Defer any machinery removal to the hygiene PRD 00199, which owns dead-abstraction pruning; a determinism PRD does not rip out compaction code.

**The real invariant** (not merely "no reader"): "persisted CRDT state is swap-asymmetric (the merge receiver differs across nodes) -> it must remain node-local and never be read back into resolution; adding any reader requires making the persisted bytes swap-symmetric first."

## Per-Parent Batch Loading (Not Page-Level DataLoader)

**Decision**: REFERENCES fields resolve via per-parent batch calls, not a page-level DataLoader.

**Why**: Each parent item calls `get_doogats_batch(ids)` with its own reference IDs. With 20 items on a page, that's ~20 batch calls rather than the 1 call a true DataLoader would make. However, SQLite is in-process with no network round-trips, so the difference is microseconds. A page-level DataLoader requires async-graphql shared request state and deferred resolution, adding substantial complexity for no measurable gain at personal scale.

**Tradeoff**: Fetching 50 items with relations issues ~50 queries instead of 3. Acceptable because SQLite in-process queries take <1ms each. Revisit only if profiling shows relation resolution as a bottleneck at >10K doogats.

## Broadcast Channel for Subscriptions

**Decision**: Use `tokio::sync::broadcast` (capacity 256) as the event bus for GraphQL subscriptions.

**Why**: Broadcast channels are lock-free, support multiple subscribers, and require zero allocation when no subscribers exist. The actor emits events after successful mutations; each WebSocket subscription creates a receiver that filters events by kind/type. This decouples the mutation path from subscription delivery.

**Tradeoff**: Slow clients that can't keep up will miss events (broadcast receiver lag). Acceptable for MVP — clients can refetch state on reconnect. A future improvement could add a replay buffer or persistent event log.

## Mobile Model: Full Replica via FFI Git Sync

**Decision** (2026-07-13): A mobile device participates in the distribution as a full git replica. `DoogatDriver` gains fetch/push/sync over HTTPS-token remotes (PRD 00191); bundle export/import remains the offline fallback. Hosted thin-client access is not the mobile model; it may layer on later behind the server-hardening work (00180) if a downstream needs it.

**Why**: The product statement is "distributed thanks to git, mobile included", and the existing decisions already commit to full-clone semantics on every device. A hosted thin client would trade away offline-first and reintroduce the cache-and-sync layer git exists to provide.

**Tradeoff**: On-device libgit2 credentials, host-scheduled sync (no background magic on iOS), and full-clone storage cost on phones. Accepted; the operating envelope is documented with the FFI sync work.

## PgWire Is a Guaranteed Write Surface (Non-Transactional)

**Decision** (2026-07-13): PgWire keeps DML/DDL. Its writes ride the app contract and the unified error policy (00173/00180) like every other transport. The interface documents its actual semantics: each statement commits independently; there is no rollback after commit and no isolation levels (Postgres transaction verbs are not honored — the docs state precisely how BEGIN/COMMIT/ROLLBACK behave).

**Rejected alternative**: gating PgWire to SELECT-only as a "specialized read interface". Rejected with the general no-specialization ruling above.

**Tradeoff**: PG-protocol clients that assume transactional semantics (ORMs, migration tools) can misbehave; carried deliberately, mitigated by documented semantics and unified, redacted errors.

## Read Freshness: Committed = Visible, Watch Mode Opt-In

**Decision** (2026-07-13): Served reads enforce "committed = visible" via a cheap HEAD-oid staleness probe (no more blanket `skip_stale_check`); a `_typedef` arriving via sync triggers a GraphQL schema reload; an opt-in watch mode (`ddb serve --watch` / `ddb watch`) absorbs external edits into commits after a debounce. A per-transport consistency contract page documents every guarantee. (PRD 00190.)

**Why**: The premise is that the markdown files are the database, so a change committed outside ddb must be served without waiting for an unrelated write. Uncommitted edits are not yet data — unless the operator opts into watch mode, which turns saves into commits rather than indexing uncommitted state.

**Rejected alternative**: indexing the uncommitted working tree directly. Rejected because it breaks the "index derived strictly from git truth" invariant that makes the index safely disposable.

## V1.0 Finish Line

**Decision** (2026-07-13): v1.0 — "the goal is reached" — is a checkable state, not a drained backlog. Four conditions, each observable:

1. **Safe + coherent**: all nine invariants in `technical/invariants.md` (I1-I9, covering both the data-safety and coherence sets) read HOLDS, each pinned by a regression test.
2. **Parity proven**: the same golden CRUD + validation workflows pass the conformance harness on all six transports (CLI, GraphQL, REST, PgWire, FFI, NoSQL HTTP), with parity failures blocking CI (PRDs 00175/00176/00192/00193).
3. **Consumable proven**: jink runs on `ddb-client` + typedef codegen + structured inputs, and the glue-deletion ledger (~2,400 backend + ~1,250 frontend LOC) is measured, not estimated. The migration work happens in jink's repo; the measurement is a v1.0 acceptance item here.
4. **Distributed proven**: a mobile device operates as a full git replica via FFI sync (00191), and CI generates the bindings and builds the platform artifacts nightly (00194).

**Explicitly NOT required for v1.0** (post-v1 or decision-gated): registry publishing (crates.io / SwiftPM / Maven), a second downstream app, in-process TLS, the PgWire extended/prepared protocol, incremental indexing beyond ~5K doogats, a user-visible conflict journal, foreign-system import tooling.

**Why**: without a finish line, "done" drifts with the backlog. These conditions restate the roadmap end state (Safe / Coherent / Consumable) plus the distribution pillar as observations, so reaching the goal is a check, not a feeling. Source: `dev/local/audit-results/2026-07-13-goal-gap-analysis.md` (G4).

**Tradeoff**: a fixed line can understate new discoveries. Rule: a newly found data-safety gap (P0) joins the v1.0 gate; anything else lands post-v1 unless a maintainer decision promotes it.

## Known Limitations

| Area | Limitation | Plan |
|------|-----------|------|
| Plugin system | No type-specific behaviors via plugins | Type-driven behavior hooks |
| Subscriptions | Slow clients miss broadcast events | Replay buffer or persistent event log |
| Conflict visibility | CRDT auto-resolution reports counts only; no first-class conflict journal | Deferred post-v1 (2026-07-13, see V1.0 Finish Line). Interim: every resolution is a Git merge commit preserving both parents — `git show <merge>` reconstructs it (documented in the sync guide). Revisit when a downstream needs a journal surface |
