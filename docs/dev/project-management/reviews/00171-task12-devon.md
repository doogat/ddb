# Devon adversarial validation — task 12

Verdict: **a wrong implementation passed every provided test**. Generic CLI body updates redirected to titles passed all five tests. The suite caught a silently discarded update, but its committed-content checks do not require values to land in the requested field. This is a bounded mutation result, not a claim about universal test robustness.

Scope: only the three test files supplied in the dispatch and `tests/e2e/common.rs` were read. No production source, architecture material, PRDs, or task descriptions were read. No test or production files were edited. The existing `target/debug/ddb` executable was used unchanged behind scratch executable facades selected with `CARGO_BIN_EXE_ddb`. Every invocation used the ordinary parallel runner and exactly these scopes:

```sh
cargo test -p ddb-e2e --test e2e -- integration_singleton_races integration_singleton_parity integration_write_lock_races
```

## Baseline

Command: the unmodified command above, outside the sandbox to permit localhost listeners.

Exit: **0**. Actual output:

```text
    Finished `test` profile [unoptimized + debuginfo] target(s) in 5.75s
     Running e2e/main.rs (target/debug/deps/e2e-851e240f61a32247)

running 5 tests
test integration_singleton_races::integration_55_b_graphql_execute_sql_singleton_insert_race ... ok
test integration_singleton_races::integration_55_d_cli_vs_pgwire_singleton_insert_race ... ok
test integration_singleton_races::integration_55_c_graphql_create_doogat_singleton_insert_race ... ok
test integration_singleton_parity::integration_54_d_cross_protocol_singleton_duplicate_parity ... ok
test integration_write_lock_races::integration_61_b_concurrent_cli_and_graphql_write_lock ... ok

test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 560 filtered out; finished in 6.92s
```

## Mutation 1: silently discard CLI updates

The scratch executable returned success for every CLI `update` without invoking the real executable. All other arguments were forwarded unchanged.

Executed mutant:

```bash
#!/bin/bash
for argument in "$@"; do
    if [[ "$argument" == "update" ]]; then
        exit 0
    fi
done
exec /Users/bob/git/src/github.com/doogat/ddb/target/debug/ddb "$@"
```

Actual command:

```sh
CARGO_BIN_EXE_ddb=/Users/bob/git/src/github.com/doogat/ddb/dev/local/tmp/00171-devon/drop-update.sh cargo test -p ddb-e2e --test e2e -- integration_singleton_races integration_singleton_parity integration_write_lock_races
```

Exit: **101**. **Caught**, because the committed document lacked the CLI update marker. Actual output:

```text
    Finished `test` profile [unoptimized + debuginfo] target(s) in 4.93s
     Running e2e/main.rs (target/debug/deps/e2e-851e240f61a32247)

running 5 tests
test integration_singleton_races::integration_55_b_graphql_execute_sql_singleton_insert_race ... ok
test integration_singleton_races::integration_55_d_cli_vs_pgwire_singleton_insert_race ... ok
test integration_singleton_races::integration_55_c_graphql_create_doogat_singleton_insert_race ... ok
test integration_singleton_parity::integration_54_d_cross_protocol_singleton_duplicate_parity ... ok
test integration_write_lock_races::integration_61_b_concurrent_cli_and_graphql_write_lock ... FAILED

failures:

---- integration_write_lock_races::integration_61_b_concurrent_cli_and_graphql_write_lock stdout ----

thread 'integration_write_lock_races::integration_61_b_concurrent_cli_and_graphql_write_lock' (479307165) panicked at tests/e2e/integration_write_lock_races.rs:195:9:
CLI write 0 not durable in HEAD for 20260905191744: HEAD lacks "wl-cli-landed-0"
HEAD content:
---
id: 20260905191744
title: WL cli seed 0
date: 2026-09-05
---

note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace

failures:
    integration_write_lock_races::integration_61_b_concurrent_cli_and_graphql_write_lock

test result: FAILED. 4 passed; 1 failed; 0 ignored; 0 measured; 560 filtered out; finished in 8.31s

error: test failed, to rerun pass `-p ddb-e2e --test e2e`
```

The facade was overwritten with a harmless unchanged-argument forwarder before creating mutation 2.

## Mutation 2: misroute body updates into the title

The scratch executable changes every CLI `update ... --body VALUE` into `update ... --title VALUE`. This drops the requested body write and overwrites an unrelated existing title. It matches no fixture names, expected values, IDs, or table names; all unaffected commands are forwarded.

Executed mutant:

```bash
#!/bin/bash
arguments=("$@")
is_update=false
mutated=false
for ((i = 0; i < ${#arguments[@]}; i++)); do
    if [[ "${arguments[i]}" == "update" ]]; then
        is_update=true
    elif $is_update && [[ "${arguments[i]}" == "--body" ]]; then
        arguments[i]=--title
        mutated=true
    fi
done
if $mutated; then
    printf 'MUTATED --body to --title: %s\n' "${arguments[*]}" >> /Users/bob/git/src/github.com/doogat/ddb/dev/local/tmp/00171-devon/body-to-title-invocations.log
fi
exec /Users/bob/git/src/github.com/doogat/ddb/target/debug/ddb "${arguments[@]}"
```

