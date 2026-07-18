# CRDT Conflict Resolution

**Source**: `ddb-core/src/crdt_resolver.rs` (487 lines)

When Git detects a merge conflict, this module resolves it using Automerge CRDT with per-zone strategies.

## Entry Point

```rust
pub fn resolve_conflicts(
    conflicts: Vec<ConflictFile>,
    crdt_strategy: Option<&str>,
) -> Result<Vec<ResolvedFile>>
```

If `crdt_strategy` is set to a non-default value (e.g. `preset:append-log`), a warning is logged since only the default strategy is implemented. The default strategy is always used.

For each `ConflictFile`:
1. Parse ancestor, ours, and theirs into three zones
2. Merge each zone with its specific strategy
3. Reassemble via `parser::serialize()`
4. Validate the result parses correctly (round-trip check)

If `resolve_conflicts` fails, `SyncManager` falls back to ours-wins (keeping local content) and logs a warning.

## Frontmatter Merge

`merge_frontmatter(ancestor, ours, theirs) -> Result<String>`

**Strategy**: Field-level CRDT map merge using Automerge.

### Algorithm

1. Convert each YAML string to `BTreeMap<String, FmValue>` where `FmValue` is either `Scalar(String)` or `List(Vec<String>)`
   - YAML sequences (e.g. tags) are preserved as `FmValue::List` for element-level merge
2. Create an Automerge document with the ancestor map — scalars as Automerge values, lists as Automerge Lists
3. Fork for "ours": apply the diff (additions, changes, deletions) from ancestor → ours
   - List diffs use set semantics: added items are appended, removed items are deleted
4. Fork for "theirs": apply the diff from ancestor → theirs
5. Merge the two forks — Automerge resolves same-key conflicts deterministically; list items from both sides are preserved
6. Extract the merged map (deduplicating list items) and convert back to YAML

**Scalar conflict**: If both sides change the same scalar field to different values, Automerge picks one deterministically (based on actor IDs).

**List merge**: Concurrent additions to the same list (e.g. tags) are both preserved. Removals are honored. This prevents data loss on concurrent tag edits.

## Body Merge

`merge_body(ancestor, ours, theirs) -> Result<String>`

**Strategy**: Character-level CRDT text merge using Automerge.

### Algorithm

1. Create an Automerge document with a text object containing the ancestor body
2. Fork for "ours": compute character-level diff (ancestor → ours) via `similar::TextDiff::from_chars()`, apply as `splice_text()` operations
3. Fork for "theirs": same process
4. Merge the two forks — Automerge resolves overlapping character edits

### Op Consolidation

Character-level diffs produce many small operations (one per character). The implementation consolidates consecutive same-type operations:

- Adjacent deletes at consecutive positions → single delete with combined count
- Adjacent inserts at the same position → single insert with concatenated text
- Operations are applied in reverse order to preserve position validity

This handles intra-line edits (e.g., one side inserts "brave " into "hello world", the other appends "!") as well as multi-line edits.

## Reference Merge

`merge_reference(ancestor, ours, theirs) -> Result<String>`

**Strategy**: Automerge List CRDT (fork-diff-merge).

### Algorithm

1. Parse ancestor, ours, and theirs into reference line vectors via `ref_lines()`
2. Create Automerge document with `ObjType::List` at root key `"refs"`, populated with ancestor lines
3. Fork for ours — apply list diff: delete removed entries (backwards for index stability), append new entries
4. Fork for theirs — same
5. Merge forks — Automerge's list CRDT preserves concurrent additions from both sides
6. Extract merged list, sort alphabetically, deduplicate, join as output

### Conflict semantics

| Ours | Theirs | Result |
|------|--------|--------|
| Removed | Removed | Removed |
| Removed | Present | Removed (delete wins) |
| Present | Removed | Removed (delete wins) |
| Same line | Same line | Keep one (dedup) |
| Changed value | Unchanged | Both entries survive |
| Unchanged | Changed value | Both entries survive |
| Both changed differently | Both entries present |
| Concurrent new entries | Both present (list union) |

Unlike the Map approach for frontmatter, List CRDT preserves both sides of a same-key conflict (e.g. both `- source:: A` and `- source:: B` survive). Output is sorted alphabetically and deduplicated.

