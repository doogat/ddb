# Inline column ZONE at CREATE TABLE

*2026-06-22T11:00:56Z by Showboat 0.6.1*
<!-- showboat-id: 2cdf71ca-98a9-486c-9455-0c9ceadb56ad -->

Declare a column's storage zone inline in CREATE TABLE, instead of a post-hoc ALTER TABLE ... SET ZONE. A plain TEXT column defaults to the body zone; appending ZONE frontmatter overrides that, so the column's value materializes into the doogat's YAML frontmatter.

Initialize a fresh repo in a temp directory.

```bash
rm -rf /tmp/ddb-zone-walkthrough && mkdir -p /tmp/ddb-zone-walkthrough && cd /tmp/ddb-zone-walkthrough && RUST_LOG=error /Users/bob/git/src/github.com/doogat/ddb/target/debug/ddb init .
```

```output
initialized ddb at .
```

Create a table that declares summary's zone inline as frontmatter. body_note keeps the default (body) zone.

```bash
cd /tmp/ddb-zone-walkthrough && RUST_LOG=error /Users/bob/git/src/github.com/doogat/ddb/target/debug/ddb query "CREATE TABLE note (summary TEXT ZONE frontmatter, body_note TEXT)"
```

```output
table note created
```

Insert a row. The printed doogat id is non-deterministic, so we discard it.

```bash
cd /tmp/ddb-zone-walkthrough && RUST_LOG=error /Users/bob/git/src/github.com/doogat/ddb/target/debug/ddb query "INSERT INTO note (summary, body_note) VALUES ('quick recap', 'long form body text')" >/dev/null && echo inserted
```

```output
inserted
```

The declared zone took effect: summary materialized into the YAML frontmatter, where it shows up as a 'summary:' key. A default body-zone TEXT column would not produce that key.

```bash
cd /tmp/ddb-zone-walkthrough && grep -h 'summary:' ddb/*.md
```

```output
summary: quick recap
```

body_note kept the default body zone, so its value lives in the markdown body, not the frontmatter.

```bash
cd /tmp/ddb-zone-walkthrough && grep -h 'long form body text' ddb/*.md
```

```output
long form body text
```

The zone is stored on the typedef identically to ALTER TABLE ... SET ZONE, so it survives a reindex: a row inserted after reindex still zones summary into frontmatter.

```bash
cd /tmp/ddb-zone-walkthrough && RUST_LOG=error /Users/bob/git/src/github.com/doogat/ddb/target/debug/ddb reindex >/dev/null && RUST_LOG=error /Users/bob/git/src/github.com/doogat/ddb/target/debug/ddb query "INSERT INTO note (summary, body_note) VALUES ('after reindex', 'more body')" >/dev/null && grep -h 'summary: after reindex' ddb/*.md
```

```output
summary: after reindex
```

Clean up the demo repo.

```bash
rm -rf /tmp/ddb-zone-walkthrough
```

```output
```
