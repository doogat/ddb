# Declarative schema apply

*2026-06-25T06:59:25Z by Showboat 0.6.1*
<!-- showboat-id: 0b05bef2-8cb5-4a67-8b76-2ac63b78d951 -->

Declare your typedefs once in a YAML document and apply them with one command. ddb diffs the desired schema against the live schema and runs the minimal safe migration, with a dry-run plan you can preview first. This replaces hand-written, ordered CREATE/ALTER sequences and the per-app idempotency logic that goes with them.

Initialize a fresh repo in a temp directory.

```bash
rm -rf /tmp/ddb-schema-walkthrough && mkdir -p /tmp/ddb-schema-walkthrough && cd /tmp/ddb-schema-walkthrough && RUST_LOG=error /Users/bob/git/src/github.com/doogat/ddb/target/debug/ddb init .
```

```output
initialized ddb at .
```

Declare the desired schema: a contact type with a frontmatter email column and a body notes column. The vocabulary is the same one a typedef or CREATE TABLE uses.

```bash
cd /tmp/ddb-schema-walkthrough && cat > contact.yaml <<'YAML'
types:
  - name: contact
    columns:
      - name: email
        data_type: VARCHAR(255)
        zone: frontmatter
        required: true
      - name: notes
        data_type: TEXT
        zone: body
YAML
cat contact.yaml
```

```output
types:
  - name: contact
    columns:
      - name: email
        data_type: VARCHAR(255)
        zone: frontmatter
        required: true
      - name: notes
        data_type: TEXT
        zone: body
```

Preview the migration with a dry-run. ddb schema diff is the alias for apply --dry-run. It prints the ordered plan and mutates nothing. The new type is not created yet.

```bash
cd /tmp/ddb-schema-walkthrough && RUST_LOG=error /Users/bob/git/src/github.com/doogat/ddb/target/debug/ddb schema diff contact.yaml
```

```output
create_type contact destructive=false -- create type contact -- CREATE TABLE contact (email VARCHAR(255) NOT NULL ZONE frontmatter, notes TEXT ZONE body)
```

Apply the schema. On an empty repo this creates every declared type. Apply is atomic: the whole plan lands as a single git commit, or rolls back on any failure.

```bash
cd /tmp/ddb-schema-walkthrough && RUST_LOG=error /Users/bob/git/src/github.com/doogat/ddb/target/debug/ddb schema apply contact.yaml
```

```output
create_type contact destructive=false -- create type contact -- CREATE TABLE contact (email VARCHAR(255) NOT NULL ZONE frontmatter, notes TEXT ZONE body)
```

Re-apply the same document. Apply is idempotent: the live schema already matches, so there is nothing to do.

```bash
cd /tmp/ddb-schema-walkthrough && RUST_LOG=error /Users/bob/git/src/github.com/doogat/ddb/target/debug/ddb schema apply contact.yaml
```

```output
no changes
```

Now reduce the schema. Leaving notes out of the desired document means the plan would drop it, which is destructive.

```bash
cd /tmp/ddb-schema-walkthrough && cat > contact_min.yaml <<'YAML'
types:
  - name: contact
    columns:
      - name: email
        data_type: VARCHAR(255)
        zone: frontmatter
        required: true
YAML
RUST_LOG=error /Users/bob/git/src/github.com/doogat/ddb/target/debug/ddb schema diff contact_min.yaml
```

```output
drop_column contact destructive=true -- drop column notes from contact -- ALTER TABLE contact DROP COLUMN notes
```

Without --allow-destructive the apply is refused before any mutation, so the notes column stays put.

```bash
cd /tmp/ddb-schema-walkthrough && RUST_LOG=error /Users/bob/git/src/github.com/doogat/ddb/target/debug/ddb schema apply contact_min.yaml 2>&1 || true
```

```output
error: schema plan contains destructive operations (drop/rename); re-run with allow_destructive
```

Pass --allow-destructive to permit drops and renames. The notes column is removed and the change commits.

```bash
cd /tmp/ddb-schema-walkthrough && RUST_LOG=error /Users/bob/git/src/github.com/doogat/ddb/target/debug/ddb schema apply contact_min.yaml --allow-destructive
```

```output
drop_column contact destructive=true -- drop column notes from contact -- ALTER TABLE contact DROP COLUMN notes
```

Clean up the demo repo.

```bash
rm -rf /tmp/ddb-schema-walkthrough
```

```output
```
