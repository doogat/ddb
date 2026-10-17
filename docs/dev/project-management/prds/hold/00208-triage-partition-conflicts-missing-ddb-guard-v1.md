---
catchup: skip
design: skip
ledger: deferred/202607161128-deferred.json
ledger_key: 775c9153eddf
source_prd: 00167-sync-collision-reassignment-atomicity-v1.md
severity: high
---

# Triage: partition-conflicts-missing-ddb-guard

## Problem

Deferred finding with no PRD owner when this stub was minted: batch `202607161128`, ledger `deferred/202607161128-deferred.json`, type `cap-overflow`, topic `partition-conflicts-missing-ddb-guard`, cycle `2`, consensus `1/5`, raised against `00167-sync-collision-reassignment-atomicity-v1.md`.

Issue: partition_conflicts routes EVERY non-reference/, ancestor-less, both-sides-present conflict into the add-add bucket with no ddb/ prefix guard, so a non-doogat file (.gitignore, README.md, .ddb.toml, any docs/*.md) is treated as a doogat ID collision: old_id becomes its file stem and fold_one_loser relocates the loser's bytes to ddb/{14-digit}.md, fabricating a bogus doogat while the real path silently keeps only the winner.

Detail: Raised by Blake (blind lens) alone. Orchestrator-verified against sync_manager/mod.rs:107-117: the partition is exactly `if ours.is_empty() || theirs.is_empty() -> delete_edit; else if path.starts_with("reference/") -> binary_ref; else if ancestor.is_none() -> add_add; else normal` - there is no ddb/ prefix test anywhere in it. Verified PRE-EXISTING and out of 00167 scope: partition_conflicts is untouched by both the cycle-1 and cycle-2 diffs, and the pre-00167 two-commit path relocated a misrouted loser to ddb/{id}.md exactly the same way via reassign_single_loser. 00167 changed only the atomicity of the relocation, not the routing that decides what gets relocated. Deferred as its own backlog PRD (add a ddb/ prefix guard to the add-add arm, route non-doogat add-add conflicts to the normal bucket, regression-test a README.md collision) rather than widening a convergence-phase PRD into the conflict-partitioning logic.

## Solution

Attended triage. A human promotes this finding into a backlog PRD through normal PRD authoring and review, or closes it. Autopilot never drains `docs/dev/project-management/prds/hold/`, and this stub is never auto-promoted.

## Requirements

### Must have
- A triage decision for ledger key `775c9153eddf`: a backlog PRD, or closed.

### Nice to have
- None until triage.

## Implementation

### Module: triage
- **Location**: `docs/dev/project-management/prds/hold/`
- **Responsibility**: hold ledger key `775c9153eddf` from `00167-sync-collision-reassignment-atomicity-v1.md` until a human triages it
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
