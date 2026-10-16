CODE LOCATION: the change lives only as an uncommitted diff in the worktree /Users/bob/git/src/github.com/doogat/ddb/dev/local/tmp/worktrees/00204-ft-8a (branch fast-track/ft-8a, base a3e2dbc). Read every changed source file from there. The full diff is /Users/bob/git/src/github.com/doogat/ddb/dev/local/tmp/FT-8a-build-candidate.patch (13 files, +618/-72). The main checkout /Users/bob/git/src/github.com/doogat/ddb is the pre-change base. Static review only: do not run cargo, ddb, git write commands or any build/test command; the candidate already passed its gates (build, strict clippy at 1.89.0, fast tier 1884, filtered core 53, sync_test 6, e2e collision 4 + bundle 8, full e2e 572/572). Do not modify any file.

CARD (binding scope and acceptance): /Users/bob/git/src/github.com/doogat/ddb/dev/local/tmp/FT-8a-card.md
PARENT PRD (Phases 0-1 only; Phase 2 renders/docs/e2e/walkthrough belong to a later item FT-8b and are OUT of scope here): /Users/bob/git/src/github.com/doogat/ddb/dev/local/prds/backlog/00204-sync-collision-loser-without-frontmatter-v1.md
Build evidence (fail-first replay, gates): /Users/bob/git/src/github.com/doogat/ddb/dev/local/reviews/FT-8a-build.md
Invariants: /Users/bob/git/src/github.com/doogat/ddb/docs/src/technical/invariants.md and AGENTS.md "Data-safety invariants".

Judge these open points explicitly, in addition to your normal lens:
1. Card-time drift: PRD 00204 Phase 1's last task says to re-point the bundle tests so the frontmatter-less import SUCCEEDS. The card instead moved FT-7's shared failing-import fixtures (6 bundle tests, 3 FFI tests, 3 CLI e2e tests) to a YAML-invalid loser (still aborts via the same `bundle merge failed:` path) and added ONE new bundle test asserting a frontmatter-less import succeeds with one verbatim fold. Is that a faithful reading of the PRD, and is any coverage lost?
2. The predicate `has_frontmatter` is `find_frontmatter(..).is_ok()`. `find_frontmatter` finds ANY `---` line (trimmed) and a later one, not `---` on line 0. So a frontmatter-less body with two horizontal rules (`---`) counts as "has frontmatter" and goes to `rewrite_id_field` (rewritten or aborted), not folded. Is that correct/acceptable per the card ("same rule split_zones uses"), or a gap that leaves some wedge inputs?
3. `sync_manager::create_conflict_commit`'s return type changed and its call site; wider than the card's cited lines. Acceptable?
4. The fail-first replay used a one-token shim (`if true || has_frontmatter(..)`) in a baseline copy, not the untouched a3e2dbc binary.
5. 40 pre-existing rustfmt drift hunks were deliberately left alone (only touched hunks formatted).
6. Walkthrough 00203 now diverges (it records the old frontmatter-less failure); FT-8b owns re-recording it.
7. Data-safety: everything must stay inside `with_write_lock` and the single `create_commit`; kept-verbatim records exist only after `create_commit` succeeds; `taken_ids` updated on both branches; pure function of inputs.
