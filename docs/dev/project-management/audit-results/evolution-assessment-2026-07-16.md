# Evolution assessment — 2026-07-16 (delta pass)

**Mode**: delta + verification. The full five-audit battery ran 2026-07-05..13 and produced the tracked 31-PRD backlog (`roadmap-2026-07.md`). Code is unchanged since (HEAD 430cb13 at audit start; only docs commits after 2026-07-13). This pass ran 8 parallel lens auditors instructed to find what that battery **missed or got wrong**; every roadmap-driving claim below was re-verified against source by the orchestrator. UNVERIFIED items are marked.

**Numbering note**: all PRD numbers in this report are POST-renumber. By maintainer decision (see Decisions below), a new PRD 00171 (harness migration) was inserted before the contract block on 2026-07-16 and former 00171-00198 shifted +1 to 00172-00199. Documents dated before 2026-07-16 cite pre-shift numbers (offset +1 for ≥00171).

**Session actions already taken**: CI red fixed (commit `8cfe90a`: crossbeam-epoch 0.9.20 for RUSTSEC-2026-0204 + 3 clippy test lints; Tier 1 green: clippy `--workspace --all-targets` clean, test-ci 1633/0/0). **Push blocked** by the locked 1Password/SSH agent — user must unlock and `git push origin master`.

## Goal and coverage (lens 9)

**Goal** (v1.0 finish line, `design-decisions.md` 2026-07-13, four observable conditions):

| # | Condition | Coverage verdict |
|---|---|---|
| 1 | Safe + coherent: I1-I9 all HOLDS with regression tests | **Planned** (00163-00179), but two invariant scopes were wrong: I1 misses the merge/sync write path (F1); I9/00179 misses the sync-load boundary + `dml.rs`/`materialize.rs` sites (F3). Scope amendments applied to 00163/00179. |
| 2 | Parity proven: golden workflows on all 6 transports, CI-blocking | **Planned** (00175/00176/00192/00193), plus one structural gap: nothing forces parity for *future* verbs (F7) — verb-disposition registry added to 00193. |
| 3 | Consumable proven: jink on ddb-client + codegen + structured inputs, ledger measured | **Planned** (00185-00189) **but gated on releases, and release cadence is tracked nowhere** (F2). jink's adoption PRD 00026 probes the *deployed binary*; its three gates (00158/00159/00160) are done and unreleased since 2026-06-19. |
| 4 | Distributed proven: mobile replica via FFI sync + bindings CI | **Planned** (00191/00194, toolchain precondition documented). |

**Verdict**: the plan, fully executed, reaches the goal — provided (a) the scope corrections in this report land (applied today), and (b) releases actually ship between phases (new, was untracked). No scope drift found: every backlog PRD serves a goal condition.

## Ranked findings

### Critical

**F1 · ops-safety/architecture (lenses 1+2 converged) · Merge/sync git writes bypass the 00162 write lock and stage from the cached index — NEW; invariants doc claims otherwise.**
Evidence (all re-verified): `git_ops/merge.rs:32` (`build_merge_tree_and_commit` stages via cached `self.repo.index()`), `:47-54` (merge commit to HEAD), `:169-178` (`perform_normal_merge` commit + forced checkout), `:199-211` (fast-forward `set_target` + forced checkout) — none inside `with_write_lock`; all 11 lock sites live in `git_ops/mod.rs` only. Callers: `sync_manager/mod.rs:246,355`, bundle import. `invariants.md:14` (I1) claims "all eight write functions wrapped… still open: delete freshness" — wrong by omission.
Failure: jink's `ddb serve` sync racing a CLI write — merge commits from a stale parent, forced checkout reverts a concurrent writer's worktree file, `git maintenance --auto` prunes the orphaned commit. The exact silent-loss class 00162 shipped to kill, still open on the primary deployment shape.
Fix: **applied as scope amendment to 00163** (which is next-up and already edits `merge.rs`): wrap the three merge write sections in the re-entrant lock, stage from fresh state, extend its tests. `invariants.md` I1 corrected today to name the gap honestly.

### High

**F2 · downstream/user (lenses 5+8 converged) · jink's next adoption wave is blocked on a ddb release, not on backlog PRDs; release cadence is untracked — NEW.**
Evidence: last tag v0.2.6 (2026-06-19), 106 commits unreleased incl. the 00162 data-loss fix and jink 00026's three feature gates (CHANGELOG `[Unreleased]`); installed binaries self-update **from releases only** (`getting-started.md:127`); jink 00026 refuses to start until the deployed binary probes true. The roadmap's "jink 00026 delivered by 00182+00176" line (roadmap-2026-07.md:109) is a wrong dependency arrow — those PRDs don't gate it, the release does.
Fix: cut v0.2.7 now (user decision — `dev/bin/release`); add "release after each roadmap phase" to the roadmap as standing policy; corrected the dependency note in the roadmap addendum.

