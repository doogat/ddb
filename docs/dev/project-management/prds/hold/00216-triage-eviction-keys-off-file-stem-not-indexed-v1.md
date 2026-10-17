---
catchup: skip
design: skip
ledger: deferred/202607161128-deferred.json
ledger_key: 0c428c3b3d77
source_prd: 00169-poison-file-reindex-resilience-v1.md
severity: high
---

# Triage: eviction-keys-off-file-stem-not-indexed-identity

## Problem

Deferred finding with no PRD owner when this stub was minted: batch `202607161128`, ledger `deferred/202607161128-deferred.json`, type `cap-overflow`, topic `eviction-keys-off-file-stem-not-indexed-identity`, cycle `2`, consensus `1/5`, raised against `00169-poison-file-reindex-resilience-v1.md`.

Issue: Poison eviction derives the ID from the filename, but supported frontmatter IDs can differ; the stale row then survives, while a newly added poison filename can evict an unrelated row with that ID.

Detail: Raised by Bob (codex) at HIGH; Eve independently classed the identical mechanism KNOWN and defended it; Alice and Carl did not raise it. Orchestrator-verified by reading the code: the new poison-eviction filter (ddb-core/src/indexer/rebuild.rs:112) calls crate::parser::extract_id_from_path, which is a bare Path::file_stem (ddb-core/src/parser/mod.rs:972-977). The PRE-EXISTING delete leg (rebuild.rs:190) derives its IDs through the exact same call. So the new code inherits the repo's long-standing filename-is-identity model rather than introducing a new one - the finding is real in shape but pre-existing in class, which is why it was deferred rather than reworked at the cap. Reachability: it needs a hand-authored ddb/{stem}.md whose frontmatter id differs from its stem; every ddb write path names the file after the id. Consequences are bounded - the index is a derived cache with git as source of truth, and a full rebuild self-heals both the stale-survivor and the mis-evicted-row cases. Wants its own follow-up PRD that fixes BOTH legs together by capturing each changed path's existing indexed (id, type) before parsing and evicting only that captured row; patching the poison leg alone would leave the two legs disagreeing about identity, which is worse than the current consistent-but-wrong model.

## Solution

Attended triage. A human promotes this finding into a backlog PRD through normal PRD authoring and review, or closes it. Autopilot never drains `docs/dev/project-management/prds/hold/`, and this stub is never auto-promoted.

## Requirements

### Must have
- A triage decision for ledger key `0c428c3b3d77`: a backlog PRD, or closed.

### Nice to have
- None until triage.

## Implementation

### Module: triage
- **Location**: `docs/dev/project-management/prds/hold/`
- **Responsibility**: hold ledger key `0c428c3b3d77` from `00169-poison-file-reindex-resilience-v1.md` until a human triages it
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
