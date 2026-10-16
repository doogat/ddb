---
item: FT-7a
date: 2026-09-28
base_sha: a739c292748bcf5e35b5c6340eec7a360ceceb0c
outcome: ready
driver: fresh Claude session (Opus 5.5)
reworks: 1
lanes:
  fanout: APPROVE (rita, cora, grace, toby, trent; security did not auto-arm)
  mallory: ok (run explicitly by driver; 1 MEDIUM)
  eve: ok (fable)
  blake: ok (sonnet, card-only)
  bob: ok (codex gpt-6-sol, read-only sandbox)
  carl: ok (copilot gemini-3.8-flash, 6m18s)
---

# FT-7a fast-track review

## Summary

Review-only run of the complete §1 roster against the build candidate
(`dev/local/tmp/FT-7a-build-candidate.patch`, SHA-256 `09d7869d…16fa5`,
re-verified byte-identical before review). Workflow `wf_ac810dbb-fd1`
(harness stages Prep/Review/Verify; implement and gate skipped because the
candidate was already gated). Reviewers were static-only, pointed at the
worktree; Bob/Carl helpers ran from this Claude driver (no nested-dispatch
markers in env; no guard bypassed).

- CRITICAL/HIGH raised: **0** in every lane. Victor had no candidates.
- The review-fanout security dimension did not arm: its trigger scans only
  `+`/`-` lines for security keywords and the real diff has none. Mallory was
  therefore run explicitly (Agent `autopilot:mallory`) so the lane is not
  omitted.
- Rubric: fanout Trent R1/R12/R13 fail (tests for strict parsing; 64-line
  function; tests.rs size); Bob R1/R2/R12/R13 fail; Carl R12/R13 fail; Blake
  B12/B13 fail; Eve D1-D5 pass.
- One rework (spec `dev/local/tmp/FT-7a-review-rework.md`, fresh Opus Ivan,
  edits only, driver ran all Cargo) addressed every MEDIUM plus cheap LOWs.
- Six independent Sonnet closure checkers (workflow `wf_15e38fbb-603`): all
  **CLOSED**.
- Raw lane output: `FT-7a-logs/review-workflow-result.json`; Bob/Carl raw at
  `FT-7a-logs/FT-7a-{bob,carl}-{output.txt,prompt.md}`.

## Findings and dispositions

