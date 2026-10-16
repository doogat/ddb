[llama-cpp] failed to reach http://localhost:8080/v1/models: fetch failed
Both files verified against the spec. Report:

## Result

Applied the exact specified diffs to both workflow files. No other lines touched.

**`.github/workflows/test.yml`**
- Replaced the `Smoke test` step (`./tests/smoke.sh`) with `Smoke test (e2e)` running `cargo test -p ddb-e2e --test e2e smoke_`, same `DDB_BIN` env, same position (after `Build CLI binary` / `Fast cargo test tier`, before `Sync integration tests`).
- The five hand-picked `E2E - *` steps, `Sync integration tests`, and the `coverage` job are untouched.

**`.github/workflows/full-validation.yml`**
- Deleted the entire `linux-smoke` job (its only content was `./tests/integration.sh`, a strict subset of `linux-e2e`'s unfiltered `cargo test -p ddb-e2e`).
- Removed `linux-smoke` from `mark-validated`'s `needs:` list (now: linux-clippy, linux-core, linux-e2e, macos-full, windows-full).
- Deleted the `Integration test` step from `macos-full` (`./tests/integration.sh`); kept `Build CLI binary` and `Full cargo test suite` (`cargo test-full` = `cargo test --workspace`, which carries the ported coverage unfiltered).
- Deleted the `Integration test` step from `windows-full` (including its `timeout-minutes: 15`, `env`, and `pwsh -File tests/integration.ps1` run); kept `Build CLI binary` and `Full cargo test suite`.
- `linux-clippy`, `linux-core`, `linux-e2e`, and `mark-validated`'s status-posting step are untouched.

Net: 3 CI steps removed (linux-smoke integration, macos integration, windows integration), 1 replaced (test.yml smoke → e2e filter).

**CI dispatch:** NOT performed — per the dispatch instructions, the parent handles commits, pushes, CI dispatch, and waits; I made no git/GitHub/cargo/statectl calls. The manual `full-validation` dispatch and green confirmation on linux-e2e / macos-full / windows-full (with shell steps absent) is the parent's next step, and shell-file deletion remains gated on that.

ASSUMPTIONS: none — the diffs were provided verbatim and applied exactly as written.

FILES_TOUCHED:
- .github/workflows/test.yml
- .github/workflows/full-validation.yml
