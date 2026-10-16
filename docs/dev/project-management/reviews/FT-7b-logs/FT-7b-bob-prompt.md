
Read /Users/bob/git/src/github.com/doogat/ddb/dev/local/tmp/FT-7b-card.md for review context, and /Users/bob/git/src/github.com/doogat/ddb/dev/local/tmp/FT-7b-build-candidate.patch for the full diff.

Read /Users/bob/git/src/github.com/doogat/ddb/dev/local/tmp/FT-7b-no-pack.md and treat its full content as prepended context: similar code, reuse precedent, findings precedent, and task prose for this diff's changed symbols.

Use this review checklist:
# Review Dimensions

Detailed checklist for reviewing completed work.

## Plan Compliance

- [ ] Implementation matches task description
- [ ] All acceptance criteria met
- [ ] No scope creep (extra features not requested)
- [ ] No missing pieces from original task

## PRD Coverage

- [ ] All "must have" requirements addressed
- [ ] Success metrics achievable with implementation
- [ ] No PRD sections left unimplemented
- [ ] Dependencies correctly handled

## Simplification

Hunt actively for simplification — do not just tick boxes. The diff under
review is fresh; this is the cheapest moment to cut complexity before it sets.
For every added or changed file, ask "what would make this simpler to read
without changing what it does?" and flag concrete opportunities.

- [ ] **Reduce complexity** — no needless indirection, dead branches, or
      abstractions built for a single caller; nesting <= 4 levels; functions
      under 50 lines
- [ ] **Eliminate redundancy** — no logic duplicated within the diff or against
      existing code; no helper that reimplements a stdlib or existing utility
- [ ] **Improve naming** — names state intent; no opaque abbreviations;
      action-named functions start with a verb
- [ ] **Follow project standards** — conventions from CLAUDE.md / AGENTS.md and
      the surrounding code; no style drift
- [ ] **No dead code** — no commented-out blocks, unused imports, variables, or
      speculatively-added parameters
- [ ] **Appropriate error handling** — explicit, never silently swallowed

**How to flag a simplification:** give the file:line, the current shape, and
the simpler replacement. Flag concrete behavior-preserving simplifications at
🟡 Medium so the decision gate routes them into the rework loop. Do not inflate
them to 🟠 High; that floods the rework loop and can trip the scope alarm.

**Balance — do not over-simplify:** never propose a change that trades clarity
for brevity, drops error handling, collapses a deliberate boundary, or removes
a documented invariant. Simpler means easier to read and maintain, not shorter
at any cost. If a "simplification" would change behavior, it is out of scope —
do not flag it.

## Testing

