# Corrected native Eve review

Actual reviewer: autopilot:eve, Claude Fable 5.1. Fresh isolated input; original combined PRD/design input superseded. Host session: 2d3dab3d-2726-4cc3-8044-f8be2cc9ca24; native agent: a7f49e5524c242a73. One successful dispatch, no fallback. The host saw an incidental design heading while extracting mechanical blocks, but did not forward design text to Eve. The exact native response below was recovered from the subagent transcript because the host final response summarized it rather than returning it verbatim.

Closure verification is done; here is the review.

## Closure of the 8 cycle-1 findings

1. HIGH per-push coverage: resolved in `/Users/bob/git/src/github.com/doogat/ddb/.github/workflows/test.yml:45-46` (`cargo test -p ddb-e2e`, `--exact` steps and `DDB_BIN` gone) and `AGENTS.md:116`. CI evidence: 566 passed per push. Residual: see FIX 1.
2. §42 totalCount: resolved (`serve.rs:1411`, `:1447`; id-eq/nonexistent cases already asserted it at `:1354`, `:1507`). Matches shell `jq -e '.totalCount == 1'`.
3. §44.E1: resolved, exact `["a","1"]` parse (`integration_junction_sync.rs:37-42`), stronger than the shell's escaped-substring grep.
4. §59.A: resolved, CLI `ddb query` read-back (`rest.rs:160-167`), same shape as shell line 2677.
5. §18d2: resolved. Fixture now exact / `/page` prefix / other.org, 1 hit + totalCount 1 (`serve.rs:1821-1860`), same discriminating fixture as shell lines 337-347.
6. §38b2b: partly resolved. Scalar pin with 3 junction rows restored and strengthened (out-of-id-order links). The shell asserted the object resolver in the same setup (line 1266 `"label":"cherry"`); the new test queries only `category_id`. See FIX 2.
7. §17.J2: resolved (`assert_doogat_id`, stronger than shell `[0-9]+`). Same weak shape survives two functions down; see FIX 3.
8. Helpers: resolved for every 00171-touched module; only `common.rs` defines `stdout`/`assert_doogat_id`. Remaining inline 14-digit checks are all in untouched modules (ledger-settled).

Regression scan of the diff: the removed 14-digit filter in `integration_singleton_races.rs:105-108` is safe and stricter (`ls-tree HEAD:ddb` is non-recursive; `_typedef` has no `.md` suffix). No assertion in the diff would still pass against the pre-change weaker behaviour: each one either tightens a count, parses a row exactly, or adds an independent process read-back.

```
FIX:
- `DDB_BIN` env still set on the linux-e2e step and read by nothing (`ddb_bin()` only honours `CARGO_BIN_EXE_ddb`). The cycle-2 ledger dismissed this on "rg returns zero hits", but that grep skipped the hidden `.github/` dir; `rg --hidden DDB_BIN` finds it at HEAD. Settled reason no longer holds. — /Users/bob/git/src/github.com/doogat/ddb/.github/workflows/full-validation.yml:60-61 — delete the two `env:`/`DDB_BIN:` lines (same edit task 15 made in test.yml).
- §38b2b multi-junction-row port pins only the `category_id` scalar; shell line 1266 also pinned the singular object resolver (`rlcat { id label }` == first-linked) in the same 3-row setup, so object/scalar agreement under multiple junction rows is unpinned — /Users/bob/git/src/github.com/doogat/ddb/tests/e2e/reference_raw_id.rs:350-360 — query `{ bookmarks { items { category_id category { id label } } } }` and assert `category.id == category_ids[2]` and `category.label == "gamma"`.
- Same weakening finding 7 fixed in §17.J2 survives in §18z8: shell line 852 asserted a numeric message for the membership INSERT; Rust keeps `assert!(!category_id.is_empty())` / `assert!(!membership_id.is_empty())` (only indirectly pinned by the later id equality) — /Users/bob/git/src/github.com/doogat/ddb/tests/e2e/integration_graphql_jink.rs:204,234 — replace both with `assert_doogat_id(&...)` (helper already imported in this file).
- Three `sleep(1)` calls (one after the final insert) add 3 s wall time for nothing: sequential mints in one process are already distinct and ascending through the repo-aware mint path, and the test's discriminator is "gamma is not the smallest id", which holds without sleeps — /Users/bob/git/src/github.com/doogat/ddb/tests/e2e/reference_raw_id.rs:322 — drop the sleep (at minimum the trailing iteration's).
- Redundant `.stdout(predicate::str::contains(id3.clone()))` immediately followed by the exact whole-line check `line == id3`, which subsumes it — /Users/bob/git/src/github.com/doogat/ddb/tests/e2e/integration_nosql_cli.rs:81,86 — drop the `contains` predicate (and the `.clone()`).
- Newly published shared helper is named `stdout`, a noun, against the action-verb naming rule; it was tolerable as a private per-module helper, less so as a `pub` API in `common.rs` — /Users/bob/git/src/github.com/doogat/ddb/tests/e2e/common.rs:376-383 — rename to `run_stdout` (3 call-site modules: smoke_core, smoke_search, smoke_sql).
VERIFY:
- The new §38b2b test pins "first-linked" as a contract, but the resolver simply returns the first `Zone::Reference` inline field in the doogat's frontmatter (`ddb-server/src/schema/base_types.rs:440-446`), i.e. junction-INSERT append order; if that order is undocumented the test pins an implementation accident — run `rg -n "first|order" /Users/bob/git/src/github.com/doogat/ddb/docs/src/technical/data-model.md /Users/bob/git/src/github.com/doogat/ddb/docs/src/technical/indexer.md` for a stated rule that the singular `<ref>_id` scalar resolves to the first reference entry; if none, add that one sentence to data-model.md so the test and docs agree.
KNOWN:
- Nightly run 34020494801 (SHA c8b72bd, macOS) failed `integration_58_a_graphql_update_warnings_empty` with `ConnectionReset` at `tests/e2e/common.rs:186` (the `.send().expect("request failed")` in `graphql_with_vars`). No cause was established; no retry, fix, skip, or reduced parallelism was made or is claimed. The green runs at 876a153 (34086467208, 34092612666) are re-execution evidence, not proof of a repair. A plausible but unconfirmed contributor is that every request builds a fresh `reqwest::blocking::Client` (new TCP connection per call) under a fully parallel 566-test run on macOS's small ephemeral-port range — out of scope: 00171 is a scenario migration; the helper's transport internals are reserved for 00188, and a flake-hardening change needs its own reproduction first.
```

```
D1: pass
D2: pass
D3: pass
D4: pass
D5: pass
```

Findings: 8 total = 6 FIX + 1 VERIFY + 1 KNOWN. No files modified; no cargo run (per instruction). Scratch copy of the deleted shell script used for comparison is at `/Users/bob/.local/tmp/claude-dev/00171-c2-integration-sh.sh` (safe to delete).