| # | Sev | Lanes | Finding | Disposition |
|---|-----|-------|---------|-------------|
| 1 | MEDIUM | rita, toby, bob | No test proves a real import holds the import lease; `let _ =` mutation passed every test | Fixed: `real_import_holds_import_lease_while_blocked_at_merge`; driver mutation `let _lease`→`let _` makes it FAIL; closed |
| 2 | MEDIUM | toby, trent R1 | `parse_ref_lines` strict parsing untested | Fixed: `parse_ref_lines_rejects_malformed_records_and_accepts_full_oids`; closed |
| 3 | MEDIUM | grace, blake, bob, carl | `import_bundle_with_lease_timeout` 64 lines > 50 | Fixed: pure extraction into `fetch_into_namespace` / `clean_namespace` (now 44 lines), lease scope unchanged; closed |
| 4 | MEDIUM | mallory (eve LOW) | Fetch auto-follows bundle tags into `refs/tags/*` outside the namespace, never cleaned | Fixed: `--no-tags` + `bundle_import_never_fetches_advertised_tags`; driver mutation (flag removed) FAILS with `refs/tags/annotated`, `refs/tags/light` leaked; CHANGELOG line; closed |
| 5 | MEDIUM | bob | Successful-import assertion near tests.rs:197 passed on old whole-prefix sweep | Fixed: sibling ref seeded and asserted to survive; closed |
| 6 | LOW | eve, grace | Docs call fetch "non-forced" as the guard; post-merge errors don't name namespace; dead match arm; noun fn name; doc order; long doc line; stale test comments; unused `open_index`; duplicate ref walk; masterless error content unasserted | Fixed (all in one closure check); post-merge failure now `tracing::warn!` naming namespace, error variant unchanged; closed |
| 7 | MEDIUM | trent R13, blake, bob, carl | `bundle/tests.rs` 1,847 lines > 800 | Deferred: split needs a new file outside the approved allowlist; file was already 831 lines at base → PRD 00203 follow-ups |
| 8 | MEDIUM | eve | Cross-process lease test unverified on Windows/macOS nightly | Deferred to CI observation: check `full-validation.yml` windows/macos on the pushed HEAD (headSha, duration, per-job) → PRD 00203 follow-ups |
| 9 | LOW | eve | `errors.md`, `overview.md`, `phase-2-exit.md` still cite `bundle.rs` | Deferred: outside allowlist → PRD 00203 follow-ups |
| 10 | LOW | eve | Merge subject now embeds 64-hex payload id | Accepted (cosmetic, no consumer; merge.rs outside allowlist) |
| 11 | LOW | eve | Windows MAX_PATH exposure from 64-hex namespace | Accepted (card mandates full id; failure is loud) |
| 12 | LOW | eve | Six of eight build-session tests lack recorded baseline-red run | Accepted (static argument: they cannot compile or would fail on base; fail-first mandatory only for the bug regression, which has proof) |
| 13 | LOW | fanout tests | Wrong-OID namespace case "would be rejected by non-forced fetch anyway" | Refuted by Eve's git-fetch(1) citation: git accepts non-ff under refs/remotes without `+`; the pre-fetch check is the real guard. No change |

Mallory also checked and found clean: argv-only git invocation, hex-only
namespace/payload id, cleanup gated on exact advertised-head match, strict
parser, lock path from local git output, no secrets in errors.

## Gates after rework (fresh, driver, Rust 1.89.0, `--locked`, shared target)

Rework snapshot: `dev/local/tmp/FT-7a-rework-candidate.patch` (tree
re-verified identical after the two mutation probes were reverted).

- `test -p ddb-core --lib bundle`: 39 passed, 0 ignored (36 + 3 new).
- Mutation probes (reverted): lease guard dropped → lease test FAILS;
  `--no-tags` removed → tag test FAILS.
- `build`: pass (4m20s). `clippy --workspace --all-targets -D warnings`: pass (6m03s).
- Fast tier (exact `test-ci` alias expansion; literal alias breaks in nested
  worktree): 42 + 1603 + 224 = 1869 passed, 0 ignored
  (`FT-7a-logs/rework-test-ci-expanded.log`).
- `--test sync_test`: 6 passed. `build -p ddb-cli`: pass.
- `ddb-e2e --test e2e bundle`: 5 passed.
- Full `ddb-e2e` (CLAUDE.md replacement-path rule): 569 passed, 0 failed,
  0 ignored, first run (`FT-7a-logs/rework-full-e2e.log`).
- Touched-file rustfmt 1.89.0 `--check`, `git diff --check`, mdbook build: pass.

## Landing

Commit `3b2700229de7858c164523b3f1f701d07d0e99f2` (diff from base equals the
rework snapshot byte-for-byte), fast-forwarded onto master, pushed. Exact-HEAD
CI: Test `36439407100` success (every step ran: dependency policy, clippy, CLI
build, fast tier, e2e, sync; coverage job success), Docs `36439407082` success.
Cross-platform: manually dispatched Full Validation `36443671857` on
headSha `3b2700229de7` succeeded (macos-full 47m, windows-full, linux-core,
linux-e2e, linux-clippy, mark-validated). Job logs show
`bundle_import_lease_covers_fetch_through_cleanup`,
`real_import_holds_import_lease_while_blocked_at_merge` and
`bundle_import_never_fetches_advertised_tags` `... ok` on both macOS and
Windows; core suite 1603/1601 passed, e2e 569, 0 failed on each.
