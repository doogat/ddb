---
catchup: skip
design: skip
ledger: deferred/202607161128-deferred.json
ledger_key: 249a62ad1ddf
source_prd: 00169-poison-file-reindex-resilience-v1.md
severity: high
---

# Triage: frontmatter-cap-bounds-parse-not-read

## Problem

Deferred finding with no PRD owner when this stub was minted: batch `202607161128`, ledger `deferred/202607161128-deferred.json`, type `settled-deferral`, topic `frontmatter-cap-bounds-parse-not-read`, cycle `1`, consensus `3/5`, raised against `00169-poison-file-reindex-resilience-v1.md`.

Issue: MAX_FRONTMATTER_BYTES bounds serde_yaml parse cost but not read cost: git2 has already materialized the whole blob and split_zones has already built a line vector before parse_frontmatter's cap can reject it. The PRD success metric 'reindex cost stays bounded' is therefore met on the CPU/parse dimension only.

Detail: Raised by Bob at HIGH, Alice at MEDIUM, and placed in KNOWN by Eve, who identified the reason it cannot be fixed cheaply: the blob is fully in memory from git2 before any ddb code sees it, so a parse-boundary cap structurally cannot bound read cost. The implementor flagged this as an open question during task 10 and it was never resolved either way - deferring it now is the first time it has been decided rather than left hanging. A real fix needs a streaming blob reader and is its own PRD.

## Solution

Attended triage. A human promotes this finding into a backlog PRD through normal PRD authoring and review, or closes it. Autopilot never drains `docs/dev/project-management/prds/hold/`, and this stub is never auto-promoted.

## Requirements

### Must have
- A triage decision for ledger key `249a62ad1ddf`: a backlog PRD, or closed.

### Nice to have
- None until triage.

## Implementation

### Module: triage
- **Location**: `docs/dev/project-management/prds/hold/`
- **Responsibility**: hold ledger key `249a62ad1ddf` from `00169-poison-file-reindex-resilience-v1.md` until a human triages it
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
