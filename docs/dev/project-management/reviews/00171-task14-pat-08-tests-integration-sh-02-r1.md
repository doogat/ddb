CLOSURE section not applicable — no `### Findings (verbatim)` block in the task description.

This diff is a pure deletion (tests/integration.sh lines 592-1129 removed in full, segment 2/7 of a 7-way partition). No new logic, no added assertions, no behavior introduced — nothing here can carry a correctness or security defect since nothing is added. The mapping-table claims about target Rust test coverage can't be checked from this prompt (the runner files aren't in scope), so per the "don't invent findings you can't ground here" rule, I'm not flagging unverifiable coverage gaps. No simplification opportunities apply to a deletion-only diff.

NO FINDINGS
