# Sync: collision losers without frontmatter are kept verbatim

*2026-10-02T21:29:00Z by Showboat 0.6.1*
<!-- showboat-id: e8e2addf-2723-4935-be0c-485a46355041 -->

Two devices create ddb/{id}.md under the same id. If the losing side of that add/add collision has no frontmatter block, sync used to fail on every run: the loser's id could not be rewritten, so nothing landed. Now the loser lands at its derived content-id path with its bytes unchanged, sync converges in one round, and ddb sync names the file. This demo uses the ddb CLI in a throwaway temp dir with a bare remote and two nodes. Logs are off and node UUIDs are replaced with a placeholder.

```bash
set -eu; export RUST_LOG=off; DDB=/Users/bob/git/src/github.com/doogat/ddb/target/debug/ddb
W=$(mktemp -d /tmp/ddb-walk-00204.XXXXXX); echo "$W" > /tmp/ddb-walk-00204.path; cd "$W"
git init -q --bare remote.git
$DDB init node1 >/dev/null
git -C node1 config commit.gpgsign false
git -C node1 remote add origin "$W/remote.git"
$DDB --repo node1 register-node Laptop | sed -E 's/\([0-9a-f-]{36}\)/(<uuid>)/'
git -C node1 push -q -u origin master
git clone -q remote.git node2
git -C node2 config commit.gpgsign false
$DDB --repo node2 register-node Desktop | sed -E 's/\([0-9a-f-]{36}\)/(<uuid>)/'
```

```output
registered node Laptop (<uuid>)
registered node Desktop (<uuid>)
```

Both nodes write ddb/20260310000000.md outside ddb (plain git, no HLC trailer). Laptop's file is a normal doogat and is pushed first. Desktop's file has no frontmatter. With no HLC on either side, the higher content wins, so Desktop's '# ...' file (0x23 sorts below '---') is the loser. A copy of its bytes is kept for comparison.

```bash
set -u; export RUST_LOG=off; DDB=/Users/bob/git/src/github.com/doogat/ddb/target/debug/ddb; cd "$(< /tmp/ddb-walk-00204.path)"
printf -- '---\nid: 20260310000000\ntitle: Meeting notes\n---\nAgenda from the laptop.\n' > node1/ddb/20260310000000.md
git -C node1 add ddb && git -C node1 commit -qm 'test: add laptop notes'
git -C node1 push -q origin master
printf '# Desk notes\nWritten by hand, no frontmatter.\n' > node2/ddb/20260310000000.md
cp node2/ddb/20260310000000.md loser.orig
git -C node2 add ddb && git -C node2 commit -qm 'test: add desktop notes'
echo committed
```

```output
committed
```

Desktop syncs. The collision resolves in one round. The winner keeps the contested path; the loser is reassigned to a content-derived id, so any device resolving the same collision picks the same path. The new lines name each loser kept verbatim, old path first.

```bash
set -u; export RUST_LOG=off; DDB=/Users/bob/git/src/github.com/doogat/ddb/target/debug/ddb; cd "$(< /tmp/ddb-walk-00204.path)"
$DDB --repo node2 sync; echo "exit=$?"
ls node2/ddb
NEW=$(ls node2/ddb | grep -v -e 20260310000000.md -e _typedef -e .gitkeep)
cmp loser.orig "node2/ddb/$NEW" && echo "loser bytes unchanged at ddb/$NEW"
head -n 2 node2/ddb/20260310000000.md
```

```output
sync: bidirectional | commits: 1 | conflicts resolved: 1
  collisions reassigned: 1
  collision losers kept verbatim: 1
    ddb/20260310000000.md -> ddb/20811107183254.md
exit=0
20260310000000.md
20811107183254.md
loser bytes unchanged at ddb/20811107183254.md
---
id: 20260310000000
```

A second sync has nothing new to report, and Laptop converges to the same two files.

```bash
set -u; export RUST_LOG=off; DDB=/Users/bob/git/src/github.com/doogat/ddb/target/debug/ddb; cd "$(< /tmp/ddb-walk-00204.path)"
$DDB --repo node2 sync; echo "exit=$?"
$DDB --repo node1 sync >/dev/null; echo "laptop exit=$?"
diff <(ls node1/ddb) <(ls node2/ddb) && echo "same files on both nodes"
```

```output
sync: up-to-date | commits: 0 | conflicts resolved: 0
exit=0
laptop exit=0
same files on both nodes
```

Operator follow-up. The kept file has no frontmatter, so the index skips it with a warning, and ddb reindex --strict fails until it gets one. Adding frontmatter with the new id (the file name) and committing makes it a normal doogat.

```bash
set -u; export RUST_LOG=off; DDB=/Users/bob/git/src/github.com/doogat/ddb/target/debug/ddb; cd "$(< /tmp/ddb-walk-00204.path)"
NEW=$(ls node2/ddb | grep -v -e 20260310000000.md -e _typedef -e .gitkeep); ID=${NEW%.md}
$DDB --repo node2 reindex --strict > strict.out 2>&1; echo "strict exit=$?"
grep -c "$NEW" strict.out
{ printf -- "---\nid: %s\ntitle: Desk notes\n---\n" "$ID"; cat "node2/ddb/$NEW"; } > fixed.md && mv fixed.md "node2/ddb/$NEW"
git -C node2 add ddb && git -C node2 commit -qm "fix: add frontmatter to kept loser"
$DDB --repo node2 reindex --strict >/dev/null; echo "strict exit=$?"
$DDB --repo node2 read "$ID" | sed -n 3p
```

```output
strict exit=1
1
strict exit=0
title: Desk notes
```

Limits. Only a loser with no frontmatter block at all is kept verbatim; a block whose YAML is invalid still aborts the whole merge, as before. A body with two '---' lines counts as a block and still takes the id-rewrite path (known residual, tracked in PRD 00204's follow-ups). FFI has no sync call, so only the CLI and the GraphQL sync mutation (collisionLosersKeptVerbatim) show these records; ddb bundle import prints the same lines. Contract: docs/src/technical/sync.md. Tests: tests/e2e/collision.rs.
