---
prd: dev/local/prds/wip/00171-migrate-shell-scenarios-to-e2e-v1.md
review: 2
date: 2026-09-07
head_sha: 876a153b4039128f8052526629773fa189058c71
reviewers: alice,blake,bob,eve
agents:
  alice: available
  blake: available
  bob: available (Claude fallback)
  carl: unavailable (nested dispatch guard, exit 3 twice)
  eve: available
---

# Review: 00171-migrate-shell-scenarios-to-e2e-v1

Coordinator reconciliation below supersedes the initial decision gate where noted. The original Eve input included the design bundle; a fresh native Eve with corrected PRD-only input completed successfully. The initial DDB_BIN dismissal was false because the search omitted hidden directories. The PRD remains paused; no source change or acceptance occurred.

Diff range: `bd3e7f0b08718448261219570876cdfe14bbbbd9..74644b8825e52794f1274b3dcec9875d1a17d808`

codex_rung_guard: fired (4 codex-implemented task(s))

Incremental cycle-2 review of PRD 00171's cycle-1 rework: commits `103c8ab`
(task 15) and `74644b8` (task 16). The prior cycle's `head_sha` was `c8b72bd`;
the raw `c8b72bd..HEAD` range also contains four commits authored by a
concurrent session (`dbc128f`, `bd3e7f0`, `4a248cb`, `876a153`), so the review
diff was narrowed to `bd3e7f0..74644b8` — exactly the two 00171 commits — and
the foreign commits were supplied separately as surrounding context only. No
finding in this cycle is raised against them.

Reviewed HEAD `876a153b4039128f8052526629773fa189058c71`: clean tree,
`master` == `origin/master`.

## Reviewer availability

- Alice: available (Claude subagent, `consensus_engine: legacy`).
- Blake: available (Claude subagent, PRD-only; never received the diff, the
  file list, the review history or the settled-decisions ledger).
- Bob: available via the documented Claude fallback. `codex-run.sh` exited 3
  ("refusing nested dispatch (already inside a CLI agent)"), so his exact
  assembled doubt + de-slop prompt ran on a Claude subagent. No codex thread id
  was produced, so no `codex_thread_id` is stamped and cycle 3, if any, runs Bob
  fresh.
- Carl: unavailable. `gemini-run.sh` exited 3 with the same nested-dispatch
  refusal on both permitted attempts. Exit 3 is a dispatch guard refusal, not
  the exit-4 permanent-unavailability class, so Carl is **not** latched into
  `state.batch.unavailable_reviewers` and a later batch will try him again.
- Eve: available (Claude Fable), joined as the fifth lens because the codex
  doubt-roster guard fired on 4 codex-implemented tasks.

## Mechanical-net limitation (fail loud)

Both installed mechanical checks are Python-only and this diff is entirely
Rust, YAML and Markdown, so **neither fired**: `compute_mech_facts.py` reported
all 15 files "skipped (non-python)" and `detect_tautological_tests.py` reported
"Checked 0 test function(s) in 0 test file(s)".
`replay_tests_against_base.py` was not run at all — it drives pytest and this
project's suite is `cargo test`. There is therefore **no automated evidence**
that the strengthened assertions in `74644b8` fail against the pre-change code.
Every one was judged by reading, and every reviewer was told so explicitly.
This is the cycle-1 settled deferral (installed tooling is Python-only;
extending it is outside 00171), restated here so the gap is not read as a pass.

Context pack: generated (`dev/local/tmp/engram-pack-00171-c2.md`, 3000 tokens).

## Consolidated findings (initial snapshot)

Eve's FIX/VERIFY/KNOWN buckets were converted to issue lines for the installed
consolidator; the raw bucket text is preserved verbatim in her section below and
in `dev/local/tmp/eve-output-00171-c2.txt`. No bucket item was dropped. Severity
on converted lines is orchestrator-classified.

