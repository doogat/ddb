# Backlog review - 2026-07-08

Scope: all 28 PRDs in `dev/local/prds/backlog/` (00162-00189), `dev/local/prds/roadmap-2026-07.md`, `docs/src/technical/invariants.md`, AGENTS.md invariant sections, project memory.

## Verdict

The backlog is structurally sound: correct priority spine (data-loss -> convergence -> app contract -> hardening -> downstream enablement), every PRD session-sized (70-170 lines), every `Depends on:` points to a lower number, and nothing re-litigates settled positioning. Five defects need fixing before the next autopilot run; the worst two will actively misdirect it.

## 1. Internal collisions

### C1 - 00162 is shipped but still in backlog (blocks autopilot)

Commits ff10bbe (lock + wraps), c25251c (e2e + integration scenario + invariant doc), f2145ff (docs) implemented PRD 00162. Verified on disk: `ddb-core/src/git_ops/write_lock.rs` and `tests/e2e/concurrent_writes.rs` exist. Autopilot picks the lowest-numbered backlog PRD first, so the next run re-enters a completed PRD. Fix: verify the shipped state against the PRD's exit criteria, tick, `mv` to `done/`.

### C2 - post-renumber cross-reference drift (systemic, highest risk)

AGENTS.md ("Data-safety invariants" + "Coherence invariants"), `docs/src/technical/invariants.md` (every I1-I9 entry), and two memory files still cite PRE-renumber PRD numbers. Mapping:

| Invariant | Docs say | Actually is |
|---|---|---|
| I1 write lock | 00166, 00167 | 00162 (SHIPPED), 00163 |
| I2 merge deletions | 00167 | 00163 |
| I3 ID minting / ceiling | 00168, 00186 | 00164, 00170 |
| I4 CRDT determinism / HLC | 00169, 00170 | 00165, 00166 |
| I5 one bad file | 00172 | 00169 |
| I6 one error policy | 00162; legs 00177/00185 | 00172; legs 00176/00179 |
| I7 app-contract-first | 00174-00178 | 00173-00177 |
| I8 one result type | 00176 | 00175 |
| I9 SQL identifiers | 00184 | 00178 |

Concrete failure mode: invariants.md today says SQL-identifier escaping is "Closed by 00184" - new 00184 is the ddb-client crate. An agent checking whether a gap is already scoped opens the wrong PRD, or worse "confirms" a gap is covered when it is not. I1's status is also stale: the lock half shipped, so I1 is part-HOLDS with only the 00163 delete-freshness half open.

Same staleness in memory: `project_data_loss_write_path` ("tracked 00166/00167") and `project_two_error_mapping_paths` ("fix scoped in rewritten 00162"). And `roadmap-2026-07.md` says "27 files, 00162-00188" - 00189 was appended later and is not listed.

Prevention: cite PRDs as `slug (number)` in AGENTS.md/invariants.md so the next renumber cannot silently break them.

### C3 - 00186 targets an FFI surface 00176 deprecates

00186's structural plan wires `on_conflict` onto the raw `ffi/driver.rs create_doogat`. By execution time (00186 runs after 00176) that method is `#[deprecated]`; the contract path is `create_structured` + `CreateResultRecord`, which is also where the new `existed` flag must ride. One-paragraph PRD edit.

### C4 - 00171 <-> 00173 rework overlap (documented, tolerable)

00171 builds the maintenance-worker dispatch inside the twin-enum actor; 00173 deletes those enums two PRDs later and re-plumbs the same three maintenance commands as closures. Both PRDs already pin the constraint (worker split must survive 00173; the 00171 concurrency test is the oracle), so this is safe - just a small double rewrite of `actor/mod.rs`/`handlers.rs`. Swapping order removes the rework but costs cross-ref churn in ~6 PRDs (00174-00177 depend on 00173 by number) and delays the availability fix. Recommendation: keep the current order.

### C5 - minor, no action needed

- 00178 adds `quote_ident` into `sql_engine/helpers.rs`; 00182 then splits that file. Sequenced correctly (add, then pure-move).
- 00181 edits `filter.rs`/`materialize.rs` after 00178 escapes and before 00182 splits. Correct order.
- 00184 claims "SQL_ERROR is not in ddb_core::error::codes"; 00172 registers it first, so the claim will be stale at execution. 00184 already carries the 00172 cross-ref; the session will reconcile.
- Unverified wrinkle for the 00169 session: its acceptance requires a *search* to carry the skip-warning, but search is not contract-routed until 00175. Confirm the GraphQL warning-extension channel is verb-agnostic before pinning that acceptance (I did not trace this in code).

## 2. Does the finished backlog leave the project in the best shape to build on?

Yes, structurally. End state: I1-I9 flip to HOLDS, transports become thin adapters over one contract + one error table, and a new downstream starts from `ddb-client` + offline SDL + codegen + `--ephemeral` instead of hand-rolling ~2,400 LOC of glue. The root-cause bet (finish the half-built app contract) is the right one. Three caveats:

1. The steering docs future sessions read (AGENTS.md, invariants.md) are mis-pointed today (C2). Fixing them is worth more than any single PRD for keeping later sessions on course.
2. I7 ("every verb through the contract") only ~closes: after 00173-00177, the sql passthrough, sync/compact/maintenance, discovery, and batch verbs still have no AppCommand DTOs. Either declare them specialized surfaces when flipping I7 to HOLDS, or scope one small follow-up PRD. Decide at flip time.
3. 00189 (cold-start rebuild race) runs LAST, after demo-facing Phase 3. A known availability bug (currently masked by a warm-up workaround in the 00162 e2e) outlives the enablement work.

## 3. Regrouping opportunities

Mostly: don't regroup. The split discipline is good, and the two tempting merges (00165+00166 convergence pair; 00185+00188 schema tooling pair) would each overflow a session. Worthwhile moves:

- **Move 00162 to done** (C1).
- **Fold `ddb-server/src/filter.rs` bare `format!("\"{col}\"")` quoting into 00178.** 00178 defers it to "a separate follow-up" that does not exist; 00178 is already consolidating the canonical helper, so one extra task closes I9 completely instead of leaving an untracked remainder.
- **Pull 00189 earlier** if the availability bug should not wait behind Phase 3. Cheapest mechanism: rename to `00169b-serialize-cold-start-index-rebuild-v1.md` (sorts lexically between 00169 and 00170, so lowest-first pickup honors it). Hacky suffix, zero churn elsewhere. Full renumbering is not worth it.
- **Optional 00171<->00173 swap** (C4) - recommend against; rework is smaller than the churn.

## 4. What is missing?

1. **The C2 renumber fix itself** - a chore commit, not a PRD; do it before the next autopilot session.
2. **filter.rs identifier quoting** - in no PRD today (see above; fold into 00178).
3. **I7 remainder** - sql/sync/discovery/batch contract routing: a scoping decision (documented specialization vs. one small PRD).
4. **00184 dependency-weight re-decision.** The PRD pre-decides to pull `ddb-core` (git2, rusqlite, automerge) into a thin HTTP client to reuse ~13 string constants. Every downstream then compiles native deps for an HTTP client - a real adoption tax that cuts against the "consumable" end state this PRD exists to serve. Cheaper safe alternative: define the code constants in `ddb-client`, add a workspace parity test with ddb-core as a **dev-dependency only** (downstreams never compile git2; drift still caught in CI). A 30-minute delta now vs. a permanent tax.
5. Nothing else found: 00189 covers the index-rebuild race from memory; the deliberately-deferred list (parameterized SQL, >5K scale, ID format change) is parked with explicit revisit conditions; every PRD carries Transport Impact + a conformance plan per AGENTS.md.

## 5. Is any PRD undermining the goal?

No PRD contradicts the settled positioning (standalone platform, six transports, 120% parity); 00172 + 00174-00177 are the parity engine itself. Two soft spots inside otherwise-aligned PRDs:

- **00184's hard ddb-core dependency** (above) mildly undermines "consumable".
- **00186's `Update`-upsert variant** says "explicit maintainer decision". Under autopilot autonomy the right default is: ship `ReturnExisting`, record `Update` as explicitly excluded (mirror 00170's delegated-decision pattern) - otherwise a session may stall or, worse, ship the wider write surface unprompted.

## Action list (smallest first)

1. Verify + move 00162 to `done/`.
2. Renumber-fix AGENTS.md invariant refs, `invariants.md` (numbers + I1 status), `roadmap-2026-07.md` (add 00189), and the two stale memory files; switch to `slug (number)` citation form.
3. Patch 00186's FFI leg to `create_structured`/`CreateResultRecord`.
4. Add the filter.rs quoting task to 00178.
5. Decide 00189 placement (rename to `00169b` vs. accept last).
6. Decide 00184 codes strategy (dev-dep parity test vs. hard dep).

## Applied (2026-07-08, all four options approved)

1. 00162 verified complete on disk (fs2 dep, `write_lock.rs`, `concurrent_writes.rs` e2e, integration scenarios 61.A/61.B, git-ops.md invariant section) → checkboxes ticked, moved to `done/`.
2. Cross-refs re-synced to the renumbered backlog: AGENTS.md invariant sections, `docs/src/technical/invariants.md` (all I1-I9; I1 now PARTIAL; PARTIAL added to the legend), `roadmap-2026-07.md` rewritten, 4 memory files + MEMORY.md index.
3. 00186 FFI leg retargeted to `create_structured`/`CreateResultRecord`; now depends on 00176; `Update`-upsert recorded as autopilot-default-excluded.
4. 00178 gained the `ddb-server/src/filter.rs` where-compiler quoting leg (invariant I9 now fully tracked by one PRD).
5. 00184 switched to in-crate code constants pinned by a ddb-core dev-dependency parity test (no runtime ddb-core dep for downstreams).
6. 00189 renamed to `00169b-serialize-cold-start-index-rebuild-v1.md` (runs right after 00169, before Phase 2/3).
7. 00171 and 00173 swapped: 00171 = collapse-actor-plumbing (runs first), 00173 = decouple-maintenance (worker built on the closure model). Constraint text in both PRDs inverted; all 00174-00177 depends-on refs updated.

Backlog after changes: 27 PRDs, 00163 lowest (next autopilot pick). Tracked-file changes (AGENTS.md, `docs/src/technical/invariants.md`) committed to master.
