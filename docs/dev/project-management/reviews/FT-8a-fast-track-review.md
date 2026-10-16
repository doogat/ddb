---
item: FT-8a
date: 2026-09-30
base_sha: a3e2dbc59b340731a4bd46752c6ae52fba80cbff
outcome: ready
driver: fresh Claude session (Opus 5.5)
reworks: 1 (plus one driver gate-fix, maintainer-approved)
lanes:
  rita: ok (APPROVE; 1 LOW)
  cora: ok (APPROVE; 1 MEDIUM, 1 LOW)
  grace: ok (APPROVE; 2 MEDIUM, 3 LOW)
  toby: ok (APPROVE; 1 MEDIUM, 3 LOW)
  trent: ok (R1/R12/R13 fail; 1 MEDIUM, 4 LOW)
  mallory: ok (APPROVE; run explicitly, git write path; no findings)
  victor: ok (2 candidates, both CONFIRMED; H1 downgraded to LOW-MEDIUM, H2 held at MEDIUM-HIGH)
  eve: ok (fable; D1-D5 pass; 4 FIX, 1 VERIFY, 8 KNOWN)
  blake: ok (card only; R12/R13 fail; 4 LOW)
  bob: ok (codex via autopilot 0.5.6 codex-run.sh; 2 HIGH, 3 MEDIUM)
  carl: ok with caveat (copilot gemini-3.8-flash via autopilot 0.5.6 gemini-run.sh; sandbox denied the scratchpad context file, so it read the handoff doc, which carries the same attention points; 9 MEDIUM, all size)
---

# FT-8a fast-track review

## Summary

Review of the build candidate `dev/local/tmp/FT-8a-build-candidate.patch`
(SHA-256 `51f59851…766d`). Before review it was re-verified byte-identical to
the live worktree diff: 13 files, +618/-72. Master, origin/master and
ls-remote were all at a3e2dbc. No stashes, three expected worktrees, no
foreign session in the checkout.

The roster ran by hand through the plan §1 manual fallback (the harness
refuses a dirty tree and re-implements):

- Fan-out lanes: Rita, Cora, Grace, Toby, Trent and Mallory (armed
  explicitly: git write path).
- Doubt and blind lanes: Eve (doubt) and Blake (card only, never the diff).
- External lanes: Bob (codex) and Carl (gemini) through the plugin helpers,
  as background Bash. Prompts and raw output are in `FT-8a-logs/`.

All ten lanes returned. CRITICAL/HIGH raised: **2**, both from Bob, and both
went to Victor. Both were CONFIRMED.

- One rework: a fresh Opus Ivan, spec `dev/local/tmp/FT-8a-review-rework.md`
  (9 items), edits only; the driver ran all Cargo.
- Fail-first replay of the two bug fixes on the pre-rework runtime: both red
  for the exact defects (below).
- Nine independent Sonnet closure checkers, one per rework item: 8 CLOSED, 1
  OPEN on test length only (accepted, #14).
- Strict clippy then failed on one rework test-helper line
  (`manual_contains`). With the rework cap spent, the maintainer approved a
  driver gate-fix: clippy's exact suggestion `taken.contains(&c)` in
  `git_ops/tests.rs` (test code, no behavior change). Every gate was rerun
  from the start.
- Rework snapshot before the gate-fix: `dev/local/tmp/FT-8a-rework-candidate.patch`
  (SHA-256 `3543b45b…05ac1`), 13 files, +799/-96. Final candidate: see Gates.

## Maintainer decisions this session

1. Horizontal-rule predicate (open point 2): **pin and defer**. The rework
   adds a test that pins today's rule (two `---` lines anywhere count as a
   block) plus a doc note; the residual wedge goes to the PRD 00204 follow-up.
2. Clippy failure after the rework: **driver applies clippy's suggestion**
   and reruns all gates.

## Findings and dispositions