## Deterministic Actor Assignment

**PRD 00165**: `merge_frontmatter`, `merge_body`, and `merge_reference` are now **pure functions** of `(ancestor, ours, theirs)`.

Each Automerge doc-build site sets a stable, content-derived actor ID — `derive_actor(zone_content)` feeding `set_actor` at every base and fork site (9 sites total: base + ours-fork + theirs-fork across the three merge functions) — so Automerge's `(Lamport counter, actor id)` tie-break selects the **same winner on every node** regardless of which side that node calls "ours" (swap-symmetric).

### How it works

- `derive_actor(content)` hashes the zone string (SHA-256, first 16 bytes) into a valid `automerge::ActorId`.
- The actor is assigned immediately after each doc is constructed or forked, **before any op is applied**.
- The argument to `derive_actor` is always the zone content string itself (never a role label or node UUID), so the same physical content maps to the same actor on every node.
- For a conflicting same-key scalar, Automerge orders the competing ops by `(Lamport counter, actor id)`. Both fields are now pure functions of the content, so the winning op is identical regardless of the local ours/theirs assignment.

### Per-function sites

| Function | Base doc | Ours fork | Theirs fork | Total |
|---|---|---|---|---|
| `merge_frontmatter` | `derive_actor(ancestor)` | `derive_actor(ours)` | `derive_actor(theirs)` | 3 |
| `merge_body` | `derive_actor(ancestor)` | `derive_actor(ours)` | `derive_actor(theirs)` | 3 |
| `merge_reference` | `derive_actor(ancestor)` | `derive_actor(ours)` | `derive_actor(theirs)` | 3 |

### List-typed frontmatter fields

List-typed frontmatter fields (e.g. `tags`) do **not** get their determinism from actors — they merge through `merge_list_fields`, whose determinism comes from ancestor-order survivors followed by lexicographically-sorted additions.

### Determinism vs. HLC winner-ordering

Determinism (this change, PRD 00165) is **distinct** from HLC winner-ordering (PRD 00166, a separate change). This PRD guarantees the same tie-break winner every time; it does not decide which side should win.

### Automerge time assumption

Byte-identical CRDT state relies on automerge stamping changes with advisory `time: 0` by default (verified in automerge 0.7.4); a future automerge upgrade must re-verify this assumption.

## Preset: Last-Writer-Wins

`resolve_lww(conflicts) -> Result<Vec<ResolvedFile>>`

Whole-file resolution based on HLC comparison:

1. Compare `ours_hlc` vs `theirs_hlc`
2. Higher HLC wins (wall_ms → counter → node string)
3. If no HLC available, falls back to ours

Used as strategy `preset:last-writer-wins` in typedef `crdt_strategy`, and as the final fallback in the three-step merge cascade.

## Preset: Append-Log

`resolve_append_log(conflicts) -> Result<Vec<ResolvedFile>>`

Section-aware body merge for log-style doogats (e.g. project type):

1. **Frontmatter**: same as default (Automerge Map)
2. **Reference**: same as default (set merge)
3. **Body**: split by `## ` headings into sections
   - **Log sections** (entries matching `- [x] YYYY-MM-DD`): parse entries, dedup by (date, first line), union from both sides, sort chronologically
   - **Non-log sections**: default Automerge Text merge

## Compaction Strategy

CRDT temp files in `.crdt/temp/` use two naming conventions:
- Body: `{commit_oid}_{doogat_id}.crdt`
- Frontmatter: `{commit_oid}_{doogat_id}_fm.crdt`

Legacy files using bare OIDs are also supported. `parse_crdt_temp_name()` returns `(oid, doogat_id, is_frontmatter)`.

Compaction modes:

- **Cleanup**: removes temp files whose commit is an ancestor of the shared head (all nodes have synced past it). Handles both `.crdt` and `_fm.crdt` files.
- **Doc compaction**: groups remaining temp files by `(doogat_id, is_frontmatter)`, loads all Automerge changes, calls `AutoCommit::save()` to produce separate compacted blobs for body and frontmatter per doogat
- **Per-doogat**: `compact_doogat(repo, doogat_id)` targets a single doogat's CRDT history
- **Threshold check**: compaction skips when `.crdt/temp/` size is below `CompactionConfig.threshold_mb` (default 1MB) unless `--force` is passed

