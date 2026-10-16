# Devon adversarial validation — task 12, round 2

Verdict: **the repaired assertions catch the prior body-to-title mutant; a new SQL value-corruption mutant passes every provided test**. This is a bounded result for these experiments, not a universal claim about robustness.

The first-round report remains unchanged at `dev/local/reviews/00171-task12-devon.md`. Only the supplied updated tests, allowed harness interfaces, and scratch experiment material were used. No production source, specifications, architecture docs, unrelated commits, test edits, hooks/config edits, commits, or autopilot-state mutations.

All runs used the existing `target/debug/ddb` executable and exactly the required five-test scope with the ordinary parallel runner:

```sh
cargo test -p ddb-e2e --test e2e -- integration_singleton_races integration_singleton_parity integration_write_lock_races
```

Localhost tests ran outside the sandbox. Cargo ran exclusively in the foreground, with no concurrent Cargo commands from this validator.

## Baseline

Command: the unmodified command above.

Exit: **0**. Actual output:

```text
   Compiling ddb-core v0.2.7 (/Users/bob/git/src/github.com/doogat/ddb/ddb-core)
   Compiling ddb-e2e v0.2.7 (/Users/bob/git/src/github.com/doogat/ddb/tests)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 37.25s
     Running e2e/main.rs (target/debug/deps/e2e-851e240f61a32247)

running 5 tests
test integration_singleton_races::integration_55_b_graphql_execute_sql_singleton_insert_race ... ok
test integration_singleton_races::integration_55_d_cli_vs_pgwire_singleton_insert_race ... ok
test integration_singleton_races::integration_55_c_graphql_create_doogat_singleton_insert_race ... ok
test integration_singleton_parity::integration_54_d_cross_protocol_singleton_duplicate_parity ... ok
test integration_write_lock_races::integration_61_b_concurrent_cli_and_graphql_write_lock ... ok

test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 560 filtered out; finished in 8.93s
```

## Mutation 1: repeat the body-to-title exploit

As in round 1, every CLI `update --body VALUE` was rewritten to `update --title VALUE`, forwarding every other argument unchanged. The implementation is the same array rewrite documented in round 1; the round-2 path and invocation log were separate.

Actual command:

```sh
CARGO_BIN_EXE_ddb=/Users/bob/git/src/github.com/doogat/ddb/dev/local/tmp/00171-devon/body-to-title-r2.sh cargo test -p ddb-e2e --test e2e -- integration_singleton_races integration_singleton_parity integration_write_lock_races
```

Exit: **101**. **Caught** by the new committed-body equality check. Actual output:

```text
    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.32s
     Running e2e/main.rs (target/debug/deps/e2e-851e240f61a32247)

running 5 tests
test integration_singleton_races::integration_55_b_graphql_execute_sql_singleton_insert_race ... ok
test integration_singleton_races::integration_55_c_graphql_create_doogat_singleton_insert_race ... ok
test integration_singleton_races::integration_55_d_cli_vs_pgwire_singleton_insert_race ... ok
test integration_singleton_parity::integration_54_d_cross_protocol_singleton_duplicate_parity ... ok
test integration_write_lock_races::integration_61_b_concurrent_cli_and_graphql_write_lock ... FAILED

failures:

---- integration_write_lock_races::integration_61_b_concurrent_cli_and_graphql_write_lock stdout ----

thread 'integration_write_lock_races::integration_61_b_concurrent_cli_and_graphql_write_lock' (479754470) panicked at tests/e2e/integration_write_lock_races.rs:210:9:
assertion `left == right` failed: CLI write 0 must update the committed body for 20260905203928
  left: ""
 right: "wl-cli-landed-0"
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace

failures:
    integration_write_lock_races::integration_61_b_concurrent_cli_and_graphql_write_lock

test result: FAILED. 4 passed; 1 failed; 0 ignored; 0 measured; 560 filtered out; finished in 8.68s

error: test failed, to rerun pass `-p ddb-e2e --test e2e`
```

Actual facade invocation evidence:

```text
MUTATED --body to --title: --repo /var/folders/m6/1mnc9pdn28d5vdxy4hxyr1m40000gn/T/.tmpdbd7rI update 20260905203928 --title wl-cli-landed-0
MUTATED --body to --title: --repo /var/folders/m6/1mnc9pdn28d5vdxy4hxyr1m40000gn/T/.tmpdbd7rI update 20260905203929 --title wl-cli-landed-1
MUTATED --body to --title: --repo /var/folders/m6/1mnc9pdn28d5vdxy4hxyr1m40000gn/T/.tmpdbd7rI update 20260905203930 --title wl-cli-landed-2
MUTATED --body to --title: --repo /var/folders/m6/1mnc9pdn28d5vdxy4hxyr1m40000gn/T/.tmpdbd7rI update 20260905203931 --title wl-cli-landed-3
```

The facade was overwritten with a harmless forwarder before mutation 2.

## Mutation 2: corrupt CLI SQL INSERT values

The facade appended `-CORRUPTED` inside the last string literal of every CLI SQL `INSERT INTO ...` command. It did not match specific table names, IDs, expected test values, or fixture paths. Other CLI commands and the server were forwarded unchanged.

