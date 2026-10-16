---
catchup: force
default_model: sonnet
model_tier_rationale: verification matrix with a named regression pin; the classifier escalates a REAL-branch fix; catchup forced because 00183 splits dml.rs and helpers.rs
---

# Verify (fix or retract) the zone-default literal-NULL write defect

**Priority:** P2 (suspected silent corruption — UNVERIFIED; small) · **Roadmap:** appended 2026-07-16 · **Depends on:** none.

Source: `dev/local/audit-results/evolution-assessment-2026-07-16.md` F17 (lens 5); jink `AGENTS.md:78`.

## Overview

### Problem Statement

jink documents, from operating a deployed ddb binary: "TEXT and REFERENCES columns default to the wrong zone and silently write literal `NULL`; add `ALTER TABLE t SET ZONE frontmatter FOR <col>` after CREATE TABLE. INTEGER/REAL/BOOLEAN/VARCHAR(N) default correctly." If real at HEAD, this is a silent-corruption class ddb's backlog does not cover. The 2026-07-16 assessment could NOT reproduce it by code reading at 430cb13: every traced SQL NULL path collapses to `""` (`dml.rs:301` INSERT, `dml.rs:903` deferred UPDATE, `helpers.rs:810-811` `expr_to_string` literal, `helpers.rs:715` `extract_default` DDL default — the latter two live in `sql_engine/helpers.rs`, corrected 2026-08-14). Either the defect was fixed since jink observed it (the 00122-era nullable handling, historical PRD since GC'd — link-ok: prose reference), or it triggers via an untraced path — the FFI raw path or CLI/REST typed create; NOT GraphQL `fields` JSON `null` at HEAD (grounded 2026-09-05: `parse_fields_json` deserializes into `BTreeMap<String, String>`, `schema/base_types.rs:22-31`, so a JSON `null` fails at parse before any write). By the time this PRD runs, 00178 has made the object form accept `null` with the decided rule (omit on create, unset on update, never written) — the GraphQL cell of the matrix verifies THAT rule. Update (2026-08-14 backlog review): jink HEAD `7b4cd56` (2026-07-17) moved zone declarations inline into its `TYPE_DEFINITIONS` and deleted the `SET ZONE` migrations, so jink no longer carries the per-table workaround — the gotcha survives only as an AGENTS.md note (near line 101 in jink's working tree). The literal-NULL question itself is still unverified and still worth the matrix.

### Target Users

jink's historical report motivates verification; its current typedefs use inline zones and no longer carry per-table workaround migrations.

### Success Metrics

- A reproduction verdict: defect REAL at HEAD (with the exact code path) or STALE (only when a fixing commit range is proven), otherwise NOT REPRODUCED in the exact tested versions — recorded in this PRD and the findings report.
- If real: fixed at the conversion boundary + a regression test that fails on the old code. If stale: the exact jink `AGENTS.md` correction (the gotcha near line 101, including the fixed-version range) is RECORDED in the findings report for the maintainer to apply jink-side — this session never writes outside the ddb repo — and the behavior is pinned by a conformance case so it cannot regress unnoticed.
- Under every verdict: a `typed_create_null_handling` conformance fixture (typed create with absent and null values per zone) lands in the harness.

## Functional Decomposition

### Capability: Reproduction matrix

#### Feature: Cross-entry-path repro
- **Description**: reproduce (or fail to reproduce) the literal-NULL write across every create entry path, against both the deployed binary and HEAD.
- **Inputs**: a temp-dir repo (NEVER the project root); `CREATE TABLE` with un-zoned TEXT + REFERENCES columns; row creation via SQL INSERT (value absent, and explicit NULL), GraphQL `createDoogat` (field absent, and object-form `null` per 00178's rule), REST typed create, CLI create, FFI structured create (the current public export after 00177), NoSQL HTTP create, and PgWire INSERT; binaries: the latest released build (0.2.7 or newer; record the exact version) and a HEAD build.
- **Outputs**: a matrix of (entry path × value case × binary) → observed `.md` content, checked for literal `NULL`/`"NULL"` text in body/reference zones.
- **Behavior**: every cell observed and recorded; no cell inferred. The matrix note lives in the PRD workdir and its verdict is copied into this PRD.

### Capability: Fix or retract

#### Feature: Verdict-driven resolution
- **Description**: act on the matrix verdict.
- **Inputs**: the matrix verdict (REAL, proven STALE, or bounded NOT REPRODUCED).
- **Outputs**: REAL → boundary fix and regression; proven STALE → proposed jink guidance correction with the demonstrated fixed range; NOT REPRODUCED → bounded evidence report naming tested versions without claiming a fixing commit. No cross-repo edits.
- **Behavior**: execute only the evidence-supported branch: fix a real defect, record a proven historical correction, or report NOT REPRODUCED with tested versions and no invented fixing commit; always add the conformance pin.

#### Feature: Conformance pin
- **Description**: the null/absent-value handling per zone becomes a pinned conformance case.
- **Inputs**: the harness (`tests/e2e/conformance/`), all six drivers supplied by 00193.
- **Outputs**: a `typed_create_null_handling` fixture asserting the written document never contains literal NULL text for absent/null values.
- **Behavior**: fixture runs on available drivers; documents the intended semantics (absent → omitted; explicit null → omitted on create / unset on update, the 00178 rule decided 2026-09-05).

## Structural Decomposition

### Repository Structure

```text
tests/e2e/conformance/
└── workflows.rs (or fixture module)   # Maps to: Conformance pin (typed_create_null_handling)
ddb-core/src/sql_engine/ or server input path  # Maps to: Verdict-driven resolution (only if REAL; exact file from the matrix)
docs/ + the findings report            # Maps to: Verdict-driven resolution (STALE branch records the jink AGENTS.md correction; no cross-repo write)
```

### Module: conformance fixture (tests/e2e/conformance/)
- **Maps to capability**: Fix or retract
- **Responsibility**: assert absent/null typed-create values never write literal NULL text.
- **Exports**: the `typed_create_null_handling` workflow fixture.

### Module: repro workspace (temp dirs only)
- **Maps to capability**: Reproduction matrix
- **Responsibility**: hold the throwaway repos; nothing lands in the source tree (AGENTS.md gotcha: never run `ddb` in the project root).
- **Exports**: none (workdir notes only).

## Dependency Graph

### Foundation Layer (Phase 0)
No dependencies - built first.
- **reproduction matrix**: the verdict everything else branches on.

### Core Layer (Phase 1)
- **verdict-driven resolution**: depends on [matrix].

### Integration Layer (Phase 2)
- **conformance pin + docs**: depends on [resolution].

## Implementation Phases

### Phase 0: Foundation
**Goal**: an observed verdict.

**Tasks**:
- [ ] Run the reproduction matrix in temp repos against HEAD and the latest available released binary, recording exact versions: GraphQL typed create and executeSql, REST typed create, CLI create/SQL, FFI structured create/execute_sql, PgWire INSERT, and NoSQL HTTP create; cross un-zoned TEXT/REFERENCES with absent/present/explicit-null cases supported by each interface (no deps) - Acceptance: every intended cell records actual Markdown or an observed unsupported-input result; old-release rejection is not corruption, equivalent CLI/FFI null input uses documented unset/absence semantics, and verdict is REAL, proven STALE, or NOT REPRODUCED in these versions without an invented fixing range.

**Exit Criteria**: verdict recorded with evidence.

### Phase 1: Core
**Goal**: the verdict acted on.

**Tasks**:
- [ ] Execute the evidence-supported resolution: REAL → boundary fix, failing-before regression, and CHANGELOG; proven STALE → exact proposed jink guidance correction with demonstrated fixed range; NOT REPRODUCED → bounded evidence report and proposed wording without claiming a fixing commit (depends on: Phase 0) - Acceptance: the chosen branch and evidence are recorded; other branches are not applicable; no cross-repo file is edited.

**Exit Criteria**: real defect fixed, historical correction proven, or non-reproduction bounded to tested versions; evidence supports exactly the reported conclusion.

### Phase 2: Integration
**Goal**: pinned forever.

**Tasks**:
- [ ] Add the `typed_create_null_handling` conformance fixture (all six drivers from 00193; absent + explicit-null cases per zone using each interface's supported contract) and document the intended semantics in `docs/src/technical/data-model.md` (depends on: Phase 1) - Acceptance: fixture green on available drivers; docs state the absent/null rule.

**Exit Criteria**: fixture in tree; Tier 1 green.

## Test Strategy

**Validation:** per-task local gates are `cargo build`, `cargo clippy --workspace --all-targets`, and `cargo test-ci`, including colocated unit regressions. Author required e2e/integration/conformance scenarios and executable walkthroughs; their heavy execution belongs to Tier 2 CI, not the per-task gate. Retain only CLAUDE.md's conditional e2e run for Claude-routed tasks deleting/replacing code paths. Walkthrough creation uses showboat; any verification uses `dev/bin/safe-showboat-verify`, never the project root. This backlog assumes 00171 has completed the shell-to-Rust harness migration.

### Critical Scenarios
- **Happy path**: typed create with all values present → document unchanged from today (regression net: existing typed-create suites).
- **Edge case**: value absent for an un-zoned TEXT column → written document contains no literal NULL text; column reads back empty/absent.
- **Error case**: explicit JSON `null` in GraphQL object-form `fields` → the 00178 rule applies (omitted on create / unset on update); never the string "NULL" in the document.

## Risks

- **Running the binary pollutes cwd**: all repro strictly in temp dirs (AGENTS.md gotcha).
- **Deployed-binary availability**: the released binary must come from GitHub releases (0.2.7 or newer at execution); record exactly which version ran in the matrix note.
- **REST/NoSQL legs**: REST typed create (post-00149 `fields`) is included in the matrix; NoSQL HTTP is read-only until 00192 — noted, not exercised.

## Transport Impact

The matrix and standing conformance fixture cover CLI, GraphQL, REST, PgWire, FFI, and NoSQL HTTP using the public interfaces present after 00192/00193; old released binaries record unsupported cells explicitly. Current FFI uses structured CRUD, and SQL-bearing paths include FFI execute_sql. No driver is silently omitted.
