---
catchup: skip
design: skip
ledger: deferred/202607161128-deferred.json
ledger_key: 157876a05a6a
source_prd: 00167-sync-collision-reassignment-atomicity-v1.md
severity: high
---

# Triage: malformed-loser-wedges-sync

## Problem

Deferred finding with no PRD owner when this stub was minted: batch `202607161128`, ledger `deferred/202607161128-deferred.json`, type `cap-overflow`, topic `malformed-loser-wedges-sync`, cycle `2`, consensus `2/5`, raised against `00167-sync-collision-reassignment-atomicity-v1.md`.

Issue: Unrecoverable sync deadlock: any add-add conflict whose losing content has no YAML frontmatter makes rewrite_id_field -> parser::find_frontmatter return Parse("no frontmatter opening ---"), fold_one_loser aborts, and commit_merge commits nothing. The next sync recomputes the identical merge and fails identically - no --skip, no --force, no documented recovery. Pre-00167 the winner commit landed so sync converged (lossily); now it never converges.

Detail: Raised by Blake at HIGH and independently by Alice at MEDIUM; Eve classed the same behaviour KNOWN and defended it. Orchestrator-verified as real code shape: merge.rs:191-197 wraps the rewrite_id_field error and propagates with ?, aborting commit_merge before its single create_commit, so nothing lands and the next sync recomputes the same conflict. This is a NEW availability regression introduced by this cycle's own task-21 hardening - it is the deliberate cost of the atomic-abort ruling, which both Eve and Pat independently judged correct on data-safety grounds (skipping a bad loser silently DELETES its content, since a loser only survives by being reassigned). Reachability is narrow: it needs a hand-authored ddb/{id}.md with no frontmatter block created independently on two devices under the same id; ddb's own writes always emit frontmatter. Alice proposes a third option neither abort nor data loss: on rewrite_id_field failure, fold the loser at its derived path with content byte-unchanged plus a warn - coherent precisely because the failing case HAS no frontmatter id to be left stale. Deferred at the rework cap rather than deflated; wants its own follow-up PRD to pick between (a) Alice's byte-unchanged fold, (b) a documented operator recovery path, or (c) accepting the wedge.

## Solution

Attended triage. A human promotes this finding into a backlog PRD through normal PRD authoring and review, or closes it. Autopilot never drains `docs/dev/project-management/prds/hold/`, and this stub is never auto-promoted.

## Requirements

### Must have
- A triage decision for ledger key `157876a05a6a`: a backlog PRD, or closed.

### Nice to have
- None until triage.

## Implementation

### Module: triage
- **Location**: `docs/dev/project-management/prds/hold/`
- **Responsibility**: hold ledger key `157876a05a6a` from `00167-sync-collision-reassignment-atomicity-v1.md` until a human triages it
- **Exports**: none

### Dependencies
- triage: No dependencies (foundation)

## Tasks

### Phase 0: Foundation
- [ ] triage: promote to backlog or close - Acceptance: this file is no longer under docs/dev/project-management/prds/hold/

### Phase 1: Core
No implementation tasks until attended triage.

## Success Criteria

- This file is no longer under `docs/dev/project-management/prds/hold/`.