| # | Sev | Lanes | Finding | Disposition |
|---|-----|-------|---------|-------------|
| 1 | HIGH -> MEDIUM-HIGH (Victor CONFIRMED) | bob | The verbatim branch re-blobbed `loser.content`, which `read_blob_content` decodes with `from_utf8_lossy`. A frontmatter-less loser with invalid UTF-8 landed changed (`caf\xE9` -> `caf` + U+FFFD) while reported as kept verbatim. New in FT-8a; at a3e2dbc the same input aborted loudly. | Fixed (rework item 1): the verbatim branch stages the loser's original blob (`Oid::from_str(losing_blob_oid)`, error mapped to `Conflict`). Test `frontmatterless_loser_with_invalid_utf8_lands_its_original_blob`. Fail-first: red on the pre-rework runtime with exactly the U+FFFD bytes. Closed. |
| 2 | HIGH -> LOW-MEDIUM (Victor CONFIRMED) | bob | A later loser's inbound-link scan walked paths that earlier losers in the same batch had staged, so it could rewrite a verbatim loser's `[[old_id]]` while the record still said verbatim. The scan over staged losers was already present at a3e2dbc; the false verbatim claim is new. No data loss. | Fixed (item 2): later link rewrites skip every earlier kept-verbatim `new_path`; fold doc comment updated. Test `later_loser_link_rewrite_never_touches_an_earlier_verbatim_loser`. Fail-first: red on the pre-rework runtime (link rewritten). Closed. |
| 3 | MEDIUM | cora, toby, trent, eve (F1); rita, blake LOW | `has_frontmatter` = `find_frontmatter(..).is_ok()` accepts `---` on any line. A frontmatter-less body with two horizontal rules takes the rewrite branch and either still wedges (text between the rules is not a YAML map) or, pre-existing at a3e2dbc, loses its preamble (Eve F2). Card- and PRD-conformant. | Maintainer: pin and defer. Test `has_frontmatter_counts_paired_horizontal_rules_as_a_block` plus a doc sentence (item 3); closed. The residual wedge and the preamble drop go to the PRD 00204 follow-up (a parser-level fix must change `split_zones` together with the predicate). |
| 4 | LOW | toby | No test that a verbatim loser never lands when a later loser in the batch aborts. | Fixed: `verbatim_loser_does_not_land_when_a_later_loser_aborts` (item 4); closed. Pins behavior the build already had. |
| 5 | MEDIUM | trent (R1); toby LOW | Card Goal 2 "inbound-link rewriting still runs" was untested for a verbatim loser. | Fixed: `verbatim_loser_still_gets_its_inbound_links_rewritten` (item 5); closed. Pins existing behavior. |
| 6 | LOW | toby, trent, eve (F5) | The sync test asserted only that the new path exists, not its bytes. | Fixed: bytes at `new_path` and at `path` are each one of the two inputs and differ (item 6); closed. |
| 7 | MEDIUM | grace, bob | Duplicated setup: the new bundle test copied `import_with_unresolvable_collision`; the sync test copied `setup_binary_sync_pair`; the derived id/path block repeated four times. | Fixed: `import_with_collision(loser_content)`, `setup_binary_sync_pair()`, `derived_id_and_path` and one `UNREWRITABLE_LOSER` const in git_ops tests (items 6-7); closed except #14. |
| 8 | LOW | grace, blake, trent, carl, bob | `fold_one_loser` grew to 63 lines. | Fixed: `loser_blob_oid` extracted; `fold_one_loser` is 50 lines and still holds the only `has_frontmatter` call (item 8); closed. |
| 9 | LOW | grace, eve (F3, F4, F12) | Over-wide doc comments; the warn log could not tell a fold from a rewrite; "moved" wording implied `old_path` is gone; the CHANGELOG implied visible CLI output. | Fixed (item 9): reflow, `kept_verbatim` warn field, "landed unchanged at `new_path`; `old_path` now holds the winner", CHANGELOG note that CLI/GraphQL rendering follows. Closed. |
| 10 | MEDIUM | bob | The build's fail-first replay used a one-token shim, not a3e2dbc production code. | Accepted: the card allows shape adapters, since the new tests cannot compile against the old `CommitHash` return. Eve verified the replay `merge.rs` differs from the candidate by that one hunk only, and the driver verified (word diff) that the replay test copy differs only by rustfmt line joins (Eve F13 VERIFY, closed). This session's rework replay ran with the shim removed. |
| 11 | MEDIUM | carl, trent, blake, bob, grace | Touched test files are over 800 lines (git_ops/tests.rs ~4000, sync_manager/tests.rs, bundle/tests.rs, ffi/tests.rs, parser/tests.rs); parser/mod.rs 989. All were over before this item. | Deferred: a split needs new files outside the allowlist. Goes to the PRD 00204 follow-up with the FT-7 split deferral. |
| 12 | LOW | carl | `has_frontmatter` allocates a `Vec<&str>`. | Accepted: `find_frontmatter` takes a slice; the call runs once per collision loser. |
| 13 | LOW | toby | `taken_ids` insertion on both branches is not bound by a test. | Accepted: the insert is shared code before the branch. |
| 14 | LOW | closure checker item 7 | `verbatim_loser_still_gets_its_inbound_links_rewritten` is 52 lines and `sync_report_lists_every_verbatim_folded_collision_loser` is 51 (target 50). | Accepted: the rework cap is spent; test code only. |
| 15 | LOW | blake | `bundle/tests.rs` guard "must not contain 'no frontmatter opening ---'" can no longer trigger. | Accepted: harmless; `collisions_reassigned == 0` still carries the intent. |
| 16 | LOW | blake, rita, ivan | The CHANGELOG Changed line is prefixed `**traits**`, where the card says `**core**`. | Accepted: matches the neighbouring Changed entries. |
| 17 | LOW | eve (F6) | The either-direction test is near-tautological (`derive_content_id` takes no direction input). | Accepted: the PRD lists it verbatim; it keeps `theirs_won=false` coverage through the verbatim branch. |
| 18 | LOW | eve (F7-F11), cora LOW | No CLI/FFI success test; walkthrough 00203 stale; sync.md:121 and invariants I1 stale; FFI `import_bundle` discards the report; a verbatim-folded file stays unindexable (and `--strict` red) until frontmatter is added. | Deferred to FT-8b (renders, e2e scenario, docs, walkthrough re-record) per the card; FFI report gap is the PRD's Transport Impact deferral. |

