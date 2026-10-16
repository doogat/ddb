# Bundle import: per-payload ref namespaces

*2026-09-28T20:23:08Z by Showboat 0.6.1*
<!-- showboat-id: e6cc4c70-1468-4608-bae2-a1e2c1ba3990 -->

Each bundle import now fetches into its own namespace, refs/remotes/bundle/<payload-id>/, where the payload id is the SHA-256 of the bundle's Git payload. A failed import keeps its refs for a retry. A later bundle without a master fails loudly instead of merging that stale master. A successful import deletes only its own namespace. This demo runs the full recovery path with the ddb CLI, in a throwaway temp dir: failed A, masterless B refused, independent C succeeds, repaired retry of A, no-op reimport. Every block starts by entering the same temp dir. Logs are off. Payload ids, OIDs and node UUIDs are replaced with placeholders so the output is stable. There is no abort command: removing a kept namespace is manual operator recovery.

```bash
set -eu; export RUST_LOG=off; DDB=/Users/bob/git/src/github.com/doogat/ddb/target/debug/ddb
W=$(mktemp -d /tmp/ddb-walk-00203.XXXXXX); echo "$W" > /tmp/ddb-walk-00203.path; cd "$W"
$DDB init node1 >/dev/null
git -C node1 config commit.gpgsign false
mkdir -p node1/ddb
printf -- '---\nid: 20260301000000\ntitle: Ancestor\n---\nShared body\n' > node1/ddb/20260301000000.md
git -C node1 add ddb && git -C node1 commit -qm 'test: add ancestor' -m 'HLC: 1000-0000-ancestor'
git clone -q node1 node2
git -C node2 config commit.gpgsign false
git -C node2 config fetch.prune true
$DDB --repo node1 register-node Laptop | sed -E 's/\([0-9a-f-]{36}\)/(<uuid>)/'
$DDB --repo node2 register-node Desktop | sed -E 's/\([0-9a-f-]{36}\)/(<uuid>)/'
```

```output
registered node Laptop (<uuid>)
registered node Desktop (<uuid>)
```

The nodes diverge. Laptop adds doogat 20260303000000 ('Imported A') and a frontmatter-less file at ddb/20260302000000.md. Desktop adds a proper doogat at the same path with a later HLC, so Laptop's file loses the add/add collision and cannot be renamed. Laptop exports bundle A.

```bash
set -eu; export RUST_LOG=off; DDB=/Users/bob/git/src/github.com/doogat/ddb/target/debug/ddb; cd "$(< /tmp/ddb-walk-00203.path)"
printf -- '---\nid: 20260303000000\ntitle: Imported A\n---\nA body\n' > node1/ddb/20260303000000.md
printf 'Just a plain body with no frontmatter block at all.\n' > node1/ddb/20260302000000.md
git -C node1 add ddb && git -C node1 commit -qm 'test: add laptop side' -m 'HLC: 2000-0000-laptopxx'
printf -- '---\nid: 20260302000000\ntitle: Winner\n---\nWinner body\n' > node2/ddb/20260302000000.md
git -C node2 add ddb && git -C node2 commit -qm 'test: add desktop side' -m 'HLC: 3000-0000-desktopx'
$DDB --repo node1 bundle export --full --output a.bundle.tar | sed "s#$PWD/##"
```

```output
exported full bundle to a.bundle.tar
```

Desktop imports A. The merge cannot rename the losing file, so the import fails and keeps A's fetched refs under A's own namespace for a retry.

```bash
set -u; export RUST_LOG=off; DDB=/Users/bob/git/src/github.com/doogat/ddb/target/debug/ddb; cd "$(< /tmp/ddb-walk-00203.path)"
$DDB --repo node2 bundle import a.bundle.tar > a.out 2>&1; echo "exit=$?"
git -C node2 for-each-ref --format='%(refname)' refs/remotes/bundle/ | sed -E 's#^refs/remotes/bundle/([0-9a-f]{64})/.*#\1#' > a.id
sed -E "s/$(< a.id)/<payload-A>/g; s/[0-9a-f]{40}/<oid>/g" a.out
git -C node2 for-each-ref --format='%(refname) %(objectname)' refs/remotes/bundle/ | sed -E "s/$(< a.id)/<payload-A>/; s/ $(git -C node1 rev-parse HEAD)\$/ = Laptop master/"
```

```output
exit=1
error: sync: bundle merge failed: conflict: collision loser at ddb/20260302000000.md (old id 20260302000000) could not be rewritten: parse: no frontmatter opening --- (fetched refs kept under refs/remotes/bundle/<payload-A>/)
refs/remotes/bundle/<payload-A>/master = Laptop master
```

Desktop repairs the collision locally by committing A's exact file content, without importing A. A's kept master would now merge cleanly. Before this fix, a later import that delivered no master merged exactly that stale master and reported success.

```bash
set -eu; export RUST_LOG=off; cd "$(< /tmp/ddb-walk-00203.path)"
printf 'Just a plain body with no frontmatter block at all.\n' > node2/ddb/20260302000000.md
git -C node2 add ddb && git -C node2 commit -qm 'test: repair collision locally' -m 'HLC: 4000-0000-desktopx'
git -C node2 rev-parse HEAD > head-before-b
git -C node2 merge-tree --write-tree HEAD "$(git -C node1 rev-parse HEAD)" >/dev/null && echo 'stale A master now merges cleanly'
```

```output
stale A master now merges cleanly
```

A third repo exports bundle B, whose only branch is sidecar (no master). Desktop imports B. B is refused as a merge failure that names B's payload id and its delivered branches. HEAD does not move, A's doogat does not land, A's kept ref is unchanged, and B's fetched head is kept in B's own namespace.

