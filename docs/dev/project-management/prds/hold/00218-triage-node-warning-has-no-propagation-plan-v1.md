---
catchup: skip
design: skip
ledger: deferred/202607161128-deferred.json
ledger_key: adf87cdeb568
source_prd: 00170-id-throughput-ceiling-decision-v1.md
severity: high
---

# Triage: node-warning-has-no-propagation-plan

## Problem

Deferred finding with no PRD owner when this stub was minted: batch `202607161128`, ledger `deferred/202607161128-deferred.json`, type `cap-overflow`, topic `node-warning-has-no-propagation-plan`, cycle `2`, consensus `1/4`, raised against `00170-id-throughput-ceiling-decision-v1.md`.

Issue: The promised structured malformed-node warning has no propagation plan: DoogatService::unique_id and SqlEngine::unique_ids return IDs without warnings, and several create paths do not return AppOutput

Detail: Raised by Bob (codex, doubt lens) at HIGH. Orchestrator-verified against source: DoogatService::unique_id (ddb-core/src/service/write_helpers.rs:244-247) returns Result<DoogatId> and SqlEngine::unique_ids (ddb-core/src/sql_engine/mod.rs:126-150) returns Result<Vec<DoogatId>>; neither carries a warning channel of any kind. PRD 00201:56 promises to 'surface a structured warning naming .git/ddb-node on the create path's existing warning channel' but names no channel, defines no AppWarning code or result shape, and adds no task threading it out to the service, SQL, raw-create or the six transports. As written the warning has nowhere to go, so the Success Metric at 00201:25 that depends on it cannot be met. Same regression-against-the-rework class as the entry above, and the same materiality: a plan-document gap in a held PRD, nothing shipped. FOURTH-CONSECUTIVE-PRD PATTERN: this is the same shape as the 00167 prd-transport-impact-overclaim, 00168 ffi-import-bundle-discards-syncreport and 00169 skip-warning-never-reaches-read-verbs entries above - a PRD asserting a warning/transport surface that does not exist. It now recurs INSIDE a rework that was itself correcting overclaims. This strongly reinforces the standing batch-end request for ONE decision about how Transport Impact and warning-carrying acceptance criteria are authored. Note the fix here is partly blocked on the same backlog work: routing warnings out of read/write verbs is 00175/00176's subject.

## Solution

Attended triage. A human promotes this finding into a backlog PRD through normal PRD authoring and review, or closes it. Autopilot never drains `docs/dev/project-management/prds/hold/`, and this stub is never auto-promoted.

## Requirements

### Must have
- A triage decision for ledger key `adf87cdeb568`: a backlog PRD, or closed.

### Nice to have
- None until triage.

## Implementation

### Module: triage
- **Location**: `docs/dev/project-management/prds/hold/`
- **Responsibility**: hold ledger key `adf87cdeb568` from `00170-id-throughput-ceiling-decision-v1.md` until a human triages it
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
