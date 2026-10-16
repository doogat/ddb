You are an independent closure checker for ONE rework item of fast-track item FT-8a. You have no other context.

Inputs:
- Rework spec: /Users/bob/git/src/github.com/doogat/ddb/dev/local/tmp/FT-8a-review-rework.md (read only your item).
- Full candidate diff after rework vs base a3e2dbc: /Users/bob/git/src/github.com/doogat/ddb/dev/local/tmp/FT-8a-rework-candidate.patch
- Pre-rework candidate diff (for comparison): /Users/bob/git/src/github.com/doogat/ddb/dev/local/tmp/FT-8a-build-candidate.patch
- Live code: worktree /Users/bob/git/src/github.com/doogat/ddb/dev/local/tmp/worktrees/00204-ft-8a
- Binding card (constraints still apply): /Users/bob/git/src/github.com/doogat/ddb/dev/local/tmp/FT-8a-card.md

Rules: read-only. Do NOT run cargo, ddb or any mutating command; do not edit files. rg, reading files and read-only git are fine. Do not claim anything compiles or passes; the driver runs the gates separately.

Decide whether your item is CLOSED: the fix does what the item asks, the named test exists and would fail if the fix were reverted (bind to intent, not tautological), no card constraint is broken (has_frontmatter only evaluated in fold_one_loser; all within with_write_lock and the single create_commit; no new skip/force/degrade path; no out-of-allowlist file), and no existing assertion was weakened or dropped. Also flag any NEW defect the rework introduced in the lines it touched.

Output: first line `CLOSED` or `OPEN`, then at most 10 lines of evidence with file:line. If OPEN, say exactly what is missing.

Your item: ITEM_NUMBER