**F3 · security (lens 3) · Synced typedef identifiers are never charset-validated; 00179's site list misses the sync-load boundary, `sql_engine/dml.rs`, parts of `materialize.rs`, and `relation_filter.rs` — NEW (extends TRACKED 00179).**
Evidence (verified): `sql_engine/builders.rs:685-703` (`schema_from_parsed`) builds `table_name` from synced `_typedef` title with no `is_safe_sql_identifier` call; 00179's write-boundary guard covers only the create verb. Unescaped `format!("\"{}\"")` sinks outside 00179's list: `dml.rs` INSERT/UPDATE/DELETE/cascade/unique sites, `materialize.rs` row/junction sites, `relation_filter.rs:74-226`.
Failure: a hostile/odd synced typedef name wedges reindex (bounded DoS / index corruption; no exfiltration — single-statement execute).
Fix: **applied as scope amendment to 00179**: add the charset guard at `schema_from_parsed` (single convergence point for every synced typedef) + broaden the escaping sweep to the named files + consolidate the duplicated validator pair (`is_safe_sql_identifier` / `validate_identifier`).

**F4 · history (lens 7) · SQL-DML typed writes are a parallel pipeline outside the unified write path — recurring bug-class, untracked — NEW.**
Evidence: fix-clustering shows the same write-semantic bug shipping N times (junction sync fixed separately on SQL INSERT `be086e2`, SQL UPDATE `f231e97`, service unset `da9846e`, FFI raw `f26beae`/`143b58e`); 00175-00178 route transports through the contract but no PRD routes `sql_engine` DML through the same core write plan.
Fix: **new PRD 00196** (route SQL DML through the unified write pipeline; after the contract block).

**F5 · history (lens 7) · The .sh/.ps1 double-authored test harness taxes every behavior change — NEW, needs a maintainer decision (AGENTS.md mandates the 4-file pattern).**
Evidence: smoke/integration .sh↔.ps1 co-change 75/69 commits; 21 pure PowerShell-dialect fixes (7 in one day, 2026-03-18); one behavior fix routinely touches 4 files (`5a55296`).
Fix direction: migrate scenarios into the cross-platform Rust e2e crate (assert_cmd), thin per-OS launch shims. **Decided 2026-07-16: migrate soon, inserted early — new PRD 00171 (before the contract block; builds its own spawn helper, must not depend on the higher-numbered 00188 ephemeral-serve).**

**F6 · interfaces (lens 4) · Schema-reload triggering is hand-wired transport policy in 5 sites; the class already shipped the same bug twice in one PRD — NEW.**
Evidence: `operations.rs:265,301,577`, `rest.rs:463` ("Mirror the GraphQL guard"), `pgwire.rs:116`; 00161 shipped the missing-reload bug on GraphQL (caught in blind review `b34dd30`) and again on REST (caught in doubt review `5a4f9e1`).
Fix: **applied as scope rider on 00172**: derive "schema changed" from the core result and fire the reload once in the actor bridge.

**F7 · interfaces (lens 4) · No forcing function makes a new verb declare its 6-transport disposition — parity is bought with review cycles per verb — NEW.**
Evidence: the 00161 probe — adapters cost only ~90 LOC each, but parity relied on 5 review rounds; the disposition table in `schema-apply.md` was written under review pressure, not enforced. 00193 covers existing workflows only.
Fix: **applied as scope amendment to 00193**: verb-disposition registry (AppCommand variant → 6 dispositions) asserted by a test that fails on an unregistered new variant.

### Medium

