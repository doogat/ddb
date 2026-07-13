# Showboat Walkthroughs

Executable feature demos built with [showboat](https://github.com/simonw/showboat), a CLI tool by Simon Willison. Each walkthrough is a Markdown file containing commentary and code blocks with real captured output. `showboat verify` re-runs all code blocks and confirms outputs still match.

## Location and naming

`dev/local/walkthroughs/{5-digit}-{slug}.md` — e.g. `00001-crud-basics.md`

## Installation

Run via uvx (no install needed):

```text
uvx showboat --help
```

Or install persistently: `uv tool install showboat` / `pip install showboat`

## Critical rule: showboat CLI only

Agents **must not** edit walkthrough files directly (no `Edit`, `Write`, `sed`, etc.). All content flows through showboat CLI:

| Command | Purpose |
|---------|---------|
| `showboat init <file> <title>` | Create new walkthrough |
| `showboat note <file> <text>` | Add commentary (also accepts stdin) |
| `showboat exec <file> <lang> <code>` | Run code, capture real output |
| `showboat pop <file>` | Remove last entry (undo failed exec) |
| `showboat image <file> <path>` | Embed an image |
| `showboat verify <file>` | Re-run all blocks, diff against recorded output (never run directly from project root — creates real commits on master; see "Verifying walkthroughs safely" below) |
| `showboat extract <file>` | Emit commands to recreate file (for rebuilding) |

Output blocks contain real captured output. Direct file editing defeats the purpose — walkthroughs are proof of work.

## Execution model

Each `showboat exec` runs in its own shell. Variables, background jobs, and working directory do **not** persist between calls. Use `--workdir <dir>` to set the working directory for a command.

`exec` prints captured output to stdout and exits with the same code as the executed command, so agents can react to errors. Use `pop` to remove a failed entry before retrying.

## Patterns

**CLI walkthrough** — use `--workdir` with a fixed temp path (not `mktemp`, since the path must be reused across exec calls):

```text
WD=/tmp/ddb-demo-feature
showboat init dev/local/walkthroughs/00001-feature.md "Feature Name"
showboat note dev/local/walkthroughs/00001-feature.md "Initialize a repo."
showboat exec --workdir $WD dev/local/walkthroughs/00001-feature.md bash "mkdir -p $WD && ddb init"
showboat exec --workdir $WD dev/local/walkthroughs/00001-feature.md bash "ddb create --title 'Test'"
showboat exec dev/local/walkthroughs/00001-feature.md bash "rm -rf $WD"
```

**Server walkthrough** — use PID file pattern since background jobs don't persist across exec calls:

```text
showboat exec --workdir $WD ... bash "ddb serve --port 19201 --pg-port 19202 & echo \$! > /tmp/ddb-serve.pid"
showboat exec ... bash "sleep 1 && curl -s http://127.0.0.1:19201/graphql -H 'Content-Type: application/json' -d '{...}'"
showboat exec ... bash "kill \$(cat /tmp/ddb-serve.pid) && rm /tmp/ddb-serve.pid"
```

## Maintenance

Walkthroughs are local working documents (`dev/local/` is gitignored). They can be regenerated anytime. When CLI output changes cause `showboat verify` to fail, regenerate using `showboat extract` to get the original commands, then re-execute.

## Verifying walkthroughs safely

Never run `showboat verify <walkthrough>` directly from the project root. Use the wrapper:

```text
dev/bin/safe-showboat-verify <walkthrough> [<walkthrough>...]
```

`showboat verify` re-executes the walkthrough's bash blocks in **the caller's cwd**. The original `--workdir` from `showboat exec` is not recorded in the rendered Markdown, so verify has no way to honor it. If the cwd is itself a git repo, blocks that auto-commit (e.g. `ddb create`, `ddb query "INSERT INTO ..."`) write real commits to that repo. PRD 00135 documents two `git reset --hard` cleanups caused by this.

The wrapper runs verify inside a throwaway `git worktree` under `dev/local/worktrees/showboat-verify-<id>/` and removes it on exit, so any contamination lands in the worktree, never on the active checkout.

To confirm the wrapper still works after upgrading showboat or editing the wrapper, run the regression test:

```text
dev/bin/showboat-verify-no-contamination-test.sh
```

It runs the wrapper against a known walkthrough (a 00050+ fixture exhibiting the contamination pattern) and asserts HEAD, working-tree status, project-root data dirs (`ddb/`, `.ddb/`, `.crdt/`, `.nodes/`), and the worktree list are unchanged afterward. If no contaminating fixture is available, the test fails fast rather than passing vacuously on a self-isolating walkthrough.
