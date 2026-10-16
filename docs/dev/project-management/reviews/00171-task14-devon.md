# Task 14 adversarial validation — Devon

Telemetry: `b76c7802`. Commit supplied by parent: `c8b72bd`. Role: independent adversarial validator, using the inherited native Codex runtime in place of the planned Claude Opus agent. No subagents.

**Tests are robust for these experiments.** Three actual generic mutants were caught: two by the focused tests and one by the complete suite. This is bounded experimental evidence, not universal proof. The SQL-title mutant exposes a focused-suite assertion gap, but it is **not an unresolved whole-suite gap**.

## Baseline and commands

All commands ran from `/Users/bob/git/src/github.com/doogat/ddb`, with foreground Cargo ownership exclusive to this validator and ordinary test parallelism. Socket runs used standard sandbox escalation. No serial markers or `--test-threads=1` were used.

Complete real-binary baseline:

```sh
CARGO_BIN_EXE_ddb=/Users/bob/git/src/github.com/doogat/ddb/target/debug/ddb /usr/bin/time -p cargo test -p ddb-e2e
```

Exit **0**. Exact runner summary:

```text
test result: ok. 565 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 53.60s
real 66.90
user 115.30
sys 88.59
```

Evidence: `dev/local/tmp/00171-devon14/baseline.log`. One intermediate tool-output chunk was truncated; the exact terminal summary and timing are intact. The initial attempt to redirect output into `dev/local/` was blocked by a hook before execution. Evidence was instead saved through the file-edit tool; no hooks were disabled.

## Experiment 1: body update routed into title

Generic behavior: replace `--body` with `--title` on CLI `update`; forward all other arguments unchanged. This deliberately places the requested payload in the wrong Markdown zone.

```sh
CARGO_BIN_EXE_ddb=/Users/bob/git/src/github.com/doogat/ddb/dev/local/tmp/00171-devon14/body_to_title.rb /usr/bin/time -p cargo test -p ddb-e2e --test e2e -- integration_singleton_races integration_singleton_parity integration_write_lock_races
```

Exit **101**. Exact runner summary:

```text
test result: FAILED. 3 passed; 2 failed; 0 ignored; 0 measured; 560 filtered out; finished in 8.18s
real 8.38
user 3.26
sys 1.45
```

**4 actual mutated invocations**, recorded in `body_to_title.jsonl`. All four writes returned success before durability assertions ran. Representative altered arguments:

```text
update 20260906083005 --body wl-cli-landed-0
→ update 20260906083005 --title wl-cli-landed-0
```

Caught at `integration_write_lock_races.rs:209`: `CLI write 0 must update the committed body`; actual parsed body `""`, expected `"wl-cli-landed-0"`. The earlier HEAD substring assertion passed, demonstrating why parsed-zone equality matters.

The second failure was an unrelated parity GraphQL connection refusal in `common.rs:169`; it is **excluded from the mutation verdict**. The final real-binary focused control below passed, including parity.

The complete suite was not required because focused tests caught the mutant.

## Experiment 2: SINGLETON declaration dropped

Generic behavior: remove a trailing `SINGLETON` modifier from CLI `query CREATE TABLE` statements; forward every other argument unchanged. No table-name, fixture-value, ID, or test-name selectors.

```sh
CARGO_BIN_EXE_ddb=/Users/bob/git/src/github.com/doogat/ddb/dev/local/tmp/00171-devon14/drop_singleton.rb /usr/bin/time -p cargo test -p ddb-e2e --test e2e -- integration_singleton_races integration_singleton_parity integration_write_lock_races
```

Exit **101**. Exact runner summary:

```text
test result: FAILED. 1 passed; 4 failed; 0 ignored; 0 measured; 560 filtered out; finished in 8.03s
real 12.38
user 2.80
sys 1.60
```

**4 actual mutated invocations**, recorded in `drop_singleton.jsonl`. Representative alteration:

```text
query "CREATE TABLE ig_race_sql (theme TEXT) SINGLETON"
→ query "CREATE TABLE ig_race_sql (theme TEXT)"
```