| Consensus | Severity | Issue | File | Task | Found By |
|-----------|----------|-------|------|------|----------|
| [2/4] | 🟡 | New test duplicates an existing insert-loop and exceeds the 50-line function guideline (73 lines); extract a shared seed helper to fix both | tests/e2e/reference_raw_id.rs | 16 | ALICE, BOB |
| [1/4] | 🟠 | PRD 00171 is not actually complete: it sits in `dev/local/prds/wip/`, all six PRD-doc checkboxes are unchecked, and `dev/local/meta/project-capsule.md:80` records "implementation complete, acceptance pending" with review cycle 2/2 not yet run | dev/local/prds/wip/00171-migrate-shell-scenarios-to-e2e-v1.md | general | BLAKE |
| [1/4] | 🟠 | Unresolved intermittent failure in the migrated e2e suite undermines the PRD's "no parallelism-induced flake" / "CI keeps identical coverage" claims. Nightly 34020494801 (head `c8b72bd`) failed on macOS: the ported `integration_warnings_update::integration_58_a_graphql_update_warnings_empty` hit a connection reset inside `ServerGuard::graphql_with_vars`'s `.expect("request failed")` (no retry/backoff against a just-spawned server); full-suite root cause undetermined | tests/e2e/common.rs:186 | Phase 1 (integration ports) | BLAKE |
| [1/4] | 🟡 | §54.D's "psql-absent graceful degradation... skip-with-note when no client is available" is an explicit PRD "No lost semantics" bullet but was not ported: the PgWire legs always drive the compiled-in `tokio-postgres` client, so the skip path never exists in Rust | dev/local/designs/00171-migrate-shell-scenarios-to-e2e-v1-mapping.md:383-388 | Phase 1 (integration ports) | BLAKE |
| [1/4] | 🟡 | [FIX] §38b2b link order [gamma, alpha, beta] makes first-linked == largest minted id, so a resolver that picks MAX(id)/last-minted still passes; only min-id and last-linked are excluded — link the middle id first (`[&category_ids[1], &category_ids[2], &category_ids[0]]`) and assert `raw_id == category_ids[1]` | tests/e2e/reference_raw_id.rs | 16 | EVE |
| [1/4] | 🟡 | [FIX] §38b2b bash 1264-1266 asserted BOTH the scalar and the object resolver (`rlcat { id label }`) on three junction rows; the new test queries only `category_id`, so an object resolver that drifts from the scalar's row is unpinned | tests/e2e/reference_raw_id.rs | 16 | EVE |
| [1/4] | ⚪ | Nightly ConnectionReset flake at common.rs:186 remains unrepaired and its exposure surface increased (full e2e suite now runs on every push, not just nightly); no retry/backoff added around the request path | tests/e2e/common.rs | 15 | ALICE |
| [1/4] | ⚪ | "timing-sensitive multi-process sections are marked serial" was not implemented literally — no `#[serial]`/`serial_test` anywhere in `tests/e2e`; isolated per-test tempdirs/ports substituted instead | tests/e2e/integration_singleton_races.rs | Phase 1 (integration ports) | BLAKE |
| [1/4] | ⚪ | Obsolete `DDB_BIN` env survives on the linux-e2e step after task 15 removed it from test.yml as dead | .github/workflows/full-validation.yml | 15 | BOB |
| [1/4] | ⚪ | `common::stdout` is a non-verb name for a newly shared cross-module helper; it reads as `std::io::stdout` / assert_cmd's `.stdout()` assertion, not "run ddb and capture output" | tests/e2e/common.rs | 16 | BOB, EVE |
| [1/4] | ⚪ | `.stdout(predicate::str::contains(id3.clone()))` is strictly implied by the new `any(\|line\| line == id3)` check on the same captured output; the `.clone()` exists only to feed the redundant predicate | tests/e2e/integration_nosql_cli.rs | 16 | BOB |
| [1/4] | ⚪ | Restored §18d2 eq-exactness pins only the hit count (1) and never which link survived; an `eq` that matched the prefix row instead of the exact one still passes | tests/e2e/serve.rs | 16 | BOB |
| [1/4] | ⚪ | Completeness of the 14-digit-id consolidation is unverified outside the five modules the diff touched (90 e2e modules, no grep available in that sandbox) | tests/e2e | 16 | BOB |
| [1/4] | ⚪ | Cycle-1 nightly macOS failure received no disposition: no retry, no readiness change, no tracked follow-up; `ServerGuard::start` still uses a stderr line plus a fixed 50 ms sleep | tests/e2e/common.rs | general | BOB |
| [1/4] | ⚪ | `serve.rs` is 2011 lines, well over the 800-line guideline; this cycle touched 6 lines in it | tests/e2e/serve.rs | 16 | BOB |
| [1/4] | ⚪ | Either-or hedge assertion `priority.as_i64() == Some(1) \|\| priority.as_str() == Some("1")` cannot distinguish the two transport representations it is meant to pin | tests/e2e/rest.rs | general | BOB |
| [1/4] | ⚪ | [KNOWN] Nightly 34020494801 macOS ConnectionReset — readiness gate reads sound (bind precedes the awaited pgwire line; a pre-bind connect would be Refused, not Reset); cause unconfirmed, single occurrence at the older c8b72bd, green on all three platforms at 876a153; ServerGuard internals are settled as PRD 00188's | tests/e2e/common.rs | general | EVE |
| [1/4] | ⚪ | [KNOWN] Inline 14-digit id checks still duplicated in eight untouched pre-existing modules; cycle-1 finding 8 was scoped to 00171-authored modules and the surgical-change rule bars rewriting files this PRD never touched | tests/e2e | 16 | EVE |
| [1/4] | 🟡 | [FIX] New shared helper `stdout()` performs an action but is named as a noun, against the action-verb naming rule, and it is now crate-wide API — rename to `run_stdout`/`capture_stdout` and update the three importers | tests/e2e/common.rs | 16 | EVE |