Actual command:

```sh
CARGO_BIN_EXE_ddb=/Users/bob/git/src/github.com/doogat/ddb/dev/local/tmp/00171-devon/body-to-title.sh cargo test -p ddb-e2e --test e2e -- integration_singleton_races integration_singleton_parity integration_write_lock_races
```

First invocation exited **101 before running any tests**, due to concurrent unrelated source edits:

```text
error[E0433]: cannot find module or crate `predicate` in this scope
  --> tests/e2e/cli_create_fk_freshness.rs:29:17
   |
29 |         .stdout(predicate::str::contains("table category created"));
   |                 ^^^^^^^^^ use of unresolved module or unlinked crate `predicate`

error[E0433]: cannot find module or crate `predicate` in this scope
  --> tests/e2e/cli_create_fk_freshness.rs:37:17
   |
37 |         .stdout(predicate::str::contains("table link created"));
   |                 ^^^^^^^^^ use of unresolved module or unlinked crate `predicate`

error: could not compile `ddb-e2e` (test "e2e") due to 2 previous errors
```

This infrastructure failure is not a caught mutant and is excluded from the verdict.

The parent confirmed that the unrelated import had been fixed, then the **identical command** was rerun. Exit: **0**. **Survived all five tests**. Actual output:

```text
    Finished `test` profile [unoptimized + debuginfo] target(s) in 4.97s
     Running e2e/main.rs (target/debug/deps/e2e-851e240f61a32247)

running 5 tests
test integration_singleton_races::integration_55_b_graphql_execute_sql_singleton_insert_race ... ok
test integration_singleton_races::integration_55_c_graphql_create_doogat_singleton_insert_race ... ok
test integration_singleton_races::integration_55_d_cli_vs_pgwire_singleton_insert_race ... ok
test integration_singleton_parity::integration_54_d_cross_protocol_singleton_duplicate_parity ... ok
test integration_write_lock_races::integration_61_b_concurrent_cli_and_graphql_write_lock ... ok

test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 560 filtered out; finished in 7.52s
```

The facade recorded each transformed invocation before executing the real binary. Actual evidence shows that all four updates were mutated:

```text
MUTATED --body to --title: --repo /var/folders/m6/1mnc9pdn28d5vdxy4hxyr1m40000gn/T/.tmpycfeZP update 20260905192650 --title wl-cli-landed-1
MUTATED --body to --title: --repo /var/folders/m6/1mnc9pdn28d5vdxy4hxyr1m40000gn/T/.tmpycfeZP update 20260905192649 --title wl-cli-landed-0
MUTATED --body to --title: --repo /var/folders/m6/1mnc9pdn28d5vdxy4hxyr1m40000gn/T/.tmpycfeZP update 20260905192651 --title wl-cli-landed-2
MUTATED --body to --title: --repo /var/folders/m6/1mnc9pdn28d5vdxy4hxyr1m40000gn/T/.tmpycfeZP update 20260905192652 --title wl-cli-landed-3
```

## Assertion gap and repair

`integration_61_b_concurrent_cli_and_graphql_write_lock` in `tests/e2e/integration_write_lock_races.rs` checks the CLI writes with `head.contains(&needle)` at line 195. A marker inside frontmatter title satisfies that condition despite the requested Markdown body never being written. The same unstructured search is used for GraphQL title writes.

Retain the existing committed-`HEAD` reads, then parse their content and assert field identity:

- CLI targets: body equals `wl-cli-landed-{k}` and title still equals `WL cli seed {k}`.
- GraphQL targets: title equals `wl-serve-landed-{k}`, with an assertion that an unrelated seeded field/body is preserved where appropriate.

Checking only the preserved CLI seed title would reject this exact mutant, but parsing and checking the requested body is the stronger direct assertion of the workflow. Avoid reverting to work-tree reads; mutation 1 demonstrated that the committed-tree check catches missing writes.

No third mutation was needed after a wrong implementation passed all tests. Two actual product mutations were exercised: one caught, one survived; the unrelated compile failure is not counted as an experiment outcome.

## Cleanup paths

- Report/evidence: `dev/local/reviews/00171-task12-devon.md`.
- Scratch facade 1: `dev/local/tmp/00171-devon/drop-update.sh` (neutralized).
- Scratch facade 2: `dev/local/tmp/00171-devon/body-to-title.sh` (neutralized).
- Invocation evidence: `dev/local/tmp/00171-devon/body-to-title-invocations.log` (retained; also copied above).

Both executable facades were overwritten with the same harmless placeholder before return:

```bash
#!/bin/bash
# Harmless placeholder: forwards all arguments unchanged.
exec /Users/bob/git/src/github.com/doogat/ddb/target/debug/ddb "$@"
```

No files were removed, no git config/hooks changed by this validator, no source/test changes, no commits, and no autopilot-state writes. Cargo has no active session remaining; ownership returned to the parent.