Caught by both GraphQL race cases at `integration_singleton_races.rs:41`: two successful responses instead of one. The CLI/PgWire race failed at line 247 with two successes instead of one. Parity failed at `integration_singleton_parity.rs:20` because the duplicate INSERT succeeded without GraphQL errors. Write-lock case passed.

The complete suite was not required because focused tests caught the mutant.

## Experiment 3: explicit SQL INSERT title discarded

Generic behavior: for CLI SQL INSERT statements whose first named column is `title` and first VALUES expression is a string literal, replace that literal with `NULL`. All other columns and commands pass through unchanged. This is a semantic argument-class selector, not a fixture selector.

Focused command:

```sh
CARGO_BIN_EXE_ddb=/Users/bob/git/src/github.com/doogat/ddb/dev/local/tmp/00171-devon14/drop_insert_title.rb /usr/bin/time -p cargo test -p ddb-e2e --test e2e -- integration_singleton_races integration_singleton_parity integration_write_lock_races
```

Exit **0**. Exact runner summary:

```text
test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 560 filtered out; finished in 7.92s
real 8.07
user 3.62
sys 1.33
```

**3 actual mutated invocations** in this focused run. The parity test successfully inserted its singleton seed and both ordinary-table rows using altered titles:

```text
INSERT INTO ig_parity_cfg (title, theme) VALUES ('seed', 'p1')
→ INSERT INTO ig_parity_cfg (title, theme) VALUES (NULL, 'p1')

INSERT INTO ig_parity_plain (title, theme) VALUES ('first', 'q1')
→ INSERT INTO ig_parity_plain (title, theme) VALUES (NULL, 'q1')

INSERT INTO ig_parity_plain (title, theme) VALUES ('second', 'q2')
→ INSERT INTO ig_parity_plain (title, theme) VALUES (NULL, 'q2')
```

The focused tests validate counts and theme values but do not validate these supplied SQL titles. An assertion that the singleton seed's stored/committed title equals `seed` would prevent this focused survivor; checking ordinary-row titles would cover the control inserts too.

Required complete-suite command, with the **same facade unchanged**:

```sh
CARGO_BIN_EXE_ddb=/Users/bob/git/src/github.com/doogat/ddb/dev/local/tmp/00171-devon14/drop_insert_title.rb /usr/bin/time -p cargo test -p ddb-e2e
```

Exit **101**. Exact runner summary:

```text
test result: FAILED. 559 passed; 6 failed; 0 ignored; 0 measured; 0 filtered out; finished in 56.06s
real 56.21
user 172.85
sys 118.88
```

**57 actual mutated invocations** in the complete run, **60 total** in `drop_insert_title.jsonl` including the preceding focused run. Counts were taken after completion; one append-only JSON record is written for each altered invocation before the real executable is invoked.

Five behavior failures caught this mutant:

- `smoke_sql::smoke_20_type_table_self_contained`: SELECT title yielded `NULL` rather than `Self-Contained Test`. This directly demonstrates persisted wrong behavior.
- `smoke_sql::smoke_11d_join_smoke_pin`: title-based join returned no rows rather than `a | 1`.
- `smoke_search::smoke_28_in_query_field_filter`: title search returned no results rather than `Meeting Notes Archive`.
- `title_template_references::membership_title_resolves_from_referenced_doogats`: derived title was ` in A/B` rather than `My Link in A/B`.
- `title_template_references::update_recomputes_membership_title`: derived title was empty rather than containing `Link B`.

The sixth failure, `pgwire::pgwire_connect_auth_ok`, was a connection refusal and is **excluded from the mutation verdict**. No startup failure is counted as a caught mutant.

## Final real-binary focused control and cleanup

After overwriting all three executable facades with harmless forwarders, the requested real-binary control ran:

```sh
CARGO_BIN_EXE_ddb=/Users/bob/git/src/github.com/doogat/ddb/target/debug/ddb /usr/bin/time -p cargo test -p ddb-e2e --test e2e -- integration_singleton_races integration_singleton_parity integration_write_lock_races
```

