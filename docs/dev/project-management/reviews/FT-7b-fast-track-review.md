---
item: FT-7b
date: 2026-09-29
base_sha: 3b2700229de7858c164523b3f1f701d07d0e99f2
outcome: ready
driver: fresh Claude session (Opus 5.5)
reworks: 1
lanes:
  rita: ok (APPROVE; 1 MEDIUM)
  cora: ok (REQUEST_CHANGES; 2 MEDIUM)
  grace: ok (REQUEST_CHANGES; 3 MEDIUM, 4 LOW)
  toby: ok (APPROVE; 1 MEDIUM)
  trent: ok (R12/R13 fail, LOW)
  mallory: ok (APPROVE; run explicitly, security trigger not armed)
  victor: no candidates (0 CRITICAL/HIGH in every lane)
  eve: ok (fable; D1-D5 pass; R3/R4 fail LOW)
  blake: ok (card only; R12/R13 fail)
  bob: ok (codex via autopilot 0.5.6 codex-run.sh)
  carl: ok (gemini via autopilot 0.5.6 gemini-run.sh)
---

# FT-7b fast-track review

## Summary

Review of the build candidate `dev/local/tmp/FT-7b-build-candidate.patch`
(SHA-256 `ac5f9350…12ddc`). Before review it was re-verified byte-identical to
the live worktree diff: 5 files, +982/-1, no runtime file. Master,
origin/master and ls-remote were all at 3b27002.

The roster was run by hand with the Agent tool, following the plan §1 manual
fallback. The harness refuses a dirty tree and re-implements, so it did not
fit a review of an existing candidate.

- Fan-out lanes: Rita, Cora, Grace, Toby, Trent and Mallory.
- Doubt and blind lanes: Eve (doubt) and Blake (card only, no diff).
- External lanes: Bob and Carl through the plugin helpers, run as background Bash.

All ten lanes returned. Every prompt and raw output is in `FT-7b-logs/` for
Bob and Carl; the Agent lanes' reports are summarised below.

- CRITICAL/HIGH raised: **0** in every lane, so Victor had no candidates.
- One rework: a fresh Opus Ivan, spec `dev/local/tmp/FT-7b-review-rework.md`,
  edits only; the driver ran all Cargo. It covered every in-allowlist MEDIUM
  plus the cheap LOWs.
- Eight independent Sonnet closure checkers, one per rework item (workflow
  `wf_20ca1a77-515`): all **CLOSED**.
- Rework snapshot: `dev/local/tmp/FT-7b-rework-candidate.patch` (SHA-256
  `d31d8dfc…bcddf`). 5 files, +1173/-1, still no runtime file.

## Findings and dispositions

