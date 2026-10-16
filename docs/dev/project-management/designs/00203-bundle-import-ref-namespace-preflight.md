# FT-7 preparation — 2026-09-26

Prepared cards only; no implementation, Rust gates, independent review,
commit or push in this task. The user authorized card preparation following
the completed FT-5 dependency follow-up. No catchup or autopilot loop started.

## Verified state

- master: a739c292748bcf5e35b5c6340eec7a360ceceb0c, rustls repair.
- Prior FT-5 state is complete/stop in dev/local/tmp/FT-5-state.json. Its Test
  CI 35573084487 passed on that HEAD on 2026-09-21; this is prior evidence,
  not a fresh test or remote check during card preparation.
- Existing user changes: ddb-server/src/actor/tests.rs (124 insertions), .kiro/
  untracked. Preserved. No stashes at inspection. Do not stash, commit or
  remove these to satisfy the harness's clean-tree precondition.
- Retain the unfinished fast-track plan. S3 leftovers, FT-8/9, PRD00206 and
  releases are outside this preparation.

## Source findings and proposed decisions

| Finding in current code/spec | Card decision |
| --- | --- |
| PRD Phase 0 changes fetch destination; Phase 1 changes hardcoded merge target and parent-wide cleanup. The old S4 split could leave a broken intermediate commit. | FT-7a includes identity, fetch, delivered-master validation, merge, cleanup, core regressions and current-behavior docs. FT-7b supplies public-interface evidence and closure. |
| import_bundle uses the shared refs/remotes/bundle namespace; merge_bundle_and_resolve selects bundle/master; delete_all_bundle_refs deletes the parent prefix (bundle/mod.rs). | One exact payload namespace, advertised-head/OID validation, merge only this payload's master, cleanup only this namespace. Retain --no-prune and no force. |
| The PRD's 12-character digest is marked a guess. Same-file concurrent retries would still share refs. compute_bundle_checksum/hash_dir_recursive concatenate names/content without structural framing and drop directory-entry errors. | Use a full streamed SHA-256 of objects.bundle after existing archive verification. This identifies the Git payload, not the entire tar. Leave v1 checksum/export compatibility unchanged; do not claim its existing checksum authenticates content. |
| A stable payload namespace can be cleaned by a concurrent same-payload importer. | One separate OS-backed import lease per Git common directory, from before repo mutation through cleanup/error. Reuse the existing fs2 acquire helper and a distinct persistent file; no dependency or general lock redesign. |
| GitBackend::with_write_lock forbids subprocess/rebuild/SQLite work inside its critical section. Existing merge/apply/delete use that lock separately. | The import lease is an outer, different lock. Keep existing Git/index ordering. It serializes cooperating importers only; it does not close the three-lease HIGH, unlocked preparation or other writers' rebuild races. |
| unbundle_git_objects currently returns success if objects.bundle is absent. A preexisting namespace also cannot prove what the current file advertised. | Missing payload fails before mutation. Read advertised heads from this Git bundle, validate preexisting candidate refs and fetched OIDs; no fallback to a stale/first/main branch. |
| Final cleanup currently precedes index.rebuild. A late failure can therefore report failure after refs were deleted. | Move own-namespace cleanup after existing post-merge work. Preserve refs on pre-cleanup failure; report partial cleanup errors honestly. Do not promise rollback of a clean merge or atomic multi-ref deletion. |
| A real failed-A fixture may fail again when wrongly selected for B, hiding the false-success bug. | Make A mergeable locally before importing masterless B. Baseline must fail on B's result/data, not merely a changed namespace string. Check HEAD/tree and exact ref/OID custody. |
| CLI and FFI both call DoogatService::import_bundle, but shared unit coverage alone is not conformance. FFI maps Sync to DdbError::Git and returns void. | FT-7b executes the same golden workflow through the actual CLI and public DoogatDriver; preserve signatures/mapping and do not claim report-shape parity. |
| The PRD proposes CLI-only --abort, while current product rules require coherent public recovery capabilities and app-contract-first design. | Draft cards defer abort. No CLI/service API is added; the original abort deliverables stay visible and unresolved until the maintainer chooses scope. |

The lease choice deliberately serializes different payload imports too. It
keeps one permanent lock file and avoids a new per-import reservation or
ownership database. Acquire waits at most 30 seconds; the operation itself
has no new overall deadline. Reuse the existing Conflict class for this
pre-merge contention and report the import lock, not a misleading Git-write
lock. Existing merge failures retain Sync + `bundle merge failed:`.

Legacy flat refs and other namespaces are retained automatically. Strict
candidate namespace checks refuse unexpected names/OIDs rather than forcing
or deleting them. Same Git payload with different node metadata shares a
namespace deliberately; metadata processing still runs. SHA-256 collision
resistance is the standard identity assumption, not a mathematical uniqueness
claim. Manual Git ref edits and older importers bypass the new advisory lease.

## Cards and execution boundaries

- dev/local/tmp/FT-7a-card.md — coherent core correction, fail-first unit oracle,
  safety/compatibility docs and changelog. Expected runtime edits: bundle only.
- dev/local/tmp/FT-7b-card.md — CLI/FFI workflow tests, showboat and evidence
  closure. Depends on reviewed, committed FT-7a. No runtime workaround allowed.

