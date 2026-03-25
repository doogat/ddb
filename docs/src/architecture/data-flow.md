# Data Flow

## Creating a Doogat

```text
ddb create --title "Example" --tags "tag1,tag2"
  │
  v
parser::generate_id()  ──>  DoogatId("20260226153042")
  │
  v
parser::serialize()  ──>  Markdown string with frontmatter
  │
  v
git_ops::commit_file("ddb/20260226153042.md", ...)
  │
  v
indexer::index_doogat()  ──>  Upsert into SQLite + FTS5
  │
  v
stdout: "20260226153042"
```

## Reading a Doogat

```text
ddb read 20260226153042
  │
  v
git_ops::read_file("ddb/20260226153042.md")
  │
  v
stdout: raw Markdown content
```

## Updating a Doogat

```text
ddb update 20260226153042 --title "New Title"
  │
  v
git_ops::read_file()  ──>  existing content
  │
  v
parser::parse()  ──>  ParsedDoogat
  │
  v
modify fields on ParsedDoogat
  │
  v
parser::serialize()  ──>  updated Markdown
  │
  v
git_ops::commit_file()  ──>  new Git commit
  │
  v
indexer::index_doogat()  ──>  update SQLite
```

## Searching

```text
ddb search "learning"
  │
  v
indexer::is_stale()?  ──>  if yes: rebuild from Git
  │
  v
indexer::search("learning")
  │  FTS5 MATCH query with snippet generation
  │  ORDER BY rank (lower = better match)
  v
stdout: results with highlighted snippets
```

## Syncing Two Nodes

This is the most complex flow. Suppose Node B syncs after both A and B have made edits:

```text
Node B: ddb sync origin master
  │
  v
git_ops::fetch("origin", "master")
  │
  v
git_ops::merge_remote("origin", "master")
  │
  ├── AlreadyUpToDate ──> done
  ├── FastForward ──> update ref, checkout
  ├── Clean ──> auto-commit merge
  └── Conflicts ──> extract ConflictFile list
        │
        v
  crdt_resolver::resolve_conflicts()
        │  For each ConflictFile:
        │    split_zones() on ancestor, ours, theirs
        │    merge_frontmatter()  ──>  Automerge map
        │    merge_body()         ──>  Automerge text (char-level)
        │    merge_reference()    ──>  Automerge List CRDT
        │    reassemble via parser::serialize()
        v
  git_ops::commit_merge(resolved_files, theirs_oid)
        │  Creates merge commit with two parents
        v
  git_ops::push("origin", "master")
        │
        v
  sync_manager::update_sync_state()
        │  known_heads = [current HEAD]
        │  last_sync = now
        │  commit .nodes/{uuid}.toml
        v
  git_ops::push()  ──>  propagate node registry
        │
        v
  indexer::rebuild()  ──>  reindex all doogats
        │
        v
  stdout: SyncReport { conflicts_resolved: N, ... }
```

After this, when Node A syncs, it fast-forwards to the resolved commit.

## Compaction

```text
ddb compact
  │
  v
sync_manager::list_nodes()  ──>  all NodeConfig entries
  │
  v
compaction::shared_head()
  │  Iteratively compute merge-base across all nodes' known_heads
  │  This is the latest commit all nodes have synced
  v
compaction::cleanup_crdt_temp()
  │  Remove temporary CRDT files from .crdt/temp/
  v
compaction::run_gc()
  │  Execute `git gc` for pack consolidation
  v
stdout: CompactionReport { files_removed, gc_success }
```
