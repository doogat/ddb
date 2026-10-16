# FT-8a lane results (driver notes)

DECISION (maintainer, 2026-09-30): HR-body predicate = Pin + defer. Rework adds a pinning test + doc note; residual wedge -> FT-8b / PRD 00204 Deferred.

## cora: REQUEST_CHANGES
- MEDIUM parser/mod.rs:180-192 / merge.rs:324: has_frontmatter accepts `---` anywhere; body with two HRs -> rewrite -> abort (residual wedge, non-map YAML) or preamble dropped on valid-map YAML. Card-conformant, pre-existing at a3e2dbc. Fix: tighten (card change) or document limit + characterization test, route to FT-8b/follow-up.
- LOW merge.rs:167-172: verbatim loser unindexed (I5 warning); index-only minting could in theory reuse id; commit_file refuses overwrite. Informational; FT-8b sync.md note.
- Passed: I1 atomicity, taken_ids both branches, I3, I4, error mapping, SyncReport init, drift faithful, create_conflict_commit minimal.

## grace: APPROVE; 2 MEDIUM, 3 LOW
- M1 bundle/tests.rs:370-440 new test duplicates import_with_unresolvable_collision setup (~35 lines); parametrize helper by loser_content.
- M2 git_ops/tests.rs derived id/path block repeated 4x; helper derived_id_and_path.
- L3 YAML-invalid fixture in 6 places; one const in git_ops/tests.rs.
- L4 fold_one_loser 63 lines (was ~54); doc comment first line "rewritten frontmatter" -> "moved or rewritten content". Accept size.
- L5 bundle/tests.rs:228,:307 doc comments over wrap width.

## blake (card only): no CRIT/HIGH; R12/R13 fail
- LOW HR-body residual wedge (parser/mod.rs:180-190).
- LOW bundle/tests.rs:881 guard "no frontmatter opening ---" can no longer trigger (collisions_reassigned==0 still protects).
- LOW either-direction test same WINNER bytes; path determinism only.
- LOW size fold_one_loser 63, commit_merge ~85 pre-existing, parser/mod.rs 988 pre-existing over 800.
- CHANGELOG Changed prefix **traits** vs card **core** (cosmetic).

## rita: APPROVE; 1 LOW
- LOW-1 HR-body residual; follow-up PRD must change split_zones with predicate. Drift faithful; coverage net up.

## cora second report (same agent re-stopped): APPROVE, same MEDIUM; proposes first-non-blank-line predicate; scenario (b) "Intro\n---\n---\nOutro" loses Intro (pre-existing).

## toby: APPROVE; 1 MEDIUM, 3 LOW
- M1 HR-body residual unpinned; add has_frontmatter_treats_mid_body_rules_as_a_block (asserts true, records decision) or tighten via PRD amendment.
- L1 no test: verbatim loser + later invalid-YAML loser -> Err, HEAD unchanged, nothing at verbatim derived path.
- L2 taken_ids both-branch requirement not bound (two frontmatter-less losers forced collision).
- L3 sync test only read_file is_ok, not bytes/winner; link rewriting on verbatim branch untested; stale-ref replay not done.

## trent: R1/R12/R13 fail, rest pass
- M R1 merge.rs:243-250 link rewriting to frontmatter-less loser untested.
- L sync test bytes not asserted. L HR edge unpinned (Trent rates open point MEDIUM suspected).
- L R12 fold_one_loser ~63 (was ~52); commit_merge ~86 pre-existing; new tests over 50: mixed (~65), bundle (~60), sync (~60).
- L R13 all over-size files pre-existing.

## carl (copilot gemini-3.8-flash, exit 0, 5m44s; could not read scratchpad context.md, read handoff instead): R12/R13 fail; 9 MEDIUM, all size/alloc
- fold_one_loser 63; new tests over 50 (bundle 60, mixed 65, sync 60); has_frontmatter Vec alloc; test files over 800 (pre-existing).
- Open points: all sound; predicate matches split_zones.