Before build, recheck branch/HEAD, remote master, status, stashes and worktrees;
reconcile the pending scope answer. If unrelated edits remain, create an
isolated checkout under dev/local/worktrees from the verified base. Preserve
the user's current checkout. Do not reset/stash their changes or run a harness
that stages all files. Copy only the approved card/context into the build
session and record the exact base and candidate.

Use the installed current skills/personas/harness after inspecting them;
the old plan's autopilot/0.5.1 path and model names are historical, not verified
current tool locations. Do not bypass nested-dispatch guards. Prior FT-5
evidence recommends a fresh Claude driver for the complete external roster.

For EACH item: fresh implementor, driver-owned sequential pinned gates, save
logs/candidate, END the build session, then fresh independent review. Run all
required consensus, blind/PRD-only and doubt/de-slop R1-R5 lenses. Preserve the
plan's complete roster: Rita, Cora, Grace, Toby, Mallory, Trent, Eve, Blake,
Bob (Codex), Carl (Gemini), plus Victor verification of every CRITICAL/HIGH
candidate. Use current mandatory equivalents if persona names have changed;
never omit a model family or lane. At most one review rework, followed by
per-finding closure and affected gates. Only ready items may be committed.

The old same-session harness example is superseded by the current mandatory
fresh-session build/review boundary. Preparation self-review below is not a
runtime review lane. After FT-7b, the explicit batch test-ci/full-e2e gates,
reviewed commits, master push and exact-HEAD CI remain required. Stop after
FT-7; do not execute old S5/S6 or dispatch a release.

00203 remains in backlog. Do not mark the abort tasks complete or move the PRD
to done while its scope is unresolved. If deferral is approved, revise the
PRD's retained deliverables explicitly and keep a linked deferred recovery
requirement. The 00168 note may close only the demonstrated CRITICAL; preserve
the separate write-lease HIGH and original failure evidence.

## Preparation self-review

Applied review-with-doubt to the draft artifacts; this is source inspection
and planning validation, not execution of the proposed tests.

| Question | Doubt, classification and action |
| --- | --- |
| Least confidence | FIX: the old phase split could fetch to a new path but merge the old one. FT-7a now includes the whole core path and its docs. |
| Assumptions | FIX: short checksum identity and “two imports never contend” were unsafe assumptions. Full payload hash plus a distinct import lease replaces them; v1 format remains unchanged. |
| Complete task | FIX: silently deleting the abort deliverable would misstate PRD completion. Cards are drafts, the choice is pending, original requirements remain visible, and closure rules forbid premature completion. |
| Practical failures | FIX: cleanup-before-rebuild violated failure custody; stale-A fixtures could mask the bug. Cards pin late-failure custody, cleanup-error limits and a mergeable-stale-A baseline oracle. |
| Retrospective | VERIFY, dismissed: the existing GitBackend can address bundle/<id>/master and delete scoped branches, and write_lock::acquire accepts a distinct file name. No new public trait or concrete GitRepo construction is required for this proposed design. |
| Missing validation | KNOWN: no fail-first run, runtime gate, concurrency proof, independent review or fresh CI yet. These belong to later build/review work; none is claimed complete here. |
| R1 bloat | FIX: keep the detailed source rationale in this note and actionable test/allowlist/constraints in two cards; no new PRD or execution framework. |
| R2 impossible defenses | VERIFY, dismissed: missing objects.bundle and leftover candidate refs are reachable states in current code; refusal tests cover real inputs, not impossible internal states. |
| R3 one-caller abstraction | FIX: require existing lock primitive and local bundle helpers; exclude a generic reservation service, new dependency or public backend method. |
| R4 paraphrasing | VERIFY, dismissed: docs changes explain custody, lock ordering and compatibility limits; no runtime comments were added. |
| R5 library tests | FIX: concurrency tests must exercise import-path wiring and ref custody, not merely re-prove fs2 exclusion; CLI/FFI checks must invoke their public entry points. |

First pass: 11 doubts, 7 fixed in the cards, 3 verified/dismissed by source
inspection, 1 known execution limitation. Second pass checked the revised
cards for standalone commit safety, full review coverage, unchanged FFI error
mapping, unrelated-file custody and honest PRD status. It tightened the late
failure test to fail specifically at the final rebuild (Index::conn is
crate-visible) and removed duplicate concurrency authoring from FT-7b.
Card headings and file references passed validation; all five planning files
passed whitespace checks and are gitignored. git diff --check passed; tracked
status still contains only the pre-existing 124 actor-test insertions, with
.kiro/ still untracked. No Cargo or runtime commands were run. Remaining design
verification is explicitly assigned to fail-first tests and the fresh review;
the abort scope question below is still unanswered.

## Approval — 2026-09-27

Maintainer said "ok to proceed" after the recommendation to defer abort. The
cards and PRD now use that scope: isolation first, no new recovery API. Earlier
pending/draft statements above are the preserved 2026-09-26 preparation record.
The original PRD is preserved in dev/local/designs/00203-pre-FT-7-scope.md.

Remote master was rechecked through GitHub API and still equals a739c29.
The main checkout now has actor/mod.rs (268 additions, 5 deletions),
actor/tests.rs (122 additions) and .kiro/ changes, all unrelated and preserved.
The build uses dev/local/worktrees/ft-7a on fast-track/ft-7a from exact base
 a739c292748bcf5e35b5c6340eec7a360ceceb0c. No old FT-5 state is reactivated.

## Unresolved questions

None for the approved FT-7 scope. Future abort semantics/parity require a
separate design before implementation.
