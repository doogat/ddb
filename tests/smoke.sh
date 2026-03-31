#!/usr/bin/env bash
set -euo pipefail

# Smoke test: fast CLI validation (init, CRUD, search, SQL, types, compact).
# For full integration tests (server, sync, CRDT), run tests/integration.sh.

# Build when DDB_BIN is not injected.
PREP_LABEL="prebuilt binary"
if [ -z "${DDB_BIN:-}" ]; then
  cargo build --quiet
  PREP_LABEL="build"
fi
DDB="${DDB_BIN:-$(cargo metadata --format-version=1 --no-deps | sed -n 's/.*"target_directory":"\([^"]*\)".*/\1/p')/debug/ddb}"

# Work in temp directories, clean up on exit
TMPDIR="$(mktemp -d)"
trap 'rm -rf "$TMPDIR"' EXIT
cd "$TMPDIR"

pass() { printf '  ✓ %s\n' "$1"; }

echo "=== smoke test ==="

pass "$PREP_LABEL"

# 1. init
$DDB init . >/dev/null
pass "init"

# 2. create doogats (no sleeps — tests cross-process ID uniqueness)
ID1=$($DDB create --title "First note" --tags "test,smoke" --body "Hello world")
ID2=$($DDB create --title "Links to first" --body "See [[$ID1]]")
ID3=$($DDB create --title "Project Alpha" --type project --tags "active" --body "A project doogat")
[ "$ID1" != "$ID2" ] && [ "$ID2" != "$ID3" ] && [ "$ID1" != "$ID3" ]
pass "create (3 unique IDs: $ID1 $ID2 $ID3)"

# 3. read
OUTPUT=$($DDB read "$ID1")
echo "$OUTPUT" | grep -q "First note"
pass "read"

# 4. update
$DDB update "$ID1" --title "First note (edited)" --tags "test,smoke,updated"
$DDB read "$ID1" | grep -q "First note (edited)"
pass "update"

# 5. delete
$DDB delete "$ID3"
! $DDB read "$ID3" 2>/dev/null
! $DDB delete "99999999999999" 2>/dev/null
pass "delete"

# 6. status
$DDB status | grep -q "^head:"
pass "status"

# 6b. broken backlink report on delete
BL_TARGET=$($DDB create --title "Backlink Target" --body "I will be deleted")
sleep 1
BL_SOURCE=$($DDB create --title "Backlink Source" --body "See [[$BL_TARGET]]")
$DDB reindex >/dev/null
$DDB delete "$BL_TARGET" 2>&1 | grep -q "broken backlinks"
$DDB status 2>/dev/null | grep -q "broken backlinks"
# Clean up: delete source so broken backlinks don't affect later tests
$DDB delete "$BL_SOURCE" >/dev/null 2>&1
pass "broken backlink report on delete"

# 7. reindex
$DDB reindex | grep -q "indexed 2 doogats"
pass "reindex"

# 7b. hashtag extraction
$DDB update "$ID1" --body "Updated with #gtd/act/next hashtag"
$DDB reindex >/dev/null
$DDB query "SELECT tag, source FROM _ddb_tags WHERE tag = 'gtd/act/next'" | grep -q "body"
pass "hashtag extraction and indexing"

# 7c. checkbox parsing
$DDB update "$ID1" --body "- [ ] open task\n- [x] done task\n- [i] 2026-01-01 10:00 - info note"
$DDB reindex >/dev/null
$DDB query "SELECT state, content FROM _ddb_checkboxes WHERE state = 'open'" | grep -q "open task"
pass "checkbox parsing and indexing"

# 7d. folder namespace
$DDB query "CREATE TABLE widget (color TEXT)" >/dev/null
# Add folder: true to the widget typedef
WIDGET_TYPEDEF=$(find "$TMPDIR/ddb/_typedef" -name "*.md" -exec grep -l "title: widget" {} \;)
sed -i.bak 's/type: _typedef/type: _typedef\nfolder: true/' "$WIDGET_TYPEDEF" && rm -f "${WIDGET_TYPEDEF}.bak"
git -C "$TMPDIR" add -A && git -C "$TMPDIR" commit -m "add folder to widget" >/dev/null
$DDB reindex >/dev/null
WIDGET_ID=$($DDB query "INSERT INTO widget (color) VALUES ('red')")
test -f "$TMPDIR/ddb/widget/${WIDGET_ID}.md"
pass "folder namespace: typed doogat in subdirectory"

# 8. full-text search
$DDB search "First note" | grep -q "$ID1"
pass "search"

# 8b. paginated search
$DDB search "First note" --limit 1 --offset 0 | grep -q "Showing 1-1 of"
pass "paginated search"

# 9. SQL queries
$DDB query "SELECT id, title FROM doogats" | grep -q "First note (edited)"
$DDB query "SELECT z.id, z.title FROM doogats z JOIN _ddb_tags t ON t.doogat_id = z.id WHERE t.tag LIKE '%smoke%'" | grep -q "$ID1"
pass "sql queries"

# 10. wikilinks
$DDB query "SELECT * FROM _ddb_links" | grep -q "$ID1"
pass "wikilinks"

# 10a. link kinds (wikilink, markdown, embed, url)
LKBODY=$(printf 'See [[%s]] wiki.\n[md link](target.md)\n![[%s]]\nhttps://example.com' "$ID1" "$ID2")
LK_ID=$($DDB create --title "Link Kinds" --body "$LKBODY")
$DDB reindex >/dev/null
LK_OUT=$($DDB query "SELECT kind FROM _ddb_links WHERE source_id = '$LK_ID' ORDER BY kind")
echo "$LK_OUT" | grep -q "url"
echo "$LK_OUT" | grep -q "embed"
echo "$LK_OUT" | grep -q "markdown"
echo "$LK_OUT" | grep -q "wikilink"
pass "link kinds (4 types indexed)"

# 10b. rename with backlink rewrite
RENAME_TARGET=$($DDB create --title "Rename Target" --body "I will move.")
$DDB create --title "Rename Linker" --body "See [[$RENAME_TARGET|Target]]." >/dev/null
$DDB reindex >/dev/null
$DDB rename "$RENAME_TARGET" "ddb/contact/${RENAME_TARGET}.md" | grep -q "1 backlinks updated"
[ -f "ddb/contact/${RENAME_TARGET}.md" ]
pass "rename with backlink rewrite"

# 11. SQL DDL/DML
$DDB query "CREATE TABLE foo (bar TEXT, baz INTEGER)" | grep -q "table foo created"
FOO_ID=$($DDB query "INSERT INTO foo (title, bar, baz) VALUES ('test row', 'hello', 42)")
echo "$FOO_ID" | grep -qE "^[0-9]{14}$"
$DDB query "SELECT bar, baz FROM foo" | grep -q "hello"
$DDB query "UPDATE foo SET baz = 99 WHERE id = '$FOO_ID'" | grep -q "1 row(s) affected"
$DDB query "SELECT baz FROM foo WHERE id = '$FOO_ID'" | grep -q "99"
$DDB query "DELETE FROM foo WHERE id = '$FOO_ID'" | grep -q "1 row(s) affected"
pass "sql ddl/dml"

# 11a. ALTER TABLE SET ZONE and TITLE TEMPLATE
$DDB query "ALTER TABLE foo SET ZONE frontmatter FOR bar" | grep -q "zone set to frontmatter"
$DDB query "ALTER TABLE foo SET TITLE TEMPLATE 'my-template'" | grep -q "title template set"
$DDB query "ALTER TABLE foo DROP TITLE TEMPLATE" | grep -q "title template dropped"
pass "alter table zone overrides and title template"

# 11b. CREATE TABLE IF NOT EXISTS (idempotent)
$DDB query "CREATE TABLE IF NOT EXISTS foo (bar TEXT, baz INTEGER)" | grep -q "already exists"
$DDB query "CREATE TABLE IF NOT EXISTS newifne (x TEXT)" | grep -q "table newifne created"
$DDB query "CREATE TABLE IF NOT EXISTS newifne (x TEXT)" | grep -q "already exists"
pass "create table if not exists (idempotent)"

# 12. install bundled type
$DDB type install contact | grep -q "installed type"
pass "type install"

# 12a. hyphenated type SQL (quoted identifiers)
$DDB type install meeting-minutes | grep -q "installed type"
HYP_ID=$($DDB query 'INSERT INTO "meeting-minutes" (date, attendees) VALUES ('\''2026-03-10'\'', '\''alice,bob'\'')' | tr -d '[:space:]')
$DDB query "SELECT date FROM \"meeting-minutes\" WHERE id = '$HYP_ID'" | grep -q "2026-03-10"
$DDB query "DELETE FROM \"meeting-minutes\" WHERE id = '$HYP_ID'" | grep -q "1 row(s) affected"
pass "hyphenated type sql (quoted identifiers)"

# 13. type suggest
$DDB query "INSERT INTO foo (title, bar, baz) VALUES ('for suggest', 'val', 1)" >/dev/null
$DDB type suggest foo | grep -q "bar"
pass "type suggest"

# 14. register node + compact
$DDB register-node "smoke-test-laptop" | grep -q "registered node"
$DDB status | grep -q "registered nodes: 1"
COMPACT_OUT=$($DDB compact --force)
echo "$COMPACT_OUT" | grep -q "backup:"
echo "$COMPACT_OUT" | grep -q "gc: ok"
echo "$COMPACT_OUT" | grep -q "crdt temp:"
echo "$COMPACT_OUT" | grep -q "repo (.git):"
pass "register-node + compact"

# 15. node list + retire
$DDB node list | grep -q "smoke-test-laptop"
NODE_UUID=$($DDB node list | grep "smoke-test-laptop" | awk '{print $1}')
$DDB node retire "$NODE_UUID" | grep -q "retired node"
pass "node list + retire"

# 16. compact --dry-run
DRYRUN_OUT=$($DDB compact --dry-run)
echo "$DRYRUN_OUT" | grep -q "dry run"
echo "$DRYRUN_OUT" | grep -q "backup would write:"
pass "compact --dry-run"

# 16a. compact --no-backup
$DDB register-node "no-backup-test" >/dev/null
NOBACKUP_OUT=$($DDB compact --no-backup --force)
echo "$NOBACKUP_OUT" | grep -q "gc: ok"
# Should NOT contain backup path
if echo "$NOBACKUP_OUT" | grep -q "backup:"; then
  echo "FAIL: --no-backup should suppress backup" >&2; exit 1
fi
pass "compact --no-backup"

# 16b. compact --backup-path
CUSTOM_BACKUP="$TMPDIR/custom-backup.bundle.tar"
BKPATH_OUT=$($DDB compact --force --backup-path "$CUSTOM_BACKUP")
echo "$BKPATH_OUT" | grep -q "backup:"
echo "$BKPATH_OUT" | grep -q "$CUSTOM_BACKUP"
[ -f "$CUSTOM_BACKUP" ]
pass "compact --backup-path"

# 16c. maintenance
$DDB maintenance run | grep -q "maintenance:"
pass "maintenance run"

MAINT_STATUS=$($DDB maintenance auto status)
echo "$MAINT_STATUS" | grep -q "off"
pass "maintenance auto status (default off)"

$DDB maintenance auto on | grep -q "enabled"
$DDB maintenance auto status | grep -q "on"
pass "maintenance auto on"

$DDB maintenance auto off | grep -q "disabled"
$DDB maintenance auto status | grep -q "off"
pass "maintenance auto off"

# 16d. discover
$DDB discover stale >/dev/null
pass "discover stale"

$DDB discover orphans | head -1 | grep -q "."
pass "discover orphans"

# Create a doogat that mentions ID1's title without linking
MENTION_ID=$($DDB create --title "Review notes" --body "About First note (edited) topic")
$DDB reindex >/dev/null
$DDB discover mentions "$ID1" | grep -q "$MENTION_ID"
pass "discover mentions"

$DDB discover similar "$ID1" | head -1 | grep -q "."
pass "discover similar"

# 16e. consistency fix
FIX_ID=$($DDB create --title "Fix Test" --tags "#gtd,zebra,apple")
BEFORE_HEAD=$(git rev-parse HEAD)
$DDB fix --dry-run | grep -q "would fix"
[ "$(git rev-parse HEAD)" = "$BEFORE_HEAD" ]
pass "fix dry-run"

$DDB fix | grep -q "fixed"
pass "fix apply"

$DDB fix | grep -q "no issues"
pass "fix idempotent"

$DDB read "$FIX_ID" | grep -q "  - apple"
pass "fix result verified"

# 16f. sequence navigation
SEQ_ROOT=$($DDB create --title "Seq Root")
# Patch child doogat to have sequence field
SEQ_CHILD1=$($DDB create --title "Seq Child 1")
SEQ_CHILD1_PATH="ddb/${SEQ_CHILD1}.md"
cat > "$SEQ_CHILD1_PATH" <<SEQEOF
---
id: $SEQ_CHILD1
title: Seq Child 1
sequence: $SEQ_ROOT
---

SEQEOF
git add "$SEQ_CHILD1_PATH"
git commit -m "add sequence field" --quiet

SEQ_CHILD2=$($DDB create --title "Seq Child 2")
SEQ_CHILD2_PATH="ddb/${SEQ_CHILD2}.md"
cat > "$SEQ_CHILD2_PATH" <<SEQEOF
---
id: $SEQ_CHILD2
title: Seq Child 2
sequence: $SEQ_ROOT
---

SEQEOF
git add "$SEQ_CHILD2_PATH"
git commit -m "add sequence field" --quiet

$DDB reindex >/dev/null
$DDB sequence tree "$SEQ_ROOT" | grep -q "$SEQ_CHILD1"
pass "sequence tree"

$DDB sequence breadcrumb "$SEQ_CHILD1" | grep -q "$SEQ_ROOT"
pass "sequence breadcrumb"

# Broken sequence ref
SEQ_BROKEN=$($DDB create --title "Seq Broken")
SEQ_BROKEN_PATH="ddb/${SEQ_BROKEN}.md"
cat > "$SEQ_BROKEN_PATH" <<SEQEOF
---
id: $SEQ_BROKEN
title: Seq Broken
sequence: "99999999999999"
---

SEQEOF
git add "$SEQ_BROKEN_PATH"
git commit -m "broken sequence ref" --quiet
$DDB reindex >/dev/null
$DDB sequence broken | grep -q "not found"
pass "sequence broken"

# --log-level flag accepted
$DDB --log-level debug status >/dev/null 2>&1
pass "--log-level flag accepted"

# help guides (no repo needed)
HELP_OUT=$($DDB help create-app)
echo "$HELP_OUT" | grep -q "CREATE TABLE"
pass "help create-app"
$DDB help | grep -q "create-app"
pass "help list"
! $DDB help nonexistent 2>/dev/null
pass "help unknown fails"

# app-building end-to-end flow
$DDB query "CREATE TABLE abcategory (name VARCHAR(100), priority ENUM('low','medium','high'))" | grep -q "table abcategory created"
AB_CAT_ID=$($DDB query "INSERT INTO abcategory (name, priority) VALUES ('work', 'high')")
echo "$AB_CAT_ID" | grep -qE "^[0-9]{14}$"
$DDB query "CREATE TABLE abbookmark (url VARCHAR(2048), description TEXT, abcategory TEXT REFERENCES abcategory)" | grep -q "table abbookmark created"
$DDB query "ALTER TABLE abbookmark SET ZONE reference FOR url" | grep -q "zone set to reference"
$DDB query "ALTER TABLE abbookmark SET TITLE TEMPLATE '{url}'" | grep -q "title template set"
# Insert with explicit title
sleep 1
AB_BM1=$($DDB query "INSERT INTO abbookmark (title, url, description) VALUES ('Rust Book', 'https://doc.rust-lang.org', 'The official Rust book')")
echo "$AB_BM1" | grep -qE "^[0-9]{14}$"
# Insert with template-derived title (no explicit title)
sleep 1
AB_BM2=$($DDB query "INSERT INTO abbookmark (url, description) VALUES ('https://crates.io', 'Rust package registry')")
echo "$AB_BM2" | grep -qE "^[0-9]{14}$"
# Link both bookmarks to category via junction table
$DDB query "INSERT INTO abbookmark_abcategory (abbookmark_id, abcategory_id) VALUES ('$AB_BM1', '$AB_CAT_ID')" | grep -q "1 row"
$DDB query "INSERT INTO abbookmark_abcategory (abbookmark_id, abcategory_id) VALUES ('$AB_BM2', '$AB_CAT_ID')" | grep -q "1 row"
# SELECT from main table
$DDB query "SELECT url FROM abbookmark" | grep -q "rust-lang"
# SELECT from junction table — both bookmarks linked
$DDB query "SELECT COUNT(*) FROM abbookmark_abcategory" | grep -q "2"
# Verify ENUM allowed_values stored in typedef
$DDB query "SELECT priority FROM abcategory" | grep -q "high"
# help create-app guide available
$DDB help create-app | grep -q "CREATE TABLE"
# Clean up
$DDB query "DROP TABLE abbookmark CASCADE" | grep -q "dropped"
$DDB query "DROP TABLE abcategory CASCADE" | grep -q "dropped"
pass "app-building end-to-end flow"

# 17. cascade delete
$DDB query "CREATE TABLE cdcategory (label VARCHAR(100))" >/dev/null
$DDB query "CREATE TABLE cdbookmark (url TEXT, cdcategory TEXT REFERENCES cdcategory)" >/dev/null
CAT_ID=$($DDB query "INSERT INTO cdcategory (label) VALUES ('work')")
sleep 1
BM_ID=$($DDB query "INSERT INTO cdbookmark (url) VALUES ('https://example.com')")
$DDB query "INSERT INTO cdbookmark_cdcategory (cdbookmark_id, cdcategory_id) VALUES ('$BM_ID', '$CAT_ID')" >/dev/null
# Verify junction row exists
$DDB query "SELECT COUNT(*) FROM cdbookmark_cdcategory WHERE cdcategory_id = '$CAT_ID'" | grep -q "1"
# Delete the category — should cascade
$DDB query "DELETE FROM cdcategory WHERE id = '$CAT_ID'" >/dev/null
# Junction row should be gone
$DDB query "SELECT COUNT(*) FROM cdbookmark_cdcategory WHERE cdcategory_id = '$CAT_ID'" | grep -q "0"
# Wikilink to deleted category should be removed from bookmark
BM_CONTENT=$($DDB read "$BM_ID")
if echo "$BM_CONTENT" | grep -q "\[\[${CAT_ID}\]\]"; then
  echo "FAIL: wikilink to deleted category still present in bookmark" >&2; exit 1
fi
# Clean up
$DDB query "DROP TABLE cdbookmark CASCADE" >/dev/null
$DDB query "DROP TABLE cdcategory CASCADE" >/dev/null
pass "cascade delete"

# 18. cascade delete via ddb delete (service path)
$DDB query "CREATE TABLE cdcat2 (label VARCHAR(100))" >/dev/null
$DDB query "CREATE TABLE cdbm2 (url TEXT, cdcat2 TEXT REFERENCES cdcat2)" >/dev/null
CAT2_ID=$($DDB query "INSERT INTO cdcat2 (label) VALUES ('svc')")
sleep 1
BM2_ID=$($DDB query "INSERT INTO cdbm2 (url) VALUES ('https://svc.example.com')")
$DDB query "INSERT INTO cdbm2_cdcat2 (cdbm2_id, cdcat2_id) VALUES ('$BM2_ID', '$CAT2_ID')" >/dev/null
# Delete via ddb delete (service path)
$DDB delete "$CAT2_ID" 2>/dev/null
# Junction row should be gone
$DDB query "SELECT COUNT(*) FROM cdbm2_cdcat2 WHERE cdcat2_id = '$CAT2_ID'" | grep -q "0"
# Wikilink removed from bookmark
BM2_CONTENT=$($DDB read "$BM2_ID")
if echo "$BM2_CONTENT" | grep -q "\[\[${CAT2_ID}\]\]"; then
  echo "FAIL: wikilink to deleted category still present (service path)" >&2; exit 1
fi
$DDB query "DROP TABLE cdbm2 CASCADE" >/dev/null
$DDB query "DROP TABLE cdcat2 CASCADE" >/dev/null
pass "cascade delete (service path)"

# 19. boolean consistency in SQL responses
$DDB query "CREATE TABLE booltest (label TEXT, active BOOLEAN)" | grep -q "table booltest created"
$DDB query "INSERT INTO booltest (label, active) VALUES ('on', true)" >/dev/null
sleep 1
$DDB query "INSERT INTO booltest (label, active) VALUES ('off', false)" >/dev/null
# Boolean true should be "true", not "1"
BOOL_TRUE=$($DDB query "SELECT active FROM booltest WHERE active = 1")
echo "$BOOL_TRUE" | grep -q "true"
# Boolean false should be "false", not "0"
BOOL_FALSE=$($DDB query "SELECT active FROM booltest WHERE active = 0")
echo "$BOOL_FALSE" | grep -q "false"
# NULL boolean stays NULL
sleep 1
$DDB query "INSERT INTO booltest (label) VALUES ('none')" >/dev/null
BOOL_NULL=$($DDB query "SELECT active FROM booltest WHERE label = 'none'")
echo "$BOOL_NULL" | grep -q "NULL"
# Mixed columns: only booleans coerced
BOOL_MIX=$($DDB query "SELECT label, active FROM booltest WHERE label = 'on'")
echo "$BOOL_MIX" | grep -q "on"
echo "$BOOL_MIX" | grep -q "true"
$DDB query "DROP TABLE booltest CASCADE" >/dev/null
pass "boolean consistency (SQL responses)"

# 20. type tables are self-contained (no JOIN needed for core columns)
SC_ID=$($DDB query "INSERT INTO foo (title, bar, baz) VALUES ('Self-Contained Test', 'val', 1)")
SC_OUT=$($DDB query "SELECT id, title, date, updated_at, bar FROM foo WHERE id = '$SC_ID'")
echo "$SC_OUT" | grep -q "$SC_ID"
echo "$SC_OUT" | grep -q "Self-Contained Test"
pass "type table self-contained (core columns without JOIN)"
$DDB query "DROP TABLE foo CASCADE" >/dev/null

echo "=== all smoke tests passed ==="
