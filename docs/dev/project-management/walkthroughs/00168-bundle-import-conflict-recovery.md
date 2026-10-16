# Bundle-import conflict recovery

*2026-08-16T00:20:47Z by Showboat 0.6.1*
<!-- showboat-id: 85285a4c-429a-4d37-804f-3e7b07df30e5 -->

A conflicted bundle import now resolves through the real conflict-resolution engine instead of silently reporting false success and pruning the imported data. Two air-gapped nodes (no git remote — pure bundle exchange) diverge on the same doogat; importing the second node's conflicting bundle produces a real merge with an honest conflict count, and re-importing the same bundle is a safe no-op.

```bash
/Users/bob/git/src/github.com/doogat/ddb/target/debug/ddb init . && /Users/bob/git/src/github.com/doogat/ddb/target/debug/ddb register-node Laptop
```

```output
initialized ddb at .
registered node Laptop (85c617d5-8747-418e-bdcb-b8dda60a67c5)
```

```bash
/Users/bob/git/src/github.com/doogat/ddb/target/debug/ddb create --title 'Shared note' --body 'original body' | tee /tmp/ddb-walk-00168-id.txt
```

```output
[2m2026-08-16T00:21:17.182360Z[0m [32m INFO[0m [2mddb_core::indexer::rebuild[0m[2m:[0m rebuild_triggered
[2m2026-08-16T00:21:17.189477Z[0m [32m INFO[0m [2mddb_core::indexer::rebuild[0m[2m:[0m rebuild_complete [3mindexed[0m[2m=[0m0 [3mtables[0m[2m=[0m0 [3mwarnings[0m[2m=[0m0
20260816022117
```

```bash
/Users/bob/git/src/github.com/doogat/ddb/target/debug/ddb bundle export --full --output /tmp/ddb-walk-00168-bootstrap.bundle.tar
```

```output
exported full bundle to /tmp/ddb-walk-00168-bootstrap.bundle.tar
```

Bootstrap node2 from the same bundle so both nodes start from the shared doogat, with no git remote involved anywhere in this walkthrough.

```bash
/Users/bob/git/src/github.com/doogat/ddb/target/debug/ddb init . && /Users/bob/git/src/github.com/doogat/ddb/target/debug/ddb register-node Desktop
```

```output
initialized ddb at .
registered node Desktop (8e6aac98-7903-4a76-9aeb-2281cb4b9cd7)
```

```bash
/Users/bob/git/src/github.com/doogat/ddb/target/debug/ddb bundle import /tmp/ddb-walk-00168-bootstrap.bundle.tar
```

```output
[2m2026-08-16T00:21:51.660366Z[0m [32m INFO[0m [2mddb_core::indexer::rebuild[0m[2m:[0m rebuild_triggered
[2m2026-08-16T00:21:51.666543Z[0m [32m INFO[0m [2mddb_core::indexer::rebuild[0m[2m:[0m rebuild_complete [3mindexed[0m[2m=[0m1 [3mtables[0m[2m=[0m0 [3mwarnings[0m[2m=[0m0
imported: conflicts resolved: 0
```

Node1 edits the title and exports a fresh bundle.

```bash
/Users/bob/git/src/github.com/doogat/ddb/target/debug/ddb update $(cat /tmp/ddb-walk-00168-id.txt) --title 'Bundle Laptop Title' --body 'laptop body'
```

```output
updated 20260816022117
```

```bash
/Users/bob/git/src/github.com/doogat/ddb/target/debug/ddb bundle export --full --output /tmp/ddb-walk-00168-conflict.bundle.tar
```

```output
exported full bundle to /tmp/ddb-walk-00168-conflict.bundle.tar
```

Node2 makes a CONFLICTING local edit to the same doogat, without ever importing node1's update first — this is the air-gapped divergence.

```bash
/Users/bob/git/src/github.com/doogat/ddb/target/debug/ddb update $(cat /tmp/ddb-walk-00168-id.txt) --title 'Bundle Desktop Title' --body 'desktop body'
```

```output
updated 20260816022117
```

Importing node1's bundle now hits a real conflict. It is resolved through the CRDT cascade and reports the true count (1), not a false success.

```bash
/Users/bob/git/src/github.com/doogat/ddb/target/debug/ddb bundle import /tmp/ddb-walk-00168-conflict.bundle.tar
```

```output
[2m2026-08-16T00:22:50.124113Z[0m [32m INFO[0m [2mddb_core::sync_manager[0m[2m:[0m merge_result: conflicts [3mcount[0m[2m=[0m1
[2m2026-08-16T00:22:50.200140Z[0m [32m INFO[0m [2mddb_core::indexer::rebuild[0m[2m:[0m rebuild_triggered
[2m2026-08-16T00:22:50.211568Z[0m [32m INFO[0m [2mddb_core::indexer::rebuild[0m[2m:[0m rebuild_complete [3mindexed[0m[2m=[0m1 [3mtables[0m[2m=[0m0 [3mwarnings[0m[2m=[0m0
imported: conflicts resolved: 1
```

```bash
/Users/bob/git/src/github.com/doogat/ddb/target/debug/ddb read $(cat /tmp/ddb-walk-00168-id.txt)
```

```output
[2m2026-08-16T00:23:02.803297Z[0m [33m WARN[0m [2mddb_core::git_ops[0m[2m:[0m removing orphaned CRDT temp file: 982a85c55e032aa3ad364c339491c74967dc3f00_20260816022117_fm.crdt
---
id: 20260816022117
title: Bundle Desktop Title
date: 2026-08-16
---
laptdesktopp body```
```

Re-importing the same bundle is a safe no-op: its commits are already ancestors of local HEAD, so zero new conflicts are reported.

```bash
/Users/bob/git/src/github.com/doogat/ddb/target/debug/ddb bundle import /tmp/ddb-walk-00168-conflict.bundle.tar
```

```output
[2m2026-08-16T00:23:22.695854Z[0m [32m INFO[0m [2mddb_core::sync_manager[0m[2m:[0m merge_result: up-to-date
[2m2026-08-16T00:23:22.696423Z[0m [32m INFO[0m [2mddb_core::indexer::rebuild[0m[2m:[0m rebuild_triggered
[2m2026-08-16T00:23:22.732167Z[0m [32m INFO[0m [2mddb_core::indexer::rebuild[0m[2m:[0m rebuild_complete [3mindexed[0m[2m=[0m1 [3mtables[0m[2m=[0m0 [3mwarnings[0m[2m=[0m0
imported: conflicts resolved: 0
```