Open points from the handoff:

1. Card-time drift (FT-7 failing fixtures moved to a YAML-invalid loser):
   every lane that judged it called it faithful with no coverage lost. The
   PRD's own acceptance ("frontmatter-less import asserted to SUCCEED and
   report one verbatim fold") is met by
   `frontmatterless_bundle_loser_imports_and_reports_one_verbatim_fold`.
2. Predicate: #3.
3. `create_conflict_commit` signature: accepted by every lane (private, one
   caller, the minimal threading).
4. Shim replay: #10.
5. 40 rustfmt drift hunks: left alone again (3 + 32 + 1 + 4).
6. Walkthrough 00203: FT-8b; not an FT-8a gate (nothing in CI runs it).

## Gates after rework (fresh, driver, Rust 1.89.0, `--locked`, shared target)

Every gate below ran on the final tree: rework, touched-hunk rustfmt and the
driver gate-fix. Final snapshot: `dev/local/tmp/FT-8a-final-candidate.patch`
(SHA-256 `a5bb396c…67a6aa`), 13 files, +799/-96.

- Fail-first replay of the two bug fixes (replay worktree = the build
  candidate, shim removed): `frontmatterless_loser_with_invalid_utf8_lands_its_original_blob`
  red with `caf` + U+FFFD instead of `caf\xE9`;
  `later_loser_link_rewrite_never_touches_an_earlier_verbatim_loser` red with
  the link rewritten. The two coverage tests (#4, #5) were green, as expected
  for pins of existing behavior (`FT-8a-logs/rework-failfirst-replay.log`).
- `build`: pass (3m37s). Strict `clippy --workspace --all-targets -- -D
  warnings`: first run failed on `manual_contains` (rework test helper); after
  the approved gate-fix, pass (10m18s) (`rework-gate-2-clippy.log`).
- Fast tier (exact `test-ci` expansion, background, sole Cargo process):
  42 + 1621 + 226 = **1889 passed**, 0 failed, 0 ignored. That is the build's
  1884 plus the 5 rework tests (`rework-gate-3-test-ci-expanded.log`).
- Filtered core (card filters plus the new rework names): **58 passed**
  (`rework-gate-4-filtered-core.log`).
- `--test sync_test`: 6 passed. `build -p ddb-cli`: pass. e2e `collision`: 4
  passed. e2e `bundle`: 8 passed, including FT-7b's 3 re-pointed CLI scenarios
  (`rework-gates-5-7.log`).
- Full `ddb-e2e`: **572 passed**, 0 failed, 0 ignored, first run
  (`rework-gate-8-full-e2e.log`).
- rustfmt: touched-hunk pass applied 0 hunks (40 pre-existing drift hunks left
  alone); whole-file `--check` clean on the five files clean at base.
  `git diff --check`: clean.
- Walkthrough: not run. 00203 is stale by design (FT-8b re-records it) and no
  CI job executes it.

## Outcome

`ready`. Both CRITICAL/HIGH candidates were Victor-confirmed, fixed, proven red
on the old runtime and closed. Everything else is fixed, accepted or deferred
with a home (PRD 00204 follow-ups and the FT-8b card).

## Landing

- Committed `d7867d2` on `fast-track/ft-8a` with explicit paths
  (`fix(sync): fold frontmatter-less collision losers byte-unchanged`). Its
  diff against a3e2dbc is byte-identical to the final snapshot. Both
  CHANGELOG lines are kept; the Fixed line now notes that rendering follows.
  The aegis hook rejected a Co-Authored-By trailer, so the commit has none
  (user policy).
- Master fast-forwarded locally a3e2dbc..d7867d2.
- Push: the first attempt failed because the 1Password SSH agent refused to
  sign (the same failure as FT-5). Once signing recovered, master was pushed
  (a3e2dbc..d7867d2).
- Exact-HEAD CI: Test 36997037232 on d7867d2 succeeded (`test` and
  `coverage`). Docs did not run: `docs.yml` triggers only on `docs/**`, and
  this commit touches none of it.
- Checkout note: between preflight and landing, an untracked
  `docs/dev/project-management/` tree appeared in the main checkout. It
  mirrors `dev/local/` (separate copies, preserved mtimes), so it is probably
  the working-documents migration that a3e2dbc prepared placeholders for. No
  foreign session ran in this checkout. Left untouched; the fast-forward
  changed only the 13 tracked files.