Exit **0**. Exact runner summary:

```text
test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 560 filtered out; finished in 7.34s
real 7.49
user 2.81
sys 0.94
```

Every created executable now contains exactly this forwarder, verified by reading all three files:

```sh
#!/bin/sh
exec /Users/bob/git/src/github.com/doogat/ddb/target/debug/ddb "$@"
```

No production/test files were edited; no commits, Git configuration changes, persistent environment changes, autopilot state changes, or `rm` commands were made. Cargo was released to the parent at approximately **07:06 UTC**, within 45 minutes of the first clock check at **06:26:08 UTC**.

Timebox caveat: the 15-minute check-in was missed across tool/approval waits; the next observed clock was 06:59:53 UTC, and the parent received an immediate update. The hard cap was respected. Source reads were confined to the supplied dispatch/test bodies and `tests/e2e/common.rs`; no product source, PRD, architecture document, or other agent prompt/report was read.

## Preserved mutation code

These are the exact experimental bodies as executed, archived here because the executable files have been neutralized.

### body_to_title.rb

```ruby
#!/usr/bin/ruby
require 'json'
REAL = '/Users/bob/git/src/github.com/doogat/ddb/target/debug/ddb'
before = ARGV.dup
args = ARGV.dup

command = args.index('update')
if command && args[(command + 1)..].include?('--body')
  body_index = ((command + 1)...args.length).find { |index| args[index] == '--body' }
  args[body_index] = '--title'
end

if args != before
  File.open(File.join(__dir__, 'body_to_title.jsonl'), 'a') do |f|
    f.flock(File::LOCK_EX)
    f.puts(JSON.generate({pid: Process.pid, before: before, after: args}))
  end
end
exec REAL, *args
```

### drop_singleton.rb

```ruby
#!/usr/bin/ruby
require 'json'
REAL = '/Users/bob/git/src/github.com/doogat/ddb/target/debug/ddb'
before = ARGV.dup
args = ARGV.dup

command = args.index('query')
if command && args[command + 1]&.match?(/\A\s*CREATE\s+TABLE\b/i)
  args[command + 1] = args[command + 1].sub(/\s+SINGLETON(?=\s*;?\s*\z)/i, '')
end

if args != before
  File.open(File.join(__dir__, 'drop_singleton.jsonl'), 'a') do |f|
    f.flock(File::LOCK_EX)
    f.puts(JSON.generate({pid: Process.pid, before: before, after: args}))
  end
end
exec REAL, *args
```

### drop_insert_title.rb

```ruby
#!/usr/bin/ruby
require 'json'
REAL = '/Users/bob/git/src/github.com/doogat/ddb/target/debug/ddb'
before = ARGV.dup
args = ARGV.dup

command = args.index('query')
if command && args[command + 1]
  sql = args[command + 1]
  if sql.match?(/\A\s*INSERT\s+INTO\s+[^\s(]+\s*\(\s*title\s*[,)]/i)
    args[command + 1] = sql.sub(/(\bVALUES\s*\(\s*)'(?:[^']|'')*'/i, '\\1NULL')
  end
end

if args != before
  File.open(File.join(__dir__, 'drop_insert_title.jsonl'), 'a') do |f|
    f.flock(File::LOCK_EX)
    f.puts(JSON.generate({pid: Process.pid, before: before, after: args}))
  end
end
exec REAL, *args
```

## Scratch inventory for parent cleanup

All created scratch files are under `dev/local/tmp/00171-devon14/`:

- `baseline.log`
- `body_to_title.rb` — neutralized executable
- `body_to_title.jsonl`
- `body_to_title-focused.log`
- `drop_singleton.rb` — neutralized executable
- `drop_singleton.jsonl`
- `drop_singleton-focused.log`
- `drop_insert_title.rb` — neutralized executable
- `drop_insert_title.jsonl`
- `drop_insert_title-focused.log`
- `drop_insert_title-full.log`
- `real-focused-control.log`

Retain this report at `dev/local/reviews/00171-task14-devon.md`. No separate test repositories or persistent server processes were created by the facades.