## bob (codex, exit 0): R1/R2/R9/R12/R13 fail; 2 HIGH -> Victor
- H1 merge.rs:243 later loser's link-rewrite can modify an earlier kept-verbatim loser's staged bytes while it is still reported verbatim.
- H2 merge.rs:226 verbatim branch re-blobs lossy-decoded String (read_blob_content replaces invalid UTF-8) -> bytes change while reported unchanged; stage losing_blob_oid.
- M replay shim not a3e2dbc production code.
- M sync test duplicates setup_binary_sync_pair; M bundle test duplicates helper setup.
- L size.

## victor H1: CONFIRMED, downgraded to LOW-MEDIUM
- Path real: later loser B's scan_and_rewrite_links_in_index (merge.rs:243/271) walks whole merge_index incl. A's staged verbatim path; winner_tree_mentions_id false; A's [[B.old_id]] rewritten, overwrites staged blob (255-258); A's record stays "verbatim".
- No data loss (link target only); reporting-contract violation. Scan over earlier staged losers pre-existing at a3e2dbc; new = verbatim claim.
- Fix options: skip kept-verbatim new_paths in later scans (keeps byte-identical promise) OR honest record. Regression: two frontmatter-less losers, A has [[B.old_id]].

## victor H2: CONFIRMED, MEDIUM-HIGH (new in FT-8a)
- read_blob_content lossy (merge.rs:518-526); partition_conflicts no UTF-8 check; loser.content lossy; verbatim branch re-blobs string -> "caf\xE9" lands as U+FFFD, reported verbatim. At base it aborted loudly. Original survives in merge parent history.
- Fix: verbatim branch stages Oid::from_str(&loser.losing_blob_oid) directly; test with b"caf\xE9" asserting landed blob oid == losing_blob_oid.

## eve (fable): D1-D5 pass, 0 CRIT/HIGH; 4 FIX (F3 warn field, F4 "moved" wording, F5 sync bytes, F12 changelog render note), 1 VERIFY (F13 replay tests formatting-only -> driver verified: word-diff shows rustfmt joins only), 8 KNOWN (F1/F2 HR predicate + preamble drop, F6 direction test near-tautological, F7 no iface success test, F8 walkthrough 00203, F9 sync.md/I1, F10 FFI discards report, F11 unindexable until frontmatter).
- Bonus: scan runs before loser's own blob staged -> own bytes never rewritten.

## Triage -> rework spec dev/local/tmp/FT-8a-review-rework.md (9 items), Ivan opus dispatched.
Accepted/deferred: file sizes (pre-existing, split outside allowlist); has_frontmatter Vec alloc (Carl, find_frontmatter takes slice); taken_ids binding test (Toby L2, shared code before branch); bundle/tests.rs:881 obsolete guard message (Blake, still harmless); CHANGELOG **traits** prefix (matches neighbours); shim replay (card-allowed, Eve verified one hunk; rework adds fresh fail-first for H1/H2 against build candidate); F6 PRD-listed; F7-F11 -> FT-8b; F1/F2 -> follow-up PRD per maintainer.

## Rework (Ivan opus) + closure
- Ivan: 9 items, 0 cargo. Snapshot FT-8a-rework-candidate.patch sha 3543b45b... (13 files +799/-96) BEFORE driver gate-fix.
- Fail-first replay (shim removed, build-candidate runtime): H2 + H1 tests RED for the exact defects; items 4/5 green (pin existing behaviour). Log scratchpad/logs/rework-failfirst-replay.log.
- rustfmt touched hunks: applied 6+1+1, drift left 3+32+1+4=40.
- Closure (Sonnet x9): items 1,2,3,4,5,6,8,9 CLOSED; item 7 OPEN: verbatim_loser_still_gets_its_inbound_links_rewritten 52 lines, sync_report_lists_every_verbatim_folded_collision_loser 51 lines -> accepted LOW (rework cap).
- Gate 1 build pass 3m37s. Gate 2 clippy FAILED: manual_contains at git_ops/tests.rs:2837 (rework helper). Maintainer decision: driver applies clippy's exact suggestion `taken.contains(&c)`; all gates rerun from gate 1.

## mallory: APPROVE, no findings
- new_path never through validate_path (pre-existing, both branches). Fold removes a DoS. Logging ids/paths only.