| # | Sev | Lanes | Finding | Disposition |
|---|-----|-------|---------|-------------|
| 1 | MEDIUM | rita, cora, bob, eve | The bundle.md conformance table said every merge failure lists the delivered branches. Only the masterless refusal does (mod.rs:253-258 vs 297-300). | Fixed (rework item 1); closed |
| 2 | MEDIUM | bob | ffi.md said "a failed import keeps its refs", which is too broad. | Fixed: it now separates failures before and after the fetch, and a partial cleanup (item 2); closed |
| 3 | MEDIUM | toby, eve (LOW) | The retry was not tied to A's namespace ref: the foreign refs pointed at `a_master`, so merging them passed. Only ancestry was asserted. | Fixed: foreign refs now point at a parentless throwaway commit. The retry asserts HEAD parents == {pre-retry HEAD, a_master} and SIDECAR absent. The CLI retry pins `conflicts resolved: 0` (item 3); closed |
| 4 | MEDIUM | cora; toby/rita/carl/eve/blake LOW | prune=false had no baseline-red proof, and it is the setting where the old flat stale ref survives. | Fixed: loop order is now `[false, true]`, and the legacy flat ref is planted before masterless B (item 4); closed. **Fresh replay on a739c29: FFI 3/3 and CLI 3/3 red at `prune=false` on the masterless-B step** (old runtime returned `Ok(())` / `imported: conflicts resolved: 0`). Logs: `rework-baseline-{ffi,cli}-prune-false.log`. Replay-only note: the new exact-namespace setup check (#5) fails first on the old runtime for a ref-name reason, which the card says is not stale-ref evidence. It was relaxed to the old OID-only check in the uncommitted baseline copy only. |
| 5 | MEDIUM | bob | Failed A's refs were checked only by OID. | Fixed: exact ref-name and OID set via `kept_a_refs` (item 5); closed |
| 6 | MEDIUM | grace, carl, bob, trent, blake | Several functions ran past 50 lines (retry tests ~75, fixtures ~56-60, masterless ~51-55). | Fixed: helpers extracted; the longest function is now 47 lines (item 6); closed |
| 7 | MEDIUM | grace, carl | Twin drift between the FFI and CLI fixtures (64-hex check, repeated namespace derivation). | Fixed: FFI 64-hex check, header twin notes, e2e `a_payload_id` (item 7); closed |
| 8 | LOW | grace, eve | A mid-file `use` block, a redundant typed binding, an unclear doc comment, and a 40\|64 OID guard. The e2e file did not use `MultiNodeSetup::read`. | Fixed (item 8); closed |
| 9 | MEDIUM | eve (VERIFY) | Is the prune axis observable on the new runtime? | Driver mutation probe: removing `--no-prune` at bundle/mod.rs:227 leaves the FFI tests 3/3 green, then reverted (hash re-verified). The axis is unobservable on the new runtime, because the per-payload refspec means pruning can touch only that payload's own refs. The loop stays as the card-mandated no-regression guard; the old runtime is red at prune=false (#4). |
| 10 | MEDIUM | grace, trent, blake, bob, carl, eve | `ffi/tests.rs` is 1,667 lines (e2e module 574) (1,085 at base, already over 800). | Deferred: a split needs a new file outside the allowlist, so it goes to PRD 00203 follow-ups together with the `bundle/tests.rs` split. |
| 11 | MEDIUM | grace, carl, eve | The fixture is duplicated between the ddb-core FFI tests and the e2e crate. | Deferred: sharing needs a ddb-core test-support surface outside scope. Twin header notes were added. Goes to PRD 00203 follow-ups. |
| 12 | MEDIUM | rita, cora, toby, bob, carl, eve, blake (LOW) | The CLI takes B's payload id from the new namespace instead of hashing `objects.bundle`. | Accepted/deferred: hashing needs sha2/tar dev-deps in tests/Cargo.toml, which is outside the allowlist and changes Cargo.lock. The SHA-256 link is pinned by the FFI test and `bundle/tests.rs` through the same core function, and CLI and FFI share that function. |
| 13 | LOW | eve | The e2e `clone` sets `commit.gpgsign false`; the FFI twin does not. | Accepted (harmless no-op, mirrors `DdbTestRepo::init`). |

Mallory found nothing: argv-only git, no secrets, temp dirs only, and the doc
claims hold against the runtime.

Blake's R2 concern (the fail-first record was not visible to the blind lens)
is answered by the build replay plus the fresh prune=false replay above.

## Gates after rework (fresh, driver, Rust 1.89.0, `--locked`, shared target)

rustfmt 1.89.0 then changed one layout-only hunk in each test file. Every
gate below ran on the formatted tree, which is the snapshot above.

- `build`: pass. `clippy --workspace --all-targets -- -D warnings`: pass
  (7m11s full, then an incremental rerun after the format).
- Fast tier (exact `test-ci` expansion): 42 + 1606 + 224 = **1872 passed**,
  0 failed, 0 ignored (`FT-7b-logs/rework-test-ci-expanded.log`).
- `test -p ddb-core --lib bundle`: 42 passed (includes the 3 FFI tests).
- `--test sync_test`: 6 passed. `build -p ddb-cli`: pass.
- Full `ddb-e2e`: **572 passed**, 0 failed, 0 ignored, first run. It includes
  the 3 new scenarios, `bundle_import_recovers_real_conflict_and_reimport_is_noop`
  and `integration_28_delta_bundle_target_export`
  (`FT-7b-logs/rework-full-e2e.log`).
- rustfmt `--check` on both test files, `git diff --check` and mdbook build
  (to the scratchpad): pass.
- Walkthrough: not re-verified. Neither the runtime nor the CLI changed, and
  the rework touched only tests and docs. The build session's
  safe-showboat-verify result (1 passed, twice) stands.