CLI flags: `ddb compact --force` (ignore threshold), `ddb compact --dry-run` (report what would happen).

When CRDT temp files are missing (compacted away), the resolver still works — it builds fresh Automerge docs from the three-way Git diff. See [Conflict Resolution Cascade](sync.md#conflict-resolution-cascade) for the full fallback decision tree.

## Round-Trip Validation

After merging all zones, the result is serialized via `parser::serialize()` and the module verifies the output parses correctly as a `ParsedDoogat`. If parsing fails, a validation error is returned — this prevents corrupted merges from being committed.

## Test Coverage

25 tests:
- Frontmatter: different fields, same field conflict, field addition, field removal, YAML special chars
- Frontmatter tags: one-sided add, concurrent additions merge, tag removal honored
- Body: non-overlapping edits, append from both sides, intra-line character-level merge
- Reference: union additions, removals, same-key conflicts, concurrent additions both present
- Full pipeline: three-zone conflict resolution with re-parse validation
- Full pipeline: no ancestor (new file on both sides)
- LWW: later HLC wins, deterministic tiebreak, no-HLC fallback
- Append-log: concurrent entries survive, dedup, non-log sections use text CRDT, empty log
- Strategy: non-default crdt_strategy still resolves (with warning)
- Error: unparseable content correctly returns error (LWW fallback path)

## Deletion Handling

`resolve_conflicts` only ever sees a `ConflictFile` for a path Git itself reports as conflicting — i.e. both sides touched it in an overlapping way. As of PRD 00200, the conflicted-merge tree is rebuilt as a true three-way merge: `commit_merge` re-runs `merge_commits(our_commit, their_commit)` in memory and overlays only the CRDT-resolved blobs onto the conflicting paths; every non-conflicting path is already correct at stage 0 and is left untouched.

This means two deletion scenarios resolve differently, and they should not be conflated:

- **Plain deletion of an untouched doogat** (one side deletes, the other side never touches that path): not a conflict. It never reaches `resolve_conflicts`. The three-way `merge_commits` result already has the path absent at stage 0, so the deletion survives in the merged tree — Git preserves it without any zone-merge logic running.
- **Delete-vs-edit** (one side deletes, the other side edits the *same* doogat): a real conflict. `sync_manager`'s delete-vs-edit bucket resolves it with the deliberate **resurrect-with-marker** policy — the edit wins, the doogat is kept, and `resurrected: true` is added to its frontmatter (see [Four-Bucket Conflict Partition](sync.md#four-bucket-conflict-partition)).

### Per-path outcome (three-way merge)

| Path shape | Final tree |
|---|---|
| both edit same region | CRDT/LWW resolved |
| ours edits only | ours' edit kept |
| ours clean-deletes | absent |
| theirs deletes, ours untouched | absent (preserved deletion, not a conflict) |
| theirs-only add/edit | theirs' change |
| ours creates | present |
| both edit, auto-mergeable (different regions) | both edits (git line-merges) |
| theirs renames | renamed path only (no duplicate) |
| delete-vs-edit (theirs deletes, ours edits) | resurrected + `resurrected: true` marker |

## SINGLETON post-sync sweep

SINGLETON conflict handling finishes **after** git/CRDT merge, not inside
`crdt_resolver.rs`. The implementation lives in
`ddb-core/src/consistency/singleton_sweep.rs` and runs from
`SyncManager::finalize_sync()` after `update_sync_state()` and before `push()`.

The sweep enumerates doogats, groups rows by `meta.doogat_type`, loads the
typedef schema, and only acts on typedefs marked `singleton: true`.

When more than one row exists for a singleton typedef, the winner is the row
with the highest commit-trailer HLC. If HLCs tie or are missing, the lexically
larger 14-digit `DoogatId` wins.

Each losing row keeps its original body and reference content, moves to
`ddb/_conflicts/{id}.md`, and gains these frontmatter overlays:

- `singleton_conflict_loser`
- `singleton_conflict_table`
- `singleton_conflict_resolved_at`

Materialization plus the consistency sweeps skip `ddb/_conflicts/`, so the
quarantined row stays recoverable on disk without re-entering the typed table.
