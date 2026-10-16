# Rework self-review

Scope: the two 00171 review fixes only. The required Alice, Blake, Bob, and Eve reviews remain preserved separately in the cycle-1 report.

## Questions and evidence

1. **Least confidence — exact assertion behavior (VERIFY).** JOIN rows are JSON-encoded arrays, consistent with the existing `common::select_scalar` and GraphQL SQL serialization tests. The new reference test deliberately links gamma before lower IDs; `extract_typed_field` returns the first matching inline reference, and its existing unit test pins first-reference selection. Execution of these e2e cases remains with CI.
2. **Assumptions — “already covered” meant equivalent assertions (FIX).** The original mapping audit found six counterexamples. The patch restores explicit counts, exact JOIN values, a prefix-negative equality fixture, cross-process REST read-back, a three-reference scalar assertion, and the numeric singleton ID check. Twelve mapping descriptions (six dialect pairs) now state the actual checks.
3. **Completeness — required lenses and PRD scope (VERIFY / KNOWN).** Consensus, blind, doubt, and the mandatory Fable guard all completed. Bob used the documented Sonnet fallback after nested Codex dispatch was refused; optional Gemini/Carl was unavailable after retry. Only 00171 was queued. Acceptance still requires a fresh cycle 2; this session must not claim the PRD accepted or finalize it.
4. **Practical failures — hidden selection filters and another live session (FIX / VERIFY).** The unfiltered CI step selects all 63 qualified reused smoke targets. The complete inventory still contains exactly 391 original source rows. An isolated worktree protects concurrent changes on master; only two workflow/docs paths and 13 allowed test paths belong to this rework.
5. **Approach — keep the patch smaller (FIX / VERIFY).** A formatter pass touched 52 unrelated files and three unrelated hunks in rest.rs. Those edits were restored and the discarded sweep saved as evidence. A committed-scope check proves the final test commit contains exactly its 13 allowed files; the worktree was clean before final gates.
6. **Validation gaps (KNOWN).** Build and all-target Clippy pass at `74644b8`; the final fast-test gate is in progress when this note is written. The new e2e test bodies are intentionally delegated to CI under the repository's explicit Tier 2 rule. The final rework report records their actual later status; source inspection is not represented as runtime conformance.

## Slop checks

- **Bloat ratio (VERIFY, dismissed):** the test patch adds a net 56 lines for six concrete assertion repairs and shared helpers. Most additions form the requested three-reference fixture; the task contains substantially more acceptance detail than the added code.
- **Repeated fixture setup (VERIFY, dismissed):** the existing category helper creates and links exactly one category before returning. Reusing it would remove the new test's distinction between creation order and first-link order. No new single-caller fixture abstraction was added.
- **Redundant ID predicates (FIX):** ID assertions now call one common helper. The scan asserts the already validated ID occupies its own line; the Git-tree check compares all Markdown stems directly with the validated survivor ID, which also stops hiding unexpected filenames.
- **Defensive impossibility (VERIFY, dismissed):** process success, non-null fields, parsed JSON, and exact row counts are observable database assertions. They are not guards for unreachable application states.
- **Single-caller abstraction (VERIFY, dismissed):** both added common helpers replace multiple existing callers. No general-purpose parsing framework was introduced.
- **Comment paraphrasing (VERIFY):** reference-order and cross-process comments explain why those fixtures distinguish regressions. The new helper doc comments state their public test-harness contract.
- **Framework-only tests (VERIFY, dismissed):** no standalone language/framework tests were added. Assertions exercise actual ddb processes, stored rows, and transport responses.

Second pass: verified the formatter cleanup, helper call sites, unchanged one-reference test, retained server-side REST read-back, and preserved 391 source keys. No additional code correction was required. Remaining limits are fresh CI execution and the mandatory next-session review, not missing implementation.