- [ ] Unit tests for new logic
- [ ] Edge cases covered
- [ ] Error paths tested
- [ ] Integration tests if crossing boundaries
- [ ] Tests actually run and pass
- [ ] No tautological test — every new or changed test fails against the
      pre-change code (the context's fail-first replay block), and no assert
      is constant, self-comparing, or an either-or hedge (the shapes block)

## Security

- [ ] No hardcoded secrets
- [ ] Input validation at boundaries
- [ ] No SQL/command injection risks
- [ ] Auth/authz correctly applied
- [ ] Sensitive data not logged

## Documentation

- [ ] Public APIs documented
- [ ] Complex logic has comments
- [ ] README updated if needed
- [ ] Breaking changes noted


In addition, work through the numbered rubric:
# Review-Work-Completion Rubric

This rubric defines binary pass/fail criteria for consensus review of completed work. Each rule is numbered and stable for tracking coverage. Reviewers must answer "R{n}: pass|fail" for every rule in their prompt.

## Rules

### Tests

R1: Tests cover every new behavior introduced by the diff.
R2: Tests bind to intent, not just observable behavior; no new or changed test is tautological (passes against the pre-change code, or cannot fail as written).
R3: No skipped or xfail tests mask failures.

### Integration

R4: Changed components integrate with existing callers.

### Security

R6: No hardcoded secrets in the codebase.
R7: All user input is validated and sanitized.
R8: No injection-unsafe queries or commands.

### Domain

R9: Implementation matches PRD feature behavior exactly.
R10: Error handling is explicit and not swallowed.
R11: No debug statements, TODOs, or placeholder markers remain.

### Code Quality

R12: Function sizes respect the 50-line limit.
R13: File sizes respect the 800-line limit.

Review the completed work against PRD requirements. Explore the codebase as needed.

OUTPUT FORMAT IS MANDATORY. Follow exactly:
## Agent Output Format (Single Source of Truth)

Each agent outputs issues in this exact format:

```
[BOB] {emoji} {description} | File: {path or "N/A"} | Task: {id or "general"}
```

**Severity emojis:** 🔴 Critical, 🟠 High, 🟡 Medium, ⚪ Low

**Rules:**
- One issue per line
- Use "N/A" for file if issue is architectural/cross-cutting
- Use "general" for task if issue spans multiple tasks or is a PRD gap
- If zero issues found: `[BOB] ✅ No issues found`

**Examples:**
```
[ALICE] 🔴 SQL injection in query builder | File: src/db/query.ts | Task: 3
[BOB] 🟠 Missing error handling strategy | File: N/A | Task: general
[CARL] 🟡 PRD section 2.3 not implemented | File: N/A | Task: 5
```


PER-RULE VERDICTS ARE MANDATORY. For every rule in the numbered rubric, emit one line:
R{n}: pass   or   R{n}: fail
(one rule per line, no other text on the line, no rationale).

## Sandbox Constraints

You run in a read-only sandbox. Read files with read-only shell commands (`cat`, `sed -n`, `rg`, `ls`); that is how you open /Users/bob/git/src/github.com/doogat/ddb/dev/local/tmp/FT-7b-card.md, /Users/bob/git/src/github.com/doogat/ddb/dev/local/tmp/FT-7b-build-candidate.patch, /Users/bob/git/src/github.com/doogat/ddb/dev/local/tmp/FT-7b-no-pack.md and any source file. You CANNOT execute code, tests, linters, or package managers, and you cannot write.

Perform STATIC analysis only:
- Read code for logical correctness, patterns, naming, structure
- Check for missing imports, dead code, type mismatches
- Review against PRD requirements by reading, not executing
- Trace data flow and control flow by reading source

If a criterion requires runtime verification (e.g. "tests pass", "linter clean"), output:
[BOB] ⚪ Cannot statically verify: {criterion description} | File: N/A | Task: {id}

Do NOT run tests, linters, builds, or package managers. Do NOT report failures from blocked execution. Never report an inability to read a file you were told to read: read it with `cat`.


CODE LOCATION: the change lives only in the uncommitted worktree /Users/bob/git/src/github.com/doogat/ddb/dev/local/tmp/worktrees/00203-ft-7b; read every source file from there. The main checkout /Users/bob/git/src/github.com/doogat/ddb is the pre-change base. Static review only: do not run cargo, ddb or any build/test command; the candidate already passed its gates. Do not modify any file.

SCOPE SOURCE: the parent PRD is /Users/bob/git/src/github.com/doogat/ddb/dev/local/prds/backlog/00203-bundle-import-ref-namespace-v1.md; the card is binding. This item is tests + docs only (runtime shipped and was reviewed in commit 3b27002). Also judge these open points: (1) prune=false has no baseline-red proof (each test stops at its first failure on prune=true); (2) the CLI test derives B's payload id from the one new namespace instead of hashing the bundle; (3) the e2e file is 479 lines and duplicates the FFI fixture across crates; (4) fixture A carries an extra doogat to make a stale merge visible; (5) the retry asserts ancestry, not an exact OID; (6) accuracy of the docs claims in the new docs/src/technical/bundle.md section and the docs/src/technical/ffi.md entry against the runtime in ddb-core/src/bundle/mod.rs and ddb-core/src/ffi/driver.rs.
