# Assumption ledger — PRD 00170-id-throughput-ceiling-decision-v1

## 4: [D1] Correct the ID-format decision's collision and throughput claims, and close the 00201 plan gaps

Ivan (opus, rework cycle 1):

- Kept the 8-char/32-bit discriminator and weakened the claim rather than widening the field, because widening would create a second rendering of node identity that 00201's own "Node discriminator drift" risk forbids.
- Pinned the sub-second width at 6 (microseconds) as the decision, since the dispatch required removing `(guess)` and 6 was the width already drafted with a supporting rationale.
- Chose the deterministic hash fallback plus structured warning over a structured error for malformed `.git/ddb-node`; the dispatch permitted either.
- Assumed the create path's existing structured-warning channel is the right carrier for the node-file warning; could not verify that plumbing reaches the mint from the files in scope, so 00201 states it as a requirement rather than a verified wiring.
- Extended the collision and throughput corrections into the blast-radius note (beyond its listed jink item), because the CRITICAL and HIGH findings reproduced verbatim there and the note is the cited evidence trail for both other documents.
- Corrected `design-decisions.md`'s deciding-factor rationale to match the verified jink evidence; the dispatch scoped that verification to the note, but leaving the two documents contradictory would have failed the "documents agree" criterion.

**Consequence flagged by Ivan, carried forward deliberately:** the decision's recorded deciding
factor was "downstream ID parsing", and the only existing downstream (jink) parses nothing —
it types every doogat id as an opaque `Scalars['ID']`. The chosen shape still stands, but its
rationale now rests on ddb's own surfaces, wikilink legibility, and unknown future downstreams
rather than on a demonstrated jink break. Cycle 2's review should confirm this reframing is
sound rather than a rationalisation.

## PRD 00171 task 12: adversarial field-identity repair

Tess treated leading/trailing Markdown body whitespace as insignificant when
checking that a CLI body update landed in the committed body. The exact payload
and preserved title remain asserted; the check reads git HEAD, not the worktree.

## PRD 00171 task 16: review assertion fidelity

- The new three-junction-row test links categories out of creation-id order (gamma, alpha, beta) so the assertion can distinguish "first-linked" from "numerically smallest id" — the PRD text implied this distinction but didn't specify exact linking order.
- REST cross-process read-back uses `repo.ddb()` (the same `DdbTestRepo` already in scope in that test), matching the finding's suggested snippet exactly.
- `assert_doogat_id`'s generic panic message replaces the more specific per-call messages that existed inline (e.g., "typed fixture must return a 14-digit id") — acceptable since the task explicitly asked to consolidate these into one shared helper.