```bash
set -u; export RUST_LOG=off; DDB=/Users/bob/git/src/github.com/doogat/ddb/target/debug/ddb; cd "$(< /tmp/ddb-walk-00203.path)"
$DDB init node3 >/dev/null && git -C node3 config commit.gpgsign false && mkdir -p node3/ddb
printf -- '---\nid: 20260401000000\ntitle: Sidecar\n---\nSidecar body\n' > node3/ddb/20260401000000.md
git -C node3 add ddb && git -C node3 commit -qm 'test: add sidecar' -m 'HLC: 1000-0000-sidecarx'
$DDB --repo node3 register-node Sidecar >/dev/null && git -C node3 branch -m sidecar
$DDB --repo node3 bundle export --full --output b.bundle.tar >/dev/null
$DDB --repo node2 bundle import b.bundle.tar > b.out 2>&1; echo "exit=$?"
N="s/$(< a.id)/<payload-A>/g; s/[0-9a-f]{64}/<payload-B>/g"
sed -E "$N" b.out
[ "$(git -C node2 rev-parse HEAD)" = "$(< head-before-b)" ] && echo 'HEAD unchanged'
git -C node2 ls-tree --name-only HEAD ddb/
git -C node2 for-each-ref --format='%(refname)' refs/remotes/bundle/ | sed -E "$N" | LC_ALL=C sort
```

```output
exit=1
error: sync: bundle merge failed: bundle payload <payload-B> delivered no master branch (delivered: ["sidecar"]); fetched refs kept under refs/remotes/bundle/<payload-B>/
HEAD unchanged
ddb/.gitkeep
ddb/20260301000000.md
ddb/20260302000000.md
refs/remotes/bundle/<payload-A>/master
refs/remotes/bundle/<payload-B>/sidecar
```

An independent two-branch bundle C (a clone of Desktop with one new doogat) imports cleanly. It deletes only its own namespace; A's and B's kept refs survive.

```bash
set -u; export RUST_LOG=off; DDB=/Users/bob/git/src/github.com/doogat/ddb/target/debug/ddb; cd "$(< /tmp/ddb-walk-00203.path)"
git clone -q node2 node4 && git -C node4 config commit.gpgsign false
$DDB --repo node4 register-node Courier >/dev/null
printf -- '---\nid: 20260305000000\ntitle: Imported C\n---\nC body\n' > node4/ddb/20260305000000.md
git -C node4 add ddb && git -C node4 commit -qm 'test: add courier doogat' -m 'HLC: 5000-0000-courierx'
git -C node4 branch feature
$DDB --repo node4 bundle export --full --output c.bundle.tar >/dev/null
$DDB --repo node2 bundle import c.bundle.tar; echo "exit=$?"
$DDB --repo node2 read 20260305000000
N="s/$(< a.id)/<payload-A>/g; s/[0-9a-f]{64}/<payload-B>/g"
git -C node2 for-each-ref --format='%(refname)' refs/remotes/bundle/ | sed -E "$N" | LC_ALL=C sort
```

```output
imported: conflicts resolved: 0
exit=0
---
id: 20260305000000
title: Imported C
---
C body
refs/remotes/bundle/<payload-A>/master
refs/remotes/bundle/<payload-B>/sidecar
```

Retrying the same bundle A now succeeds, because the collision was repaired. The retry reuses A's namespace (same Git payload, same id), merges A, and deletes only A's namespace. B's kept ref stays until someone deals with it by hand.

```bash
set -u; export RUST_LOG=off; DDB=/Users/bob/git/src/github.com/doogat/ddb/target/debug/ddb; cd "$(< /tmp/ddb-walk-00203.path)"
$DDB --repo node2 bundle import a.bundle.tar; echo "exit=$?"
git -C node2 merge-base --is-ancestor "$(git -C node1 rev-parse HEAD)" HEAD && echo 'A merged into Desktop'
$DDB --repo node2 read 20260303000000
N="s/$(< a.id)/<payload-A>/g; s/[0-9a-f]{64}/<payload-B>/g"
git -C node2 for-each-ref --format='%(refname)' refs/remotes/bundle/ | sed -E "$N" | LC_ALL=C sort
```

```output
imported: conflicts resolved: 0
exit=0
A merged into Desktop
---
id: 20260303000000
title: Imported A
---
A body
refs/remotes/bundle/<payload-B>/sidecar
```

Reimporting A is the existing no-op: already up to date, zero conflicts, HEAD and the kept refs unchanged.

```bash
set -u; export RUST_LOG=off; DDB=/Users/bob/git/src/github.com/doogat/ddb/target/debug/ddb; cd "$(< /tmp/ddb-walk-00203.path)"
H=$(git -C node2 rev-parse HEAD)
$DDB --repo node2 bundle import a.bundle.tar; echo "exit=$?"
[ "$(git -C node2 rev-parse HEAD)" = "$H" ] && echo 'HEAD unchanged'
N="s/$(< a.id)/<payload-A>/g; s/[0-9a-f]{64}/<payload-B>/g"
git -C node2 for-each-ref --format='%(refname)' refs/remotes/bundle/ | sed -E "$N" | LC_ALL=C sort
```

```output
imported: conflicts resolved: 0
exit=0
HEAD unchanged
refs/remotes/bundle/<payload-B>/sidecar
```

Limits. There is no abort command: B's namespace stays until an operator removes that one namespace by hand; its recovery API is deferred. Serializing imports does not close 00168's separate write-lease finding. The same workflow runs through the public FFI driver in ddb-core/src/ffi/tests.rs (bundle_import_ffi_*) and through the CLI in tests/e2e/integration_bundle_import_ref_namespace.rs. Contract: docs/src/technical/bundle.md.