No `### Auto-dismissed (ledger)` section was emitted: `--ledger-dismiss BLAKE`
matched none of Blake's re-raises, because his cycle-2 wording differs from the
cycle-1 entries. Those re-raises are dismissed by gate judgment instead and
recorded in the ledger (see Decision gate).

## Verification of the eight prior actionable findings

All four lenses judged these on the code. Seven of eight are resolved; one is
partially resolved.

| # | Cycle-1 finding | Verdict |
|---|-----------------|---------|
| 1 | Per-push CI coverage dropped | **Resolved.** `test.yml:45-46` is one unfiltered `cargo test -p ddb-e2e` after "Build CLI binary"; the five `--exact` steps and `DDB_BIN` are gone; `test-ci` excludes `ddb-e2e` so nothing double-runs; the AGENTS.md sentence matches. |
| 2 | §42 `totalCount` dropped | **Resolved.** `serve.rs:1411,1447`. Two rows are seeded, so a `totalCount` ignoring the filter returns 2 and fails. |
| 3 | §44.E1 single-char containment | **Resolved.** `integration_junction_sync.rs:37-42` parses the row and asserts `== ["a","1"]`; `["abc","10"]` now fails. Stricter than the bash original. |
| 4 | §59.A cross-process read-back | **Resolved.** `rest.rs:160-167` reads the typed column back through a separate `ddb query` process, pinned to the seeded `"books"`. |
| 5 | §18d2 eq-exactness fixture | **Resolved for the prefix-eq failure mode.** Link B is now `https://example.com/page` and the expectation is 1 hit / totalCount 1, so a prefix/LIKE `eq` returns 2 and fails. It does not pin *which* row survives — see Bob's ⚪ finding. |
| 6 | §38b2b multi-junction raw-scalar pin | **Partially resolved.** The new test links out of id order and excludes a smallest-id resolver, but see the gate's confirmed finding below. |
| 7 | §17.J2 numeric id | **Resolved.** `assert_doogat_id` (14 ASCII digits) is stricter than bash's `[0-9]+`. |
| 8 | Duplicated helpers | **Resolved for the named sites.** `stdout` and the id-shape check are consolidated in `common.rs`; the sites cycle 1 named are all converted. Survivors exist only in modules 00171 never touched (verified, see gate). |