The requested singleton seed value `p1` consequently became `p1-CORRUPTED`; the two ordinary-table CLI inserts were also corrupted. This is incorrect product behavior: accepted write values are changed without a request to change them.

Executed mutant:

```bash
#!/bin/bash
arguments=("$@")
quote="'"
for ((i = 0; i + 1 < ${#arguments[@]}; i++)); do
    if [[ "${arguments[i]}" == "query" && "${arguments[i+1]}" == "INSERT INTO "* ]]; then
        sql="${arguments[i+1]}"
        prefix="${sql%${quote}*}"
        suffix="${sql##*${quote}}"
        arguments[i+1]="${prefix}-CORRUPTED${quote}${suffix}"
        printf 'MUTATED SQL INSERT: %s\n' "${arguments[i+1]}" >> /Users/bob/git/src/github.com/doogat/ddb/dev/local/tmp/00171-devon/insert-value-r2-invocations.log
    fi
done
exec /Users/bob/git/src/github.com/doogat/ddb/target/debug/ddb "${arguments[@]}"
```

Actual command:

```sh
CARGO_BIN_EXE_ddb=/Users/bob/git/src/github.com/doogat/ddb/dev/local/tmp/00171-devon/insert-value-r2.sh cargo test -p ddb-e2e --test e2e -- integration_singleton_races integration_singleton_parity integration_write_lock_races
```

Exit: **0**. **Survived all five tests**. Actual output:

```text
    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.14s
     Running e2e/main.rs (target/debug/deps/e2e-851e240f61a32247)

running 5 tests
test integration_singleton_races::integration_55_b_graphql_execute_sql_singleton_insert_race ... ok
test integration_singleton_races::integration_55_c_graphql_create_doogat_singleton_insert_race ... ok
test integration_singleton_races::integration_55_d_cli_vs_pgwire_singleton_insert_race ... ok
test integration_singleton_parity::integration_54_d_cross_protocol_singleton_duplicate_parity ... ok
test integration_write_lock_races::integration_61_b_concurrent_cli_and_graphql_write_lock ... ok

test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 560 filtered out; finished in 9.10s
```

Actual facade invocation evidence:

```text
MUTATED SQL INSERT: INSERT INTO ig_parity_cfg (title, theme) VALUES ('seed', 'p1-CORRUPTED')
MUTATED SQL INSERT: INSERT INTO ig_parity_plain (title, theme) VALUES ('first', 'q1-CORRUPTED')
MUTATED SQL INSERT: INSERT INTO ig_parity_plain (title, theme) VALUES ('second', 'q2-CORRUPTED')
```

## Assertion gap and repair

The final `SELECT theme FROM ig_parity_cfg` in `integration_singleton_parity.rs` uses `predicate::str::contains("p1")`. This accepts `p1-CORRUPTED`, so a successful seed insert can silently change its payload while every duplicate-rejection and end-state assertion still passes.

For the direct fix, capture that query's stdout and assert its trimmed value equals `p1`, matching the exact-value approach already used in `integration_singleton_races.rs`. Checking the seed's committed parsed theme too would additionally bind the value to durable Git content, but exact query equality alone rejects this demonstrated mutant. The ordinary-table control checks row count only, so it also accepts changed payloads; exact expected row-value assertions would cover that broader property if desired.

The bounded round stopped after this surviving mutant, as instructed. Two mutations were actually exercised: one caught, one survived. No third mutant, infrastructure failures, or claims of universal robustness.

## Cleanup and ownership

Both new executable facades were overwritten and neutralized before return:

```bash
#!/bin/bash
# Harmless placeholder: forwards all arguments unchanged.
exec /Users/bob/git/src/github.com/doogat/ddb/target/debug/ddb "$@"
```

Paths retained for parent cleanup:

- `dev/local/reviews/00171-task12-devon-round2.md` — report and command/output evidence.
- `dev/local/tmp/00171-devon/body-to-title-r2.sh` — harmless forwarder.
- `dev/local/tmp/00171-devon/insert-value-r2.sh` — harmless forwarder.
- `dev/local/tmp/00171-devon/body-to-title-r2-invocations.log` — evidence, copied above.
- `dev/local/tmp/00171-devon/insert-value-r2-invocations.log` — evidence, copied above.

Round-1 files were preserved unchanged. No files removed. All Cargo sessions finished; exclusive Cargo ownership was explicitly released to the parent immediately after mutation 2's result and facade cleanup.

## Parent regression replay after repair

Commit `3965754` changes the final theme query to exact trimmed equality. The same mutant and five-test command above were replayed with facade path `dev/local/tmp/00171-devon/insert-value-replay.sh`. Exit 101: 4 passed, 1 failed, 0 ignored, 560 filtered out, 8.44s. The failure is `integration_singleton_parity.rs:200`: left `p1-CORRUPTED`, right `p1`. The known mutant is now rejected; this was a regression replay, not a third exploratory Devon round. The replay facade was neutralized immediately. No demonstrated surviving mutant remains.