**F8 · ops (lens 2) · Batch read-modify-write ops (fix_all, migrate_all, zone_migrate, rename backlinks) read unlocked, then commit full replacements — lost-update window — NEW.** Evidence: `consistency/mod.rs:559`, `migrations.rs:109`, `zone_migrate.rs:96`, `git_ops/rename.rs:80`; the lock serializes tree construction only. Fix: **new PRD 00195** (per-file OID re-check at commit, drop changed files).
**F9 · interfaces (lens 4) · `SqlResult` has 4 hand-rolled renderers with field + semantic drift** (FFI fabricates `affected_rows` for SELECT where GraphQL returns null; `column_types` PgWire-only; CLI drops headers). Fix: **new PRD 00197** (derive transport renders from the core type — includes the serde→GqlValue bridge that also kills GraphQL's twin field-listing).
**F10 · ops (lens 2) · `.crdt/temp` lifecycle contradiction**: `GitRepo::open` wipes it (`mod.rs:174,577-595`) while sync persists CRDT continuity state there (`sync_manager/mod.rs:77...`) and compaction maintains it; no production reader; `compacted_*` names invisible to cleanup. Dead-or-broken machinery on the CRDT convergence path. Fix: **scope amendment to 00165** (decide lifecycle while making resolution deterministic).
**F11 · simplification (lens 6) · `map_to_yaml` in `crdt_resolver.rs:312-342` writes merge-resolution frontmatter with weaker quoting/escaping than the parser's `yaml_quote`** — a self-inflicted poison-file feeder (00169 hardens the reader, not this writer). Fix: **scope amendment to 00165** (reuse parser quoting helpers).
**F12 · ops (lens 2) · `git maintenance`/commit-graph subprocesses run inside the write lock** (`mod.rs:341,377` + `write_commit_graph` per commit) — a slow gc exhausts the 10 s lock timeout for every other writer. Fix: **scope amendment to 00174** (run after guard drop).
**F13 · architecture (lens 1) · `GitRepo::open` auto-commits a `.ddb-version` migration into ANY repo it is pointed at** (`mod.rs:181-226`, no ddb-repo evidence check); the stray `.ddb-version` is committed at ddb's own source root today (was cleaned once, auto-recommitted). Fix: **new PRD 00199 task** (gate migration on positive evidence; then remove the stray file).
**F14 · architecture (lens 1) · `contains`/`startsWith` don't escape LIKE wildcards** (`ddb-server/src/filter.rs:1076-1082`; bound, so no injection — but `_`/`%` in values silently wildcard-match in a snake_case domain). Fix: **scope amendment to 00182** (escape + `ESCAPE '\'`).
**F15 · architecture (lens 1) · 00183's split list is stale**: 14 production modules exceed the 800-line cap (dml.rs 1663, ddl.rs 1654, materialize.rs 1309, …); the PRD names 3. Fix: **scope amendment to 00183** (+ owns the `sql_conn` transitional-exception exit, which had no tracking PRD — lens 6).
**F16 · security (lens 3) · NoSQL scan/backlinks endpoints are unbounded** (`service/utility.rs:132-158`; REST clamps to 200, NoSQL has no limit). Fix: **scope amendment to 00180**.
**F17 · downstream (lens 5) · Suspected: "default-zoned TEXT/REFERENCES columns silently write literal NULL" (jink AGENTS.md:78) — UNVERIFIED at HEAD.** Every SQL NULL path traced today collapses to `""` (`dml.rs:301,903`, `expr_to_string:811`, `extract_default:715`), so the defect is either fixed since jink observed it (PRD 00122-era) or triggered via an untraced path (GraphQL `fields` JSON null). Fix: **new PRD 00198** (repro against the deployed binary + HEAD; fix or retract, correct jink's gotcha, pin with a conformance case).

### Low

- **L1** HLC revwalk depth-cap (100) silently degrades tie-breaks to theirs-wins (`git_ops/read.rs:82-97`) — **folded into 00166**.
- **L2** No general index-schema version gate; only an ad-hoc FTS-column probe (`indexer/mod.rs:132-180`) — **folded into 00169** (stamp `PRAGMA user_version`, rebuild on mismatch).
- **L3** YAML frontmatter of synced files has no size/alias-expansion cap — **folded into 00169** (bound parse cost at the boundary). UNVERIFIED: serde_yaml's internal alias limits not traced.
- **L4** `SearchResult.updated_at` is index-stamp time, not document time (`indexer/mod.rs:261`) — document or derive from commit time; **00199**.
- **L5** `set_auto_maintenance` silently replaces a corrupt config (`service/ops.rs:63`) — fail-loud; **00199**.
- **L6** Write lock unusable on gitfile layouts (worktrees/submodules; `write_lock.rs:52-63`); `fs2` crate unmaintained (fs4 is the fork — UNVERIFIED currency) — document limitation; **00199**.
- **L7** Dead traits: `TypedMaterializationPort` (traits.rs:308 + ports.rs:313, zero consumers), `ConflictResolver`/`DefaultResolver` (traits.rs:357 + crdt_resolver.rs:828, never constructed) — delete; **00199**.
- **L8** Doogat-ID shape check re-implemented 4×, two variants diverge (attachments.rs:31, server lib.rs:205, consistency:335 prefix-variant, parser:230 no length check) — one `DoogatId::is_valid_shape()`; **folded into 00164**.
- **L9** Micro-dupes: `capitalize`×2, `date:`-default ×3 layers, two pluralizers with different English (rename, don't merge — `pluralize` pins the stored data-key contract) — **00199**.
- **L10** CLI sync output omits `SyncReport.resurrected` — the single most safety-relevant field (`ddb-cli/src/commands/sync.rs:12-38`) — **folded into 00163** (one `outln!`).
- **L11** Pagination policy diverges per transport (GraphQL 10k error / REST 200 clamp / NoSQL+FFI unbounded) — **folded into 00176** (PageRequest policy in contract) with the NoSQL cap in 00180.
- **L12** 00180 G5 premise correction: `/ws` already enforces auth in-handler (`ws.rs:30-85`) — **00180 re-scoped** to add the missing regression test, not "add auth".
- **L13** Maintenance-verb result drift (FFI compact discards `CompactionReport`, `import_bundle` discards `SyncReport`, CLI-only `--dry-run`) — falls under the roadmap's deliberate "remaining verbs after I7 flips" deferral; **inventoried, deferred** (recorded here so it isn't silent).
- **L14** jink stale doc pointer + search-DSL stability contract (its DSL broke jink twice) — jink-side cosmetic + rides 00176's conformance plan.

## Deletion-ledger corrections (lens 5)

All ledger rows still exist and remain deliverable. Corrections: (1) sort/pagination fallback and category pre-resolution are enabled by **done** 00158/00159, not 00182/00176 — they await the **release**, then jink-side adoption; (2) `build_fields_json` shrank to one call site but the double-encoding class scattered into inline `json!().to_string()` sites — 00178's jink adoption pass must sweep all of them; (3) tests/common/mod.rs deletable subset ~150-200 LOC (00188's PRD already counts this correctly). Compatibility: 00173 is safe for jink if its conformance tests pin `extensions.code` presence and HTTP-200-on-GraphQL-errors (jink branches only on `UNIQUE_VIOLATION`).

## What the architecture gets right (don't fix)

- Domain types are adapter-neutral (zero rusqlite/git2/axum/async-graphql under `types/**`).
- `write_lock.rs` is a solid primitive (panic-safe re-entrancy, bounded timeout, fail-loud) — the gap is coverage, not mechanism.
- Panic discipline holds: FFI handles mutex poisoning; ~53 core unwraps are const-regex/guarded.
- `apply_schema` is the model contract verb (command → service → AppOutput, honest rollback reporting, defense-in-depth quoting).
- Warning wire shape `{code, message}` is uniform everywhere it exists; GraphQL drops warnings loudly, never silently.
- Contract refactors demonstrably end fix-chains (00143/00149/00155/00156 landed with no post-landing fix tails).

## Checked and safe (ruled out — don't re-audit)

- Auth coverage: every data-bearing entrypoint authenticated (incl. PgWire MD5-of-token, WS in-handler); `/health*` deliberately open.
- FTS MATCH is always a bound parameter; where-filter identifiers all escaped via `escape_sql_ident`; schema-apply DDL validated + quoted.
- Path traversal: `validate_path` rejects absolute/`..`; attachment route validates id+filename; token file 0600, never logged.
- Lock ordering SQLite-outer/git-inner holds everywhere checked; no deadlock cycle; schema-apply crash windows self-heal (index rebuilds from git).
- Sync finalize ordering crash-safe; compaction backs up before destructive steps; bundle import checksum-verified.
- No dead feature flags, no TODO/FIXME debt, zero lint-suppression markers; `IndexPort`/`SqlBackend`/`NoSqlMirrorPort`/`DoogatSource` seams all genuinely used by tests or prod.
- parser/ and git_ops/ historically stable; `dml.rs` constraint chain quiet since 2026-04-15.

## Actions taken this session

1. CI red fixed and Tier-1-verified (commit `8cfe90a`); **push pending user's SSH agent unlock**.
2. Scope amendments applied to PRDs 00163, 00164, 00165, 00166, 00169, 00172, 00174, 00176, 00179, 00180, 00182, 00183, 00193 (marked `> **Scope addition (2026-07-16 assessment)**`).
3. New PRDs authored into `dev/local/prds/backlog/`: 00171 (inserted; see Decisions) + 00195-00199 (appended).
4. **Renumber executed**: former 00171-00198 → 00172-00199; all living cross-references re-synced (backlog PRDs, roadmap, AGENTS.md, invariants.md, design-decisions.md, project memories, capsule).
5. `roadmap-2026-07.md` rewritten (Update 2026-07-16) — ordering authority updated; release-cadence policy added; jink-00026 dependency arrow corrected.
6. `invariants.md` I1 corrected (merge/sync gap named, tracked by 00163); AGENTS.md guardrails updated (Tier-1 clippy `--all-targets`; merge-path lock rule).
7. Capsule refreshed; memory updated.

## Decisions (maintainer, 2026-07-16)

1. **Release (F2): cut v0.2.7 as soon as the pending push lands and CI is green** — quickest delivery; no existing-user constraint (not production yet). Applied: standing release-after-each-phase policy in the roadmap. Blocked on the SSH-agent unlock for the push.
2. **Harness (F5): migrate soon, inserted early** — new PRD 00171 before the contract block; the AGENTS.md 4-file mandate is rewritten by that PRD when the migration completes (stays in force until then).
