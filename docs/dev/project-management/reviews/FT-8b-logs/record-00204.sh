#!/usr/bin/env bash
# Record walkthrough 00204 (FT-8b). showboat CLI only.
set -eu
F=/Users/bob/git/src/github.com/doogat/ddb/dev/local/walkthroughs/00204-sync-collision-loser-kept-verbatim.md
SB="uvx showboat"
cd /private/tmp/claude-501/-Users-bob-git-src-github-com-doogat-ddb/f3de6c34-82c9-4f73-a74a-80eed3a8256c/scratchpad
H='set -u; export RUST_LOG=off; DDB=/Users/bob/git/src/github.com/doogat/ddb/target/debug/ddb; cd "$(< /tmp/ddb-walk-00204.path)"'

$SB init "$F" 'Sync: collision losers without frontmatter are kept verbatim'
$SB note "$F" "Two devices create ddb/{id}.md under the same id. If the losing side of that add/add collision has no frontmatter block, sync used to fail on every run: the loser's id could not be rewritten, so nothing landed. Now the loser lands at its derived content-id path with its bytes unchanged, sync converges in one round, and ddb sync names the file. This demo uses the ddb CLI in a throwaway temp dir with a bare remote and two nodes. Logs are off and node UUIDs are replaced with a placeholder."
$SB exec "$F" bash 'set -eu; export RUST_LOG=off; DDB=/Users/bob/git/src/github.com/doogat/ddb/target/debug/ddb
W=$(mktemp -d /tmp/ddb-walk-00204.XXXXXX); echo "$W" > /tmp/ddb-walk-00204.path; cd "$W"
git init -q --bare remote.git
$DDB init node1 >/dev/null
git -C node1 config commit.gpgsign false
git -C node1 remote add origin "$W/remote.git"
$DDB --repo node1 register-node Laptop | sed -E '"'"'s/\([0-9a-f-]{36}\)/(<uuid>)/'"'"'
git -C node1 push -q -u origin master
git clone -q remote.git node2
git -C node2 config commit.gpgsign false
$DDB --repo node2 register-node Desktop | sed -E '"'"'s/\([0-9a-f-]{36}\)/(<uuid>)/'"'"''
$SB note "$F" "Both nodes write ddb/20260310000000.md outside ddb (plain git, no HLC trailer). Laptop's file is a normal doogat and is pushed first. Desktop's file has no frontmatter. With no HLC on either side, the higher content wins, so Desktop's '# ...' file (0x23 sorts below '---') is the loser. A copy of its bytes is kept for comparison."
$SB exec "$F" bash "$H"'
printf -- '"'"'---\nid: 20260310000000\ntitle: Meeting notes\n---\nAgenda from the laptop.\n'"'"' > node1/ddb/20260310000000.md
git -C node1 add ddb && git -C node1 commit -qm '"'"'test: add laptop notes'"'"'
git -C node1 push -q origin master
printf '"'"'# Desk notes\nWritten by hand, no frontmatter.\n'"'"' > node2/ddb/20260310000000.md
cp node2/ddb/20260310000000.md loser.orig
git -C node2 add ddb && git -C node2 commit -qm '"'"'test: add desktop notes'"'"'
echo committed'
$SB note "$F" "Desktop syncs. The collision resolves in one round. The winner keeps the contested path; the loser is reassigned to a content-derived id, so any device resolving the same collision picks the same path. The new lines name each loser kept verbatim, old path first."
$SB exec "$F" bash "$H"'
$DDB --repo node2 sync; echo "exit=$?"
ls node2/ddb
NEW=$(ls node2/ddb | grep -v -e 20260310000000.md -e _typedef -e .gitkeep)
cmp loser.orig "node2/ddb/$NEW" && echo "loser bytes unchanged at ddb/$NEW"
head -n 2 node2/ddb/20260310000000.md'
$SB note "$F" "A second sync has nothing new to report, and Laptop converges to the same two files."
$SB exec "$F" bash "$H"'
$DDB --repo node2 sync; echo "exit=$?"
$DDB --repo node1 sync >/dev/null; echo "laptop exit=$?"
diff <(ls node1/ddb) <(ls node2/ddb) && echo "same files on both nodes"'
$SB note "$F" "Operator follow-up. The kept file has no frontmatter, so the index skips it with a warning, and ddb reindex --strict fails until it gets one. Adding frontmatter with the new id (the file name) and committing makes it a normal doogat."
$SB exec "$F" bash "$H"'
NEW=$(ls node2/ddb | grep -v -e 20260310000000.md -e _typedef -e .gitkeep); ID=${NEW%.md}
$DDB --repo node2 reindex --strict > strict.out 2>&1; echo "strict exit=$?"
grep -c "$NEW" strict.out
{ printf -- "---\nid: %s\ntitle: Desk notes\n---\n" "$ID"; cat "node2/ddb/$NEW"; } > fixed.md && mv fixed.md "node2/ddb/$NEW"
git -C node2 add ddb && git -C node2 commit -qm "fix: add frontmatter to kept loser"
$DDB --repo node2 reindex --strict >/dev/null; echo "strict exit=$?"
$DDB --repo node2 read "$ID" | sed -n 3p'
$SB note "$F" "Limits. Only a loser with no frontmatter block at all is kept verbatim; a block whose YAML is invalid still aborts the whole merge, as before. A body with two '---' lines counts as a block and still takes the id-rewrite path (known residual, tracked in PRD 00204's follow-ups). FFI has no sync call, so only the CLI and the GraphQL sync mutation (collisionLosersKeptVerbatim) show these records; ddb bundle import prints the same lines. Contract: docs/src/technical/sync.md. Tests: tests/e2e/collision.rs."
echo RECORDED