## Gate-run evidence (facts established by this cycle, not by a reviewer)

Two reviewer claims were checked mechanically at the gate rather than taken on
assertion:

1. **Bob's `DDB_BIN` finding is refuted.** `rg -n 'DDB_BIN' <repo> --glob '!dev/local/**'`
   returns **zero hits**. No `DDB_BIN` remains in `full-validation.yml` or
   anywhere else in the tree. Bob's own finding notes he had no grep available.
2. **Bob's VERIFY item is answered.** `rg -n 'len\(\) == 14|is_14_digit|is_ascii_digit' tests/e2e/`
   returns hits in `sql_expressions_test.rs`, `cli_create_fk_freshness.rs`,
   `set_flag.rs`, `typed_field_mutations.rs`, `concurrent_writes.rs`,
   `discover.rs`, `alter_column_type.rs`, `sql_lifecycle.rs`,
   `cli_create_unregistered_type.rs`, the `conformance/` drivers, and
   `common.rs:389` (the shared helper). Every non-helper hit is in a module PRD
   00171 never touched, matching Eve's KNOWN 2. The cycle-1 finding named
   specific 00171-authored sites; all of those are consolidated.

3. **Eve's FIX 1 is confirmed by reading the code.** `reference_raw_id.rs:164`
   mints alpha, beta, gamma in ascending id order, so `category_ids[2]` (gamma)
   is the **largest** id. Line 195 links `[category_ids[2], category_ids[0],
   category_ids[1]]` and line 215 asserts `raw_id == category_ids[2]`. The test
   therefore excludes a smallest-id resolver (as its comment claims) but a
   `MAX(id)` / last-minted resolver passes it unchanged. Eve's proposed fix —
   link the middle id first and assert `category_ids[1]` — excludes min-id,
   max-id and last-linked simultaneously.

4. **Alice's duplication claim is confirmed.** `reference_raw_id.rs:162-180`
   and `:320-338` are the same seed loop, differing only in table and variable
   names.

5. **Bob's §18d2 point is confirmed.** `serve.rs:1854-1860` asserts
   `hits.len() == 1` and `totalCount == 1` and never reads `hits[0]["title"]`,
   so an `eq` returning the wrong single row passes.

## Alice

Verified all eight prior findings resolved, with file:line evidence for each.
Raised one 🟡 (the 73-line new test duplicating an existing insert loop) and one
⚪ (the nightly flake, unrepaired, with increased exposure).

Her disposition of the flake, quoted because it is the sharpest framing anyone
gave it: "It is a residual risk, not a repaired one: no retry/backoff exists
around that `.expect("request failed")`, and task 15's own fix (this rework) now
runs the full e2e suite — including every server-spawn test — on every push
instead of only nightly, which mechanically quadruples this flake's exposure
surface without adding any resilience."

R1: pass
R2: pass
R3: pass
R4: pass
R6: pass
R7: pass
R8: pass
R9: pass
R10: pass
R11: pass
R12: fail
R13: pass

## Blake

Blind lens, PRD only. Independently located the code and confirmed the large
body of the work: the four shell files are gone, `tests/e2e/` carries 91
registered modules, the mapping table has 391 rows all DONE, both workflows are
rewired with `cargo build -p ddb-cli` preceding `cargo test -p ddb-e2e`,
AGENTS.md carries the new mandate, and a repo-wide search for the four shell
filenames outside `dev/local` returns zero hits.

Raised two 🟠, one 🟡 and one ⚪. Three of the four are re-raises of cycle-1
settled decisions he cannot see by construction; the flake finding is live and
is the one item this cycle could not close (see Decision gate).

B1: fail
B2: pass
B3: pass
B4: pass
B5: pass
B6: pass
B7: pass
B8: pass
B9: pass
B10: pass
B11: pass
B12: pass
B13: pass
B14: pass
B15: fail
B16: pass
B17: pass
B18: pass
B19: pass

## Bob

Ran through the documented Claude fallback (codex exit 3, nested dispatch
refused), carrying the full doubt + de-slop lens and both rubrics, read-only and
with no command execution. Confirmed all eight prior findings resolved with
file:line evidence, including two non-obvious checks: `.cargo/config.toml:4`
excludes `ddb-e2e` from `test-ci` so the new per-push e2e step does not
double-run, and the dropped 14-digit stem filter in
`integration_singleton_races.rs:107` is *strengthening* rather than weakening,
because `git ls-tree HEAD:ddb` is non-recursive so `_typedef` has no `.md`
suffix and is already dropped by `filter_map`.

Raised nine ⚪ findings, four FIX proposals, one VERIFY and four KNOWN items.
One finding (`DDB_BIN`) is refuted by gate evidence; his VERIFY is answered by
gate evidence. He explicitly flagged that the nightly macOS failure "is *not*
repaired; it needs a durable home (issue or PRD note), because nothing in this
diff changes the failure mode."

R1: pass
R2: pass
R3: pass
R4: pass
R6: pass
R7: pass
R8: pass
R9: pass
R10: pass
R11: pass
R12: fail
R13: fail

D1: pass
D2: pass
D3: pass
D4: pass
D5: pass

## Eve

This subsection preserves the initial dispatch. The authoritative corrected native Fable response is `00171-cycle-2-eve-corrected-raw.md`; the corrected lane ran both lenses with D1-D5 all pass. Its findings are reconciled below. The input correction does not consume a third review cycle.

Fifth lens (Fable), activated by the codex doubt-roster guard. Read the diff,
`common.rs`, both workflows, every touched test, the pre-deletion
`integration.sh` at `c8b72bd~1` via git history, the mapping table, the server
startup path and the reference resolver. Gave a per-finding "would it fail
against the weak version?" verdict for all eight prior findings and found #6
only partially resolved.

Raw buckets, verbatim:

```
FIX:
- §38b2b link order [gamma, alpha, beta] makes first-linked == largest minted id, so a resolver that picks MAX(id)/last-minted still passes; only min-id and last-linked are excluded — tests/e2e/reference_raw_id.rs:195,215 — link the middle id first: `[&category_ids[1], &category_ids[2], &category_ids[0]]` and assert `raw_id == category_ids[1]` (then min-id→alpha, max-id→gamma, last-linked→alpha all fail; `.find()` file-order at base_types.rs:442-445 still passes)
- §38b2b bash 1264-1266 asserted BOTH the scalar and the object resolver (`rlcat { id label }` → cherry) on three junction rows; the new test queries only `category_id`, so an object resolver that drifts from the scalar's row is unpinned — tests/e2e/reference_raw_id.rs:207 — query `{ bookmarks { items { category_id category { id label } } } }` and assert `category.id == raw_id` and `category.label` == the first-linked label
- New shared helper `stdout()` performs an action (runs `ddb`, asserts success) but is named as a noun, against the action-verb naming rule; it is now crate-wide API — tests/e2e/common.rs:377 — rename to `run_stdout` (or `capture_stdout`) and update the three importers (smoke_core.rs:1, smoke_search.rs:1, smoke_sql.rs:1)
VERIFY:
- (none)
KNOWN:
- Nightly 34020494801 macOS `integration_58_a_graphql_update_warnings_empty` ConnectionReset at common.rs:186 — the failing call is the first request after `ServerGuard::start` (58_a's only `graphql_with_vars` is the CREATE TABLE in `setup_ig_upd_demo`), but the stderr readiness gate is sound for HTTP: `TcpListener::bind` (lib.rs:89) completes before `pgwire::start` is polled inside `select!` and the harness waits for the pgwire line (pgwire.rs:356-358, printed after its bind); a pre-bind connect would surface as Refused, not Reset; tracing goes to `--log-dir` and no request path uses `eprintln!`, so the dropped stderr pipe reader (common.rs:76) cannot panic the server. Cause unconfirmed; single occurrence at the older c8b72bd, green on all three platforms at 876a153. Out of scope: the readiness/spawn internals were settled in cycle 1 as PRD 00188's, no 00171 line touches this path, and it is not reproducible from reading; record it in 00188 as a macOS flake to absorb with retry-on-reset in the health poll.
- Inline 14-digit id checks still duplicated in eight untouched pre-existing modules (sql_expressions_test.rs:31,49; discover.rs:189,227; typed_field_mutations.rs:46; sql_lifecycle.rs:54,839; concurrent_writes.rs:50; cli_create_fk_freshness.rs:63,97; alter_column_type.rs:36,104; set_flag.rs:110) — cycle-1 finding 8 was scoped to 00171-authored modules; the surgical-change rule says not to rewrite files this PRD never touched.
```

D1: pass
D2: pass
D3: pass
D4: pass
D5: pass

## Verification-check queue

`dev/local/reviews/00171-migrate-shell-scenarios-to-e2e-v1-checks-2.json` is an
empty array. Eve's VERIFY bucket was `(none)`. Bob's single VERIFY item named an
`rg` search, which is **not queued: command shape** — a grep is not a test,
lint, build or type-check, and its exit code does not encode the claim. The gate
ran it directly instead and recorded the answer above.

## Decision gate

Cycle 2 of cap 2. Loop environment variables are set, but this is the attended
00171 closeout: per the standing instruction for this session, an unresolved
high or a failed acceptance criterion is not accepted merely because the child
runs under the loop, and the cap is not raised.

### Settled by the ledger (not unresolved)

Four findings are re-raises of cycle-1 settled decisions or new items of the
same settled class. All are appended to
`00171-migrate-shell-scenarios-to-e2e-v1-ledger.json` with their reasons:

- Blake 🟠 "PRD not complete / still in wip / boxes unchecked" — **discarded**.
  Same file, same claim as the cycle-1 discard. The wip location is the process
  working as designed: the PRD stays there until review converges and a separate
  fresh finalize session accepts it.
- Blake 🟡 "§54.D psql-absent skip not ported" — **discarded**. Same claim as the
  cycle-1 discard. The skip existed to tolerate a missing external `psql`
  binary; the Rust client is compiled in and present on every platform, so the
  branch has no reachable trigger. Porting a skip that can never fire would
  reduce coverage, not preserve it.
- Blake ⚪ "no `#[serial]` markers" — **settled deferral**, twice over in cycle 1.
- Bob ⚪ `DDB_BIN` in `full-validation.yml` — **discarded, contradicts computed
  facts** (zero `rg` hits repo-wide).
- Bob ⚪ 14-digit completeness unverified — **discarded**, answered by gate
  evidence; survivors are all in untouched modules.
- Bob ⚪ `serve.rs` at 2011 lines — **settled deferral**, same class as the
  cycle-1 size deferral and the 00163 precedent.
- Bob ⚪ `rest.rs` either-or hedge on `priority` — **settled deferral**; outside
  the reviewed diff (unchanged cycle-1 code) and Bob scoped it out himself.

### Actionable, unfixed at cap (6)

These are genuine, in-scope, additive test-only repairs. Three were confirmed by
gate-run evidence rather than accepted on assertion. None was fixed, because the
rework cap is reached and this child may not open a third cycle:

1. 🟡 [confirmed] §38b2b link order does not exclude a `MAX(id)` resolver —
   `tests/e2e/reference_raw_id.rs:195,215`.
2. 🟡 §38b2b object resolver unpinned on multiple junction rows —
   `tests/e2e/reference_raw_id.rs:207`.
3. 🟡 [2/4, confirmed] The new 73-line test duplicates the seed loop at
   `reference_raw_id.rs:320-338`; one `seed_labeled_rows` helper closes both the
   duplication and the length.
4. ⚪ [confirmed] §18d2 pins the hit count but not which row survived —
   `tests/e2e/serve.rs:1854-1860`.
5. ⚪ `common::stdout` is a noun name for a now crate-wide action helper.
6. ⚪ Redundant `.stdout(predicate::str::contains(id3.clone()))` in
   `tests/e2e/integration_nosql_cli.rs`.

Items 1, 2 and 4 all bear on the PRD's named "No lost semantics" success metric,
which is why they are recorded here rather than swept silently.

### Unresolved and blocking convergence (1)

🟠 **Nightly macOS `ConnectionReset` in a migrated test, cause unconfirmed.**
`integration_warnings_update::integration_58_a_graphql_update_warnings_empty`
failed once at `c8b72bd` in Nightly 34020494801, at
`ServerGuard::graphql_with_vars`'s `.expect("request failed")`
(`tests/e2e/common.rs:186`), with 564 passed / 1 failed on macOS. The same
Nightly also failed a Windows `git_ops` log-capture unit test that is outside
this PRD.

What is established:

- At the reviewed SHA the named test passed inside the complete, normally
  parallel e2e suite on **all three platforms** (Full Validation 34092612666:
  linux-e2e, macos-full, windows-full each 566 passed, 0 failed, 2 ignored).
  The Windows `git_ops` test also passed there.
- **No code was changed for it.** No retry, no skip, no readiness change, no
  reduced parallelism was used to obtain the green result.
- Eve traced the readiness path and found it sound for HTTP: `TcpListener::bind`
  completes before `pgwire::start` is polled, the harness waits for the pgwire
  stderr line printed after its own bind, and a pre-bind connect would surface
  as `Refused`, not `Reset`. **Cause remains unconfirmed.**
- Alice established the new fact this cycle: task 15's own fix moved the full
  e2e suite from nightly-only to **every push**, so this flake's exposure
  increased as a direct consequence of this PRD's rework, with no resilience
  added.

Severity is contested 1-vs-3: Blake (blind) rates it 🟠 High; Alice, Bob and Eve
each rate it ⚪ Low with a durable home named (PRD 00188, whose scope already
owns `ServerGuard` internals by a cycle-1 settled decision). Blake's framing
invokes the PRD's "no parallelism-induced flake" clause, which is written about
the §55/§61 multi-process race sections; the flaking test is a §58 warnings
test, so that specific clause does not cover it. But all four lenses agree on
the substance: the failure is real, unexplained, unrepaired, and this PRD
increased how often it will be seen.

This gate does **not** discard it (there is no verified reason to), does not
downgrade it unilaterally, and does not record it as a settled deferral (it is
not one — no prior cycle settled it). Whether an unexplained flake in migrated
test code, with increased exposure caused by this PRD's own CI change, blocks
acceptance of PRD 00171 is an acceptance decision reserved for the maintainer.

Initial verdict: 19 findings
Tests: 1819 passed, 0 failed, 0 skipped (suite run this cycle — `cargo test-ci` at 876a153, exit 0, 3/3 groups ok: 42 + 1555 + 222)

Note on the test counts: `dev/local/autopilot/last-verification.json` records
`sha: 74644b8`, which is **not** this cycle's reviewed HEAD, so its counts were
not reused. The suite was re-run at `876a153` and the line above reports that
fresh run. The stale record was left untouched — it is the work phase's artifact.

## Coordinator reconciliation after corrected Eve input

Native `autopilot:eve` (Claude Fable 5.1), agent `a7f49e5524c242a73` in fresh host session `2d3dab3d-2726-4cc3-8044-f8be2cc9ca24`, completed once with the exact PRD and no design document supplied. All five doubt rules pass. Exact response: `00171-cycle-2-eve-corrected-raw.md`. The installed consolidator was rerun with the corrected lane and exited 0; its unmodified output is `00171-cycle-2-corrected-consolidator-output.md`.

The consolidator merges a shared factual observation with the highest severity. Its 2/4 High reset row must not be read as two High votes: corrected Eve explicitly says KNOWN/out of scope; only Blake rates the historical reset High. Three other reviewers rate the observation Low. No reset repair or root cause is claimed. The maintainer's acceptance disposition is pending through the attended parent's question; cap 2 is unchanged.

Direct coordinator check `rg -n 'DDB_BIN' .github/workflows/full-validation.yml` returned line 61. The earlier "zero hits" reason skipped `.github/` and is invalid. Its discard is superseded, and the unused env block is restored to the actionable tail. The exact-line ID and new multi-junction fixture observations were independently checked against source; the original MAX(id) fixture gap remains valid even though corrected Eve did not repeat it.

Corrected Eve also found the sibling §18z8 numeric-ID assertions, unnecessary seeding sleeps, and a documentation verification item. The coordinator ran the named documentation search; data-model.md and indexer.md contain no explicit singular first-reference rule. server.md describes the raw scalar and singular resolver and calls ID filtering "first-value", but does not explicitly define the multi-reference scalar/object selection order. Record that guidance gap without changing the resolver policy. Any sleep reduction must preserve the existing ID-throughput ceiling and must not modify minting code; the proposed 3-second saving is unproven because ID allocation itself may wait.

Current unresolved list (one acceptance decision plus ten bounded tail observations):

1. Historical macOS reset: maintainer disposition pending; contested severity High 1/4 versus Low 3/4. Current CI is green; cause unconfirmed.
2. Middle-ID-first fixture must exclude MAX(id), MIN(id), and last-linked selection (coordinator-confirmed).
3. Add multi-junction object/scalar/label agreement (both Eve dispatches).
4. Consolidate the duplicate seed loop (Alice/Bob; confirmed).
5. Assert the identity of the exact search hit, beyond count (Bob; confirmed).
6. Rename the shared action helper to run_stdout (Bob/both Eve dispatches).
7. Remove the redundant contains predicate (Bob/both Eve dispatches).
8. Remove obsolete DDB_BIN from full-validation.yml (Bob/corrected Eve; initial discard superseded).
9. Strengthen §18z8's two sibling nonempty-ID assertions (corrected Eve; source-confirmed).
10. Assess unnecessary seeding sleeps while preserving the existing minting ceiling (corrected Eve; no performance claim accepted without validation).
11. State the existing singular first-reference selection semantics in developer guidance (corrected Eve's named verification check run; guidance gap remains).

These observations have not been implemented or accepted. No PRD checkbox or done move has occurred. The original six source repairs plus this corrected-input supplement belong to the existing bounded tail-sweep procedure if the maintainer resumes; no increased cap or new review cycle is authorized. All other original settled decisions remain unchanged. Review preparation and current-SHA local/CI verification are complete, and both child sessions have exited.

Verdict: 11 findings

## Maintainer acceptance — 2026-09-07

The maintainer explicitly accepted all 11 reconciled findings as deferrals at `876a153` and waived the remaining tail sweep. [Acceptance record](00171-acceptance-2026-09-07.md) preserves the exact approval, each finding, and SHA-specific validation. This supersedes the earlier paused/pending disposition; the findings and raw review verdicts remain unchanged. All mandatory lenses and D1–D5 rubric requirements ran, and the installed review gate with `--require-codex-guard --assert-constraint-met` passed. No repair of the macOS reset or remaining assertion gaps is claimed. Proceed only to a fresh finalize session, then stop before 00172.

Acceptance: maintainer accepted with 11 explicit deferrals; tail sweep waived.
