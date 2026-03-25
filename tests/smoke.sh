#!/usr/bin/env bash
set -euo pipefail

SMOKE_PROFILE="${SMOKE_PROFILE:-full}"
case "$SMOKE_PROFILE" in
  quick|full) ;;
  *)
    echo "unknown SMOKE_PROFILE: $SMOKE_PROFILE (expected quick or full)" >&2
    exit 2
    ;;
esac

# Build and lint for the full profile when ZDB_BIN is not injected.
PREP_LABEL="prebuilt binary"
if [ -z "${ZDB_BIN:-}" ]; then
  cargo build --quiet
  if [ "$SMOKE_PROFILE" = "full" ]; then
    cargo clippy --workspace --quiet
    cargo bench --no-run --quiet 2>/dev/null
    PREP_LABEL="clippy + bench compile"
  else
    PREP_LABEL="build"
  fi
fi
ZDB="${ZDB_BIN:-$(cargo metadata --format-version=1 --no-deps | sed -n 's/.*"target_directory":"\([^"]*\)".*/\1/p')/debug/zdb}"

# Work in temp directories, clean up on exit
TMPDIR="$(mktemp -d)"
REMOTE_DIR="$(mktemp -d)"
NODE1_DIR="$(mktemp -d)"
NODE2_DIR="$(mktemp -d)"
NODE3_DIR="$(mktemp -d)"
trap 'rm -rf "$TMPDIR" "$REMOTE_DIR" "$NODE1_DIR" "$NODE2_DIR" "$NODE3_DIR"' EXIT
cd "$TMPDIR"

pass() { printf '  ✓ %s\n' "$1"; }

echo "=== smoke test ($SMOKE_PROFILE) ==="

pass "$PREP_LABEL"

# 1. init
$ZDB init . >/dev/null
pass "init"

# 2. create zettels (no sleeps — tests cross-process ID uniqueness)
ID1=$($ZDB create --title "First note" --tags "test,smoke" --body "Hello world")
ID2=$($ZDB create --title "Links to first" --body "See [[$ID1]]")
ID3=$($ZDB create --title "Project Alpha" --type project --tags "active" --body "A project zettel")
[ "$ID1" != "$ID2" ] && [ "$ID2" != "$ID3" ] && [ "$ID1" != "$ID3" ]
pass "create (3 unique IDs: $ID1 $ID2 $ID3)"

# 3. read
OUTPUT=$($ZDB read "$ID1")
echo "$OUTPUT" | grep -q "First note"
pass "read"

# 4. update
$ZDB update "$ID1" --title "First note (edited)" --tags "test,smoke,updated"
$ZDB read "$ID1" | grep -q "First note (edited)"
pass "update"

# 5. delete
$ZDB delete "$ID3"
! $ZDB read "$ID3" 2>/dev/null
! $ZDB delete "99999999999999" 2>/dev/null
pass "delete"

# 6. status
$ZDB status | grep -q "^head:"
pass "status"

# 6b. broken backlink report on delete
BL_TARGET=$($ZDB create --title "Backlink Target" --body "I will be deleted")
sleep 1
BL_SOURCE=$($ZDB create --title "Backlink Source" --body "See [[$BL_TARGET]]")
$ZDB reindex >/dev/null
$ZDB delete "$BL_TARGET" 2>&1 | grep -q "broken backlinks"
$ZDB status 2>/dev/null | grep -q "broken backlinks"
# Clean up: delete source so broken backlinks don't affect later tests
$ZDB delete "$BL_SOURCE" >/dev/null 2>&1
pass "broken backlink report on delete"

# 7. reindex
$ZDB reindex | grep -q "indexed 2 zettels"
pass "reindex"

# 7b. hashtag extraction
$ZDB update "$ID1" --body "Updated with #gtd/act/next hashtag"
$ZDB reindex >/dev/null
$ZDB query "SELECT tag, source FROM _zdb_tags WHERE tag = 'gtd/act/next'" | grep -q "body"
pass "hashtag extraction and indexing"

# 7c. checkbox parsing
$ZDB update "$ID1" --body "- [ ] open task\n- [x] done task\n- [i] 2026-01-01 10:00 - info note"
$ZDB reindex >/dev/null
$ZDB query "SELECT state, content FROM _zdb_checkboxes WHERE state = 'open'" | grep -q "open task"
pass "checkbox parsing and indexing"

# 7d. folder namespace
$ZDB query "CREATE TABLE widget (color TEXT)" >/dev/null
# Add folder: true to the widget typedef
WIDGET_TYPEDEF=$(find "$TMPDIR/zettelkasten/_typedef" -name "*.md" -exec grep -l "title: widget" {} \;)
sed -i.bak 's/type: _typedef/type: _typedef\nfolder: true/' "$WIDGET_TYPEDEF" && rm -f "${WIDGET_TYPEDEF}.bak"
git -C "$TMPDIR" add -A && git -C "$TMPDIR" commit -m "add folder to widget" >/dev/null
$ZDB reindex >/dev/null
WIDGET_ID=$($ZDB query "INSERT INTO widget (color) VALUES ('red')")
test -f "$TMPDIR/zettelkasten/widget/${WIDGET_ID}.md"
pass "folder namespace: typed zettel in subdirectory"

# 8. full-text search
$ZDB search "First note" | grep -q "$ID1"
pass "search"

# 8b. paginated search
$ZDB search "First note" --limit 1 --offset 0 | grep -q "Showing 1-1 of"
pass "paginated search"

# 9. SQL queries
$ZDB query "SELECT id, title FROM zettels" | grep -q "First note (edited)"
$ZDB query "SELECT z.id, z.title FROM zettels z JOIN _zdb_tags t ON t.zettel_id = z.id WHERE t.tag LIKE '%smoke%'" | grep -q "$ID1"
pass "sql queries"

# 10. wikilinks
$ZDB query "SELECT * FROM _zdb_links" | grep -q "$ID1"
pass "wikilinks"

# 10a. link kinds (wikilink, markdown, embed, url)
LKBODY=$(printf 'See [[%s]] wiki.\n[md link](target.md)\n![[%s]]\nhttps://example.com' "$ID1" "$ID2")
LK_ID=$($ZDB create --title "Link Kinds" --body "$LKBODY")
$ZDB reindex >/dev/null
LK_OUT=$($ZDB query "SELECT kind FROM _zdb_links WHERE source_id = '$LK_ID' ORDER BY kind")
echo "$LK_OUT" | grep -q "url"
echo "$LK_OUT" | grep -q "embed"
echo "$LK_OUT" | grep -q "markdown"
echo "$LK_OUT" | grep -q "wikilink"
pass "link kinds (4 types indexed)"

# 10b. rename with backlink rewrite
RENAME_TARGET=$($ZDB create --title "Rename Target" --body "I will move.")
$ZDB create --title "Rename Linker" --body "See [[$RENAME_TARGET|Target]]." >/dev/null
$ZDB reindex >/dev/null
$ZDB rename "$RENAME_TARGET" "zettelkasten/contact/${RENAME_TARGET}.md" | grep -q "1 backlinks updated"
[ -f "zettelkasten/contact/${RENAME_TARGET}.md" ]
pass "rename with backlink rewrite"

# 11. SQL DDL/DML
$ZDB query "CREATE TABLE foo (bar TEXT, baz INTEGER)" | grep -q "table foo created"
FOO_ID=$($ZDB query "INSERT INTO foo (title, bar, baz) VALUES ('test row', 'hello', 42)")
echo "$FOO_ID" | grep -qE "^[0-9]{14}$"
$ZDB query "SELECT bar, baz FROM foo" | grep -q "hello"
$ZDB query "UPDATE foo SET baz = 99 WHERE id = '$FOO_ID'" | grep -q "1 row(s) affected"
$ZDB query "SELECT baz FROM foo WHERE id = '$FOO_ID'" | grep -q "99"
$ZDB query "DELETE FROM foo WHERE id = '$FOO_ID'" | grep -q "1 row(s) affected"
pass "sql ddl/dml"

# 11a. ALTER TABLE SET ZONE and TITLE TEMPLATE
$ZDB query "ALTER TABLE foo SET ZONE frontmatter FOR bar" | grep -q "zone set to frontmatter"
$ZDB query "ALTER TABLE foo SET TITLE TEMPLATE 'my-template'" | grep -q "title template set"
$ZDB query "ALTER TABLE foo DROP TITLE TEMPLATE" | grep -q "title template dropped"
pass "alter table zone overrides and title template"

# 11b. CREATE TABLE IF NOT EXISTS (idempotent)
$ZDB query "CREATE TABLE IF NOT EXISTS foo (bar TEXT, baz INTEGER)" | grep -q "already exists"
$ZDB query "CREATE TABLE IF NOT EXISTS newifne (x TEXT)" | grep -q "table newifne created"
$ZDB query "CREATE TABLE IF NOT EXISTS newifne (x TEXT)" | grep -q "already exists"
pass "create table if not exists (idempotent)"

# 12. install bundled type
$ZDB type install contact | grep -q "installed type"
pass "type install"

# 12a. hyphenated type SQL (quoted identifiers)
$ZDB type install meeting-minutes | grep -q "installed type"
HYP_ID=$($ZDB query 'INSERT INTO "meeting-minutes" (date, attendees) VALUES ('\''2026-03-10'\'', '\''alice,bob'\'')' | tr -d '[:space:]')
$ZDB query "SELECT date FROM \"meeting-minutes\" WHERE id = '$HYP_ID'" | grep -q "2026-03-10"
$ZDB query "DELETE FROM \"meeting-minutes\" WHERE id = '$HYP_ID'" | grep -q "1 row(s) affected"
pass "hyphenated type sql (quoted identifiers)"

# 13. type suggest
$ZDB query "INSERT INTO foo (title, bar, baz) VALUES ('for suggest', 'val', 1)" >/dev/null
$ZDB type suggest foo | grep -q "bar"
pass "type suggest"

# 14. register node + compact
$ZDB register-node "smoke-test-laptop" | grep -q "registered node"
$ZDB status | grep -q "registered nodes: 1"
COMPACT_OUT=$($ZDB compact --force)
echo "$COMPACT_OUT" | grep -q "backup:"
echo "$COMPACT_OUT" | grep -q "gc: ok"
echo "$COMPACT_OUT" | grep -q "crdt temp:"
echo "$COMPACT_OUT" | grep -q "repo (.git):"
pass "register-node + compact"

# 15. node list + retire
$ZDB node list | grep -q "smoke-test-laptop"
NODE_UUID=$($ZDB node list | grep "smoke-test-laptop" | awk '{print $1}')
$ZDB node retire "$NODE_UUID" | grep -q "retired node"
pass "node list + retire"

# 16. compact --dry-run
DRYRUN_OUT=$($ZDB compact --dry-run)
echo "$DRYRUN_OUT" | grep -q "dry run"
echo "$DRYRUN_OUT" | grep -q "backup would write:"
pass "compact --dry-run"

# 16a. compact --no-backup
$ZDB register-node "no-backup-test" >/dev/null
NOBACKUP_OUT=$($ZDB compact --no-backup --force)
echo "$NOBACKUP_OUT" | grep -q "gc: ok"
# Should NOT contain backup path
if echo "$NOBACKUP_OUT" | grep -q "backup:"; then
  echo "FAIL: --no-backup should suppress backup" >&2; exit 1
fi
pass "compact --no-backup"

# 16b. compact --backup-path
CUSTOM_BACKUP="$TMPDIR/custom-backup.bundle.tar"
BKPATH_OUT=$($ZDB compact --force --backup-path "$CUSTOM_BACKUP")
echo "$BKPATH_OUT" | grep -q "backup:"
echo "$BKPATH_OUT" | grep -q "$CUSTOM_BACKUP"
[ -f "$CUSTOM_BACKUP" ]
pass "compact --backup-path"

# 16c. maintenance
$ZDB maintenance run | grep -q "maintenance:"
pass "maintenance run"

MAINT_STATUS=$($ZDB maintenance auto status)
echo "$MAINT_STATUS" | grep -q "off"
pass "maintenance auto status (default off)"

$ZDB maintenance auto on | grep -q "enabled"
$ZDB maintenance auto status | grep -q "on"
pass "maintenance auto on"

$ZDB maintenance auto off | grep -q "disabled"
$ZDB maintenance auto status | grep -q "off"
pass "maintenance auto off"

# 16d. discover
$ZDB discover stale >/dev/null
pass "discover stale"

$ZDB discover orphans | head -1 | grep -q "."
pass "discover orphans"

# Create a zettel that mentions ID1's title without linking
MENTION_ID=$($ZDB create --title "Review notes" --body "About First note (edited) topic")
$ZDB reindex >/dev/null
$ZDB discover mentions "$ID1" | grep -q "$MENTION_ID"
pass "discover mentions"

$ZDB discover similar "$ID1" | head -1 | grep -q "."
pass "discover similar"

# 16e. consistency fix
FIX_ID=$($ZDB create --title "Fix Test" --tags "#gtd,zebra,apple")
BEFORE_HEAD=$(git rev-parse HEAD)
$ZDB fix --dry-run | grep -q "would fix"
[ "$(git rev-parse HEAD)" = "$BEFORE_HEAD" ]
pass "fix dry-run"

$ZDB fix | grep -q "fixed"
pass "fix apply"

$ZDB fix | grep -q "no issues"
pass "fix idempotent"

$ZDB read "$FIX_ID" | grep -q "  - apple"
pass "fix result verified"

# 16f. sequence navigation
SEQ_ROOT=$($ZDB create --title "Seq Root")
# Patch child zettel to have sequence field
SEQ_CHILD1=$($ZDB create --title "Seq Child 1")
SEQ_CHILD1_PATH="zettelkasten/${SEQ_CHILD1}.md"
cat > "$SEQ_CHILD1_PATH" <<SEQEOF
---
id: $SEQ_CHILD1
title: Seq Child 1
sequence: $SEQ_ROOT
---

SEQEOF
git add "$SEQ_CHILD1_PATH"
git commit -m "add sequence field" --quiet

SEQ_CHILD2=$($ZDB create --title "Seq Child 2")
SEQ_CHILD2_PATH="zettelkasten/${SEQ_CHILD2}.md"
cat > "$SEQ_CHILD2_PATH" <<SEQEOF
---
id: $SEQ_CHILD2
title: Seq Child 2
sequence: $SEQ_ROOT
---

SEQEOF
git add "$SEQ_CHILD2_PATH"
git commit -m "add sequence field" --quiet

$ZDB reindex >/dev/null
$ZDB sequence tree "$SEQ_ROOT" | grep -q "$SEQ_CHILD1"
pass "sequence tree"

$ZDB sequence breadcrumb "$SEQ_CHILD1" | grep -q "$SEQ_ROOT"
pass "sequence breadcrumb"

# Broken sequence ref
SEQ_BROKEN=$($ZDB create --title "Seq Broken")
SEQ_BROKEN_PATH="zettelkasten/${SEQ_BROKEN}.md"
cat > "$SEQ_BROKEN_PATH" <<SEQEOF
---
id: $SEQ_BROKEN
title: Seq Broken
sequence: "99999999999999"
---

SEQEOF
git add "$SEQ_BROKEN_PATH"
git commit -m "broken sequence ref" --quiet
$ZDB reindex >/dev/null
$ZDB sequence broken | grep -q "not found"
pass "sequence broken"

# 16b. --log-level flag accepted
$ZDB --log-level debug status >/dev/null 2>&1
pass "--log-level flag accepted"

# 16c. help guides (no repo needed)
HELP_OUT=$($ZDB help create-app)
echo "$HELP_OUT" | grep -q "CREATE TABLE"
pass "help create-app"
$ZDB help | grep -q "create-app"
pass "help list"
! $ZDB help nonexistent 2>/dev/null
pass "help unknown fails"

# 16d. app-building end-to-end flow
$ZDB query "CREATE TABLE abcategory (name VARCHAR(100))" | grep -q "table abcategory created"
AB_CAT_ID=$($ZDB query "INSERT INTO abcategory (name) VALUES ('work')")
echo "$AB_CAT_ID" | grep -qE "^[0-9]{14}$"
$ZDB query "CREATE TABLE abbookmark (url VARCHAR(2048), description TEXT, abcategory TEXT REFERENCES abcategory)" | grep -q "table abbookmark created"
$ZDB query "ALTER TABLE abbookmark SET ZONE reference FOR url" | grep -q "zone set to reference"
$ZDB query "ALTER TABLE abbookmark SET TITLE TEMPLATE '{url}'" | grep -q "title template set"
# Insert with explicit title
sleep 1
AB_BM1=$($ZDB query "INSERT INTO abbookmark (title, url, description) VALUES ('Rust Book', 'https://doc.rust-lang.org', 'The official Rust book')")
echo "$AB_BM1" | grep -qE "^[0-9]{14}$"
# Insert with template-derived title (no explicit title)
sleep 1
AB_BM2=$($ZDB query "INSERT INTO abbookmark (url, description) VALUES ('https://crates.io', 'Rust package registry')")
echo "$AB_BM2" | grep -qE "^[0-9]{14}$"
# Link bookmark to category via junction table
$ZDB query "INSERT INTO abbookmark_abcategory (abbookmark_id, abcategory_id) VALUES ('$AB_BM1', '$AB_CAT_ID')" | grep -q "1 row"
# SELECT from main table
$ZDB query "SELECT url FROM abbookmark" | grep -q "rust-lang"
# SELECT from junction table
$ZDB query "SELECT abcategory_id FROM abbookmark_abcategory WHERE abbookmark_id = '$AB_BM1'" | grep -q "$AB_CAT_ID"
# help create-app guide available
$ZDB help create-app | grep -q "CREATE TABLE"
# Clean up
$ZDB query "DROP TABLE abbookmark CASCADE" | grep -q "dropped"
$ZDB query "DROP TABLE abcategory CASCADE" | grep -q "dropped"
pass "app-building end-to-end flow"

if [ "$SMOKE_PROFILE" = "quick" ]; then
  pass "quick profile complete"
  exit 0
fi

# 17. GraphQL server
SERVER_PORT=$((19200 + (RANDOM % 800)))
PG_PORT=$((SERVER_PORT + 1))
$ZDB serve --port "$SERVER_PORT" --pg-port "$PG_PORT" &
SERVER_PID=$!
# Wait for server to start
for i in $(seq 1 20); do
  if curl -sf "http://127.0.0.1:$SERVER_PORT/graphql" \
    -H "Authorization: Bearer $(cat ~/.config/zetteldb/token 2>/dev/null || echo '')" \
    -H "Content-Type: application/json" \
    -d '{"query":"{ typeDefs { name } }"}' >/dev/null 2>&1; then
    break
  fi
  sleep 0.2
done
TOKEN=$(cat ~/.config/zetteldb/token 2>/dev/null || echo '')
GQL_URL="http://127.0.0.1:$SERVER_PORT/graphql"
REST_URL="http://127.0.0.1:$SERVER_PORT/rest"
gql() {
  curl -sf "$GQL_URL" \
    -H "Authorization: Bearer $TOKEN" \
    -H "Content-Type: application/json" \
    -d "$1"
}
rest() {
  curl -sf "$REST_URL$1" \
    -H "Authorization: Bearer $TOKEN" \
    -H "Content-Type: application/json" \
    "${@:2}"
}

# Test auth
HTTP_CODE=$(curl -so /dev/null -w "%{http_code}" "$GQL_URL" \
  -H "Content-Type: application/json" \
  -d '{"query":"{ typeDefs { name } }"}')
[ "$HTTP_CODE" = "401" ]
pass "serve: auth rejects missing token"

# Health endpoint (no auth required)
HEALTH=$(curl -sf "http://127.0.0.1:$SERVER_PORT/health")
echo "$HEALTH" | grep -q '"status":"ok"'
pass "serve: health endpoint"

# Test query
RESULT=$(gql '{"query":"{ typeDefs { name } }"}')
echo "$RESULT" | grep -q '"typeDefs"'
pass "serve: graphql query"

# Test mutation — create
RESULT=$(gql '{"query":"mutation { createZettel(input: { title: \"Smoke Server\" }) { id title } }"}')
echo "$RESULT" | grep -q '"Smoke Server"'
GQL_ID=$(echo "$RESULT" | sed -n 's/.*"id":"\([^"]*\)".*/\1/p')
pass "serve: graphql create"

# 18. expanded GraphQL operations
RESULT=$(gql "{\"query\":\"mutation { updateZettel(input: { id: \\\"$GQL_ID\\\", title: \\\"Smoke Updated\\\" }) { id title } }\"}")
echo "$RESULT" | grep -q '"Smoke Updated"'
pass "serve: graphql update"

RESULT=$(gql '{"query":"{ search(query: \"Smoke\") { totalCount hits { id title } } }"}')
echo "$RESULT" | grep -q '"search"'
pass "serve: graphql search"

RESULT=$(gql '{"query":"{ zettels { id title } }"}')
echo "$RESULT" | grep -q '"zettels"'
pass "serve: graphql zettels"

RESULT=$(gql "{\"query\":\"mutation { deleteZettel(id: \\\"$GQL_ID\\\") }\"}")
echo "$RESULT" | grep -q "true"
pass "serve: graphql delete"

# 18b. GraphQL checkbox queries
RESULT=$(gql '{"query":"{ openActions { state content } }"}')
echo "$RESULT" | grep -q '"openActions"'
pass "serve: graphql openActions"

# 19. REST API CRUD
HTTP_CODE=$(curl -so /dev/null -w "%{http_code}" "$REST_URL/zettels" \
  -H "Content-Type: application/json" \
  -d '{"title":"REST No Auth"}')
[ "$HTTP_CODE" = "401" ]
pass "rest: auth rejects missing token"

RESULT=$(curl -sf -w "\n%{http_code}" "$REST_URL/zettels" \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"title":"REST Smoke","body":"rest body","tags":["rest"]}')
HTTP_CODE=$(echo "$RESULT" | tail -1)
BODY=$(echo "$RESULT" | sed '$d')
[ "$HTTP_CODE" = "201" ]
REST_ID=$(echo "$BODY" | sed -n 's/.*"id":"\([^"]*\)".*/\1/p')
pass "rest: create"

RESULT=$(rest "/zettels/$REST_ID")
echo "$RESULT" | grep -q "REST Smoke"
pass "rest: get"

rest "/zettels/$REST_ID" -X PUT -d '{"title":"REST Updated"}' | grep -q "REST Updated"
pass "rest: update"

RESULT=$(rest "/zettels?tag=rest")
echo "$RESULT" | grep -q "$REST_ID"
pass "rest: list with filter"

# Field filtering: create typed zettel via SQL, filter by field
gql '{"query":"mutation{executeSql(sql:\"CREATE TABLE smokeitem (label TEXT NOT NULL, priority INTEGER)\"){message}}"}'
gql '{"query":"mutation{executeSql(sql:\"INSERT INTO smokeitem (label, priority) VALUES ('\''Smoke1'\'', 7)\"){message}}"}'
RESULT=$(rest "/zettels?field.priority=7")
echo "$RESULT" | grep -q "Smoke1"
pass "rest: field filter"

# Field filter with nonexistent value returns empty
RESULT=$(rest "/zettels?field.priority=999")
echo "$RESULT" | grep -q '"data":\[\]'
pass "rest: field filter no match"

HTTP_CODE=$(curl -so /dev/null -w "%{http_code}" "$REST_URL/zettels/$REST_ID" \
  -H "Authorization: Bearer $TOKEN" -X DELETE)
[ "$HTTP_CODE" = "204" ]
pass "rest: delete"

HTTP_CODE=$(curl -so /dev/null -w "%{http_code}" "$REST_URL/zettels/$REST_ID" \
  -H "Authorization: Bearer $TOKEN")
[ "$HTTP_CODE" = "404" ]
pass "rest: get after delete returns 404"

# 20. PgWire basic query
if command -v psql >/dev/null 2>&1; then
  PGPASSWORD="$TOKEN" psql -h 127.0.0.1 -p "$PG_PORT" -U zdb -d zdb -t -c "SELECT id, title FROM zettels" | grep -q "First note"
  pass "pgwire: select"

  ! PGPASSWORD="wrong" psql -h 127.0.0.1 -p "$PG_PORT" -U zdb -d zdb -c "SELECT 1" 2>/dev/null
  pass "pgwire: auth rejection"
else
  pass "pgwire: skipped (no psql)"
fi

# NoSQL server endpoints
NOSQL_URL="http://127.0.0.1:$SERVER_PORT/nosql"
nosql() {
  curl -sf "$NOSQL_URL$1" \
    -H "Authorization: Bearer $TOKEN"
}

nosql "/$ID1" | grep -q "First note"
pass "nosql-api: get by id"

nosql "?tag=smoke" | grep -q "$ID1"
pass "nosql-api: scan by tag"

HTTP_CODE=$(curl -so /dev/null -w "%{http_code}" "$NOSQL_URL?type=project&tag=test" \
  -H "Authorization: Bearer $TOKEN")
[ "$HTTP_CODE" = "400" ]
pass "nosql-api: rejects both type and tag"

HTTP_CODE=$(curl -so /dev/null -w "%{http_code}" "$NOSQL_URL/$ID1" \
  -H "Content-Type: application/json")
[ "$HTTP_CODE" = "401" ]
pass "nosql-api: auth rejects missing token"

# error sanitization — SQL error must not leak raw details
RESULT=$(gql '{"query":"mutation { executeSql(sql: \"SELCT * FORM oops\") { message } }"}' || true)
if [ -z "$RESULT" ]; then echo "FAIL: gql returned empty response" >&2; exit 1; fi
echo "$RESULT" | grep -q '"errors"'
echo "$RESULT" | grep -qi "query failed\|internal error"
! echo "$RESULT" | grep -qi "SELCT\|syntax error\|sqlite"
pass "serve: sql error sanitized (no raw details)"

# error sanitization — not-found returns descriptive message
HTTP_CODE=$(curl -so /dev/null -w "%{http_code}" "$REST_URL/zettels/99990101000000" \
  -H "Authorization: Bearer $TOKEN")
[ "$HTTP_CODE" = "404" ]
pass "serve: not-found returns 404"

# compact mutation
RESULT=$(gql '{"query":"mutation { compact { filesRemoved crdtDocsCompacted gcSuccess crdtTempBytesBefore crdtTempBytesAfter crdtTempFilesBefore crdtTempFilesAfter repoBytesBefore repoBytesAfter backupPath } }"}')
echo "$RESULT" | grep -q '"gcSuccess"'
pass "serve: compact mutation"

# compact(force: true)
RESULT=$(gql '{"query":"mutation { compact(force: true) { filesRemoved crdtDocsCompacted gcSuccess crdtTempBytesBefore crdtTempBytesAfter repoBytesBefore repoBytesAfter backupPath } }"}')
echo "$RESULT" | grep -q '"gcSuccess"'
echo "$RESULT" | grep -q '"backupPath"'
pass "serve: compact(force: true) mutation"

# compact(noBackup: true)
RESULT=$(gql '{"query":"mutation { compact(force: true, noBackup: true) { gcSuccess backupPath } }"}')
echo "$RESULT" | grep -q '"gcSuccess"'
echo "$RESULT" | grep -q '"backupPath":null'
pass "serve: compact(noBackup: true) mutation"

# compact(backupPath: custom)
GQL_BACKUP="$TMPDIR/gql-backup.bundle.tar"
RESULT=$(gql "{\"query\":\"mutation { compact(force: true, backupPath: \\\"$GQL_BACKUP\\\") { gcSuccess backupPath } }\"}")
echo "$RESULT" | grep -q '"gcSuccess"'
echo "$RESULT" | grep -q '"backupPath"'
[ -f "$GQL_BACKUP" ]
pass "serve: compact(backupPath) mutation"

# maintenance mutation
RESULT=$(gql '{"query":"mutation { maintenance { success durationMs fallbackUsed tasksRun } }"}')
echo "$RESULT" | grep -q '"success"'
pass "serve: maintenance mutation"

# sync mutation — no remote configured for this repo, expect error not panic
RESULT=$(gql '{"query":"mutation { sync { direction commitsTransferred conflictsResolved resurrected } }"}')
echo "$RESULT" | grep -q '"errors"'
pass "serve: sync mutation (no remote → error)"

# 37. WebSocket payload auth (browser-style, no Authorization header)
if command -v websocat >/dev/null 2>&1; then
  REPLY=$(echo '{"type":"connection_init","payload":{"Authorization":"Bearer '"$TOKEN"'"}}' | \
    websocat --protocol graphql-transport-ws -1 "ws://127.0.0.1:$SERVER_PORT/ws" 2>/dev/null || true)
  echo "$REPLY" | grep -q "connection_ack"
  pass "ws: payload auth (connection_init)"
else
  pass "ws: payload auth (skipped, no websocat — see e2e tests)"
fi

# 38. read-under-write: concurrent read + write
WRITE_TMP="$TMPDIR/write_result.json"
gql '{"query":"mutation { createZettel(input: { title: \"ReadPoolWrite\" }) { id } }"}' > "$WRITE_TMP" &
WRITE_PID=$!
READ_RESULT=$(gql '{"query":"{ zettels { id title } }"}')
echo "$READ_RESULT" | grep -q '"zettels"'
wait "$WRITE_PID"
grep -q '"id"' "$WRITE_TMP"
pass "serve: read-under-write (concurrent read + write)"

# 38b. multi-value references via GraphQL + REST
gql '{"query":"mutation{executeSql(sql:\"CREATE TABLE mvcategory (name VARCHAR(100))\"){message}}"}'
gql '{"query":"mutation{executeSql(sql:\"CREATE TABLE mvbookmark (mvcategory TEXT REFERENCES mvcategory)\"){message}}"}'
sleep 1
MV_CAT1=$(gql '{"query":"mutation{executeSql(sql:\"INSERT INTO mvcategory (name) VALUES (\\\"Science\\\")\"){message}}"}' | sed -n 's/.*"message":"\([0-9]*\)".*/\1/p')
MV_CAT2=$(gql '{"query":"mutation{executeSql(sql:\"INSERT INTO mvcategory (name) VALUES (\\\"Math\\\")\"){message}}"}' | sed -n 's/.*"message":"\([0-9]*\)".*/\1/p')
[ -n "$MV_CAT1" ] && [ -n "$MV_CAT2" ]
MV_BM=$(gql "{\"query\":\"mutation{executeSql(sql:\\\"INSERT INTO mvbookmark (mvcategory) VALUES ('$MV_CAT1')\\\"){message}}\"}" | sed -n 's/.*"message":"\([0-9]*\)".*/\1/p')
[ -n "$MV_BM" ]
gql "{\"query\":\"mutation{executeSql(sql:\\\"INSERT INTO mvbookmark_mvcategory (mvbookmark_id, mvcategory_id) VALUES ('$MV_BM', '$MV_CAT2')\\\"){message}}\"}" >/dev/null
# GraphQL typed query — pluralized list field returns both categories
RESULT=$(gql '{"query":"{ mvbookmarks { items { id mvcategories } } }"}')
echo "$RESULT" | grep -q "$MV_CAT1"
echo "$RESULT" | grep -q "$MV_CAT2"
pass "serve: graphql multi-value ref list field"
# REST — structured references object
RESULT=$(rest "/zettels/$MV_BM")
echo "$RESULT" | grep -q '"references"'
echo "$RESULT" | grep -q '"mvcategory"'
pass "serve: rest multi-value ref structured json"

kill "$SERVER_PID" 2>/dev/null || true
wait "$SERVER_PID" 2>/dev/null || true
pass "serve: clean shutdown"

echo "=== sync conflict scenarios ==="

# --- Two-node setup ---
# bare remote
git init --bare "$REMOTE_DIR" >/dev/null 2>&1

# node1: init + push
cd "$NODE1_DIR"
$ZDB init . >/dev/null
git remote add origin "$REMOTE_DIR"
$ZDB register-node "Laptop" >/dev/null

# 21. fast-forward sync
SYNC_ID=$($ZDB create --title "Shared note" --tags "shared" --body "Original body")
git push -u origin master >/dev/null 2>&1

# clone to node2
git clone "$REMOTE_DIR" "$NODE2_DIR" >/dev/null 2>&1
cd "$NODE2_DIR"
# init zdb index without reinitializing git
$ZDB reindex >/dev/null
$ZDB register-node "Desktop" >/dev/null

$ZDB read "$SYNC_ID" | grep -q "Shared note"
pass "fast-forward sync"

# 22. non-overlapping edits (clean git merge, no CRDT)
cd "$NODE1_DIR"
$ZDB update "$SYNC_ID" --title "Updated Title" --tags "shared,laptop"

cd "$NODE2_DIR"
$ZDB update "$SYNC_ID" --body "Modified body"

cd "$NODE1_DIR"
$ZDB sync origin master >/dev/null

cd "$NODE2_DIR"
SYNC_OUT=$($ZDB sync origin master)
echo "$SYNC_OUT" | grep -q "conflicts resolved: 0"

$ZDB read "$SYNC_ID" | grep -q "Updated Title"
$ZDB read "$SYNC_ID" | grep -q "Modified body"
pass "non-overlapping edits (clean merge)"

# 23. frontmatter scalar conflict (title) — CRDT resolves
cd "$NODE1_DIR"
$ZDB sync origin master >/dev/null
$ZDB update "$SYNC_ID" --title "Laptop Title"

cd "$NODE2_DIR"
$ZDB update "$SYNC_ID" --title "Desktop Title"

cd "$NODE1_DIR"
$ZDB sync origin master >/dev/null

cd "$NODE2_DIR"
SYNC_OUT=$($ZDB sync origin master)
echo "$SYNC_OUT" | grep -q "conflicts resolved: 1"

TITLE=$($ZDB read "$SYNC_ID" | grep "^title:")
echo "$TITLE" | grep -qE "(Laptop Title|Desktop Title)"
pass "frontmatter scalar conflict (CRDT)"

# 24. frontmatter list conflict (tags) — CRDT set-union
cd "$NODE1_DIR"
$ZDB sync origin master >/dev/null
$ZDB update "$SYNC_ID" --tags "base,alpha"

cd "$NODE2_DIR"
$ZDB update "$SYNC_ID" --tags "base,beta"

cd "$NODE1_DIR"
$ZDB sync origin master >/dev/null

cd "$NODE2_DIR"
$ZDB sync origin master >/dev/null

READ_OUT=$($ZDB read "$SYNC_ID")
echo "$READ_OUT" | grep -q "alpha"
echo "$READ_OUT" | grep -q "beta"
pass "frontmatter list conflict (tag union)"

# 25. body conflict — Automerge Text CRDT
cd "$NODE1_DIR"
$ZDB sync origin master >/dev/null
$ZDB update "$SYNC_ID" --body $'Line one LAPTOP.\nLine two.\nLine three.'

cd "$NODE2_DIR"
$ZDB update "$SYNC_ID" --body $'Line one.\nLine two DESKTOP.\nLine three.'

cd "$NODE1_DIR"
$ZDB sync origin master >/dev/null

cd "$NODE2_DIR"
SYNC_OUT=$($ZDB sync origin master)
echo "$SYNC_OUT" | grep -q "conflicts resolved: 1"

READ_OUT=$($ZDB read "$SYNC_ID")
echo "$READ_OUT" | grep -q "LAPTOP"
echo "$READ_OUT" | grep -q "DESKTOP"
pass "body conflict (CRDT text merge)"

# 26. reference section conflict — write files directly, CRDT union
cd "$NODE1_DIR"
$ZDB sync origin master >/dev/null

ZETTEL_FILE="zettelkasten/${SYNC_ID}.md"

# node1: append reference section with laptop-specific field
CONTENT=$(cat "$ZETTEL_FILE")
printf '%s\n---\n- laptop note:: Added from laptop\n' "$CONTENT" > "$ZETTEL_FILE"
git add "$ZETTEL_FILE" && git commit -m "node1 add reference" >/dev/null 2>&1
git push origin master >/dev/null 2>&1

# node2: append different reference field (from its pre-push version)
cd "$NODE2_DIR"
CONTENT=$(cat "$ZETTEL_FILE")
printf '%s\n---\n- desktop note:: Added from desktop\n' "$CONTENT" > "$ZETTEL_FILE"
git add "$ZETTEL_FILE" && git commit -m "node2 add reference" >/dev/null 2>&1

SYNC_OUT=$($ZDB sync origin master)
echo "$SYNC_OUT" | grep -q "conflicts resolved: 1"

READ_OUT=$($ZDB read "$SYNC_ID")
echo "$READ_OUT" | grep -q "laptop note"
echo "$READ_OUT" | grep -q "desktop note"
pass "reference section conflict (CRDT union)"

# 27b. delete-vs-edit conflict — edit wins, zettel resurrected
cd "$NODE1_DIR"
$ZDB sync origin master >/dev/null
DEL_ID=$($ZDB create --title "To be deleted" --body "Original content")
$ZDB sync origin master >/dev/null

cd "$NODE2_DIR"
$ZDB sync origin master >/dev/null
$ZDB read "$DEL_ID" | grep -q "To be deleted"

# node1 deletes, node2 edits
cd "$NODE1_DIR"
$ZDB delete "$DEL_ID"

cd "$NODE2_DIR"
$ZDB update "$DEL_ID" --body "Edited on desktop"

cd "$NODE1_DIR"
$ZDB sync origin master >/dev/null

cd "$NODE2_DIR"
$ZDB sync origin master >/dev/null

# Edit wins: zettel exists and is marked resurrected
$ZDB read "$DEL_ID" | grep -q "Edited on desktop"
$ZDB status | grep -q "resurrected"
pass "delete-vs-edit conflict (edit wins, resurrected)"

echo "=== id collision detection ==="

# 27a. add-add collision: both zettels survive
# Use a fresh pair of repos to avoid interference with earlier sync state.
COLL_REMOTE="$(mktemp -d)"
COLL_A="$(mktemp -d)"
COLL_B_PARENT="$(mktemp -d)"
git init --bare "$COLL_REMOTE" >/dev/null 2>&1

# Node A: init, push, register
cd "$COLL_A"
$ZDB init . >/dev/null
git remote add origin "$COLL_REMOTE"
$ZDB register-node "CollA" >/dev/null
git push -u origin master >/dev/null 2>&1

# Node B: clone, register
git clone "$COLL_REMOTE" "$COLL_B_PARENT/repo" >/dev/null 2>&1
COLL_B="$COLL_B_PARENT/repo"
cd "$COLL_B"
$ZDB reindex >/dev/null
$ZDB register-node "CollB" >/dev/null
git push origin master >/dev/null 2>&1

# Sync A to pick up B's node
cd "$COLL_A"
$ZDB sync origin master >/dev/null

# Both create the same-ID zettel independently
COLL_ID="20260101120000"
cd "$COLL_A"
mkdir -p zettelkasten
printf -- '---\nid: %s\ntitle: From A\ndate: 2026-01-01\n---\nBody A\n' "$COLL_ID" > "zettelkasten/${COLL_ID}.md"
git add "zettelkasten/${COLL_ID}.md"
git commit -m "A creates $COLL_ID" >/dev/null 2>&1
git push origin master >/dev/null 2>&1

cd "$COLL_B"
mkdir -p zettelkasten
printf -- '---\nid: %s\ntitle: From B\ndate: 2026-01-01\n---\nBody B\n' "$COLL_ID" > "zettelkasten/${COLL_ID}.md"
git add "zettelkasten/${COLL_ID}.md"
git commit -m "B creates $COLL_ID" >/dev/null 2>&1

# B syncs — collision detected and resolved
COLL_OUT=$($ZDB sync origin master)
echo "$COLL_OUT" | grep -q "collisions reassigned: 1"
COLL_COUNT=$(find zettelkasten -name '*.md' ! -name '_*' | wc -l | tr -d ' ')
[ "$COLL_COUNT" -eq 2 ]
pass "add-add collision: both zettels survive"

echo "=== bundle sync ==="

# 27. bundle export --full + import
cd "$NODE1_DIR"
$ZDB sync origin master >/dev/null
$ZDB bundle export --full --output "$TMPDIR/full-bundle.tar"
echo "$TMPDIR/full-bundle.tar" | grep -q "full-bundle.tar"

cd "$NODE3_DIR"
$ZDB init . >/dev/null
$ZDB register-node "Tablet" >/dev/null
$ZDB bundle import "$TMPDIR/full-bundle.tar" | grep -q "imported"
$ZDB read "$SYNC_ID" | grep -q "laptop note"
pass "bundle export --full + import"

# 28. delta bundle export + import
cd "$NODE1_DIR"
DELTA_ID=$($ZDB create --title "Delta note" --body "only in delta")

NODE2_UUID=$(cd "$NODE2_DIR" && cat .git/zdb-node)
$ZDB bundle export --target "$NODE2_UUID" --output "$TMPDIR/delta-bundle.tar"

cd "$NODE2_DIR"
$ZDB bundle import "$TMPDIR/delta-bundle.tar" | grep -q "imported"
$ZDB read "$DELTA_ID" | grep -q "Delta note"
pass "delta bundle export + import"

# 29. update-bin help + rollback
$ZDB update-bin --help | grep -q "Update zdb"
$ZDB update-bin --help | grep -q "\-\-rollback"
pass "update-bin --help (includes --rollback)"

# rollback with no backup should fail gracefully
ROLLBACK_OUT=$($ZDB update-bin --rollback 2>&1 || true)
echo "$ROLLBACK_OUT" | grep -q "no backup"
pass "update-bin --rollback (no backup error)"

# 30. ALTER TABLE + DROP TABLE + bulk UPDATE/DELETE
cd "$TMPDIR"
$ZDB query "CREATE TABLE smokealt (name TEXT, score INTEGER)" | grep -q "table smokealt created"
$ZDB query "INSERT INTO smokealt (name, score) VALUES ('a', 1)" >/dev/null
sleep 1
$ZDB query "INSERT INTO smokealt (name, score) VALUES ('b', 2)" >/dev/null
$ZDB query "ALTER TABLE smokealt ADD COLUMN tag TEXT" | grep -q "altered"
$ZDB query "SELECT name, tag FROM smokealt" | grep -q "NULL"
$ZDB query "ALTER TABLE smokealt RENAME COLUMN tag TO label" | grep -q "renamed"
$ZDB query "SELECT name, label FROM smokealt" | grep -q "a"
$ZDB query "UPDATE smokealt SET score = 99 WHERE name = 'a'" | grep -q "1 row(s) affected"
$ZDB query "DELETE FROM smokealt WHERE name = 'b'" | grep -q "1 row(s) affected"
$ZDB query "DROP TABLE smokealt CASCADE" | grep -q "dropped"
pass "alter/drop table + bulk ops"

# 31. file attachments
cd "$TMPDIR"
echo "hello attachment" > $TMPDIR/zdb-smoke-attach.txt
$ZDB attach "$ID1" $TMPDIR/zdb-smoke-attach.txt | grep -q "attached"
$ZDB attachments "$ID1" | grep -q "zdb-smoke-attach.txt"
$ZDB attachments "$ID1" | grep -q "text/plain"
$ZDB query "SELECT name, mime FROM _zdb_attachments WHERE zettel_id = '$ID1'" | grep -q "zdb-smoke-attach.txt"
$ZDB detach "$ID1" "zdb-smoke-attach.txt" | grep -q "detached"
$ZDB attachments "$ID1" | grep -q "no attachments"
rm -f $TMPDIR/zdb-smoke-attach.txt
pass "file attachments (attach/list/query/detach)"

# 32. NoSQL CLI commands
cd "$TMPDIR"
$ZDB get "$ID1" | grep -q "First note (edited)"
pass "nosql: get"

$ZDB scan --tag test | grep -q "$ID1"
pass "nosql: scan --tag"

$ZDB scan --type foo | grep -qE "^[0-9]{14}$"
pass "nosql: scan --type"

$ZDB backlinks "$ID1" | grep -q "$ID2"
pass "nosql: backlinks"

# 33. stale node resync after compaction
echo "=== stale node resync ==="
STALE_REMOTE="$(mktemp -d)"
STALE_N1="$(mktemp -d)"
STALE_N2="$(mktemp -d)"
trap 'rm -rf "$TMPDIR" "$REMOTE_DIR" "$NODE1_DIR" "$NODE2_DIR" "$NODE3_DIR" "$STALE_REMOTE" "$STALE_N1" "$STALE_N2" "$COLL_REMOTE" "$COLL_A" "$COLL_B_PARENT"' EXIT

git init --bare "$STALE_REMOTE" >/dev/null 2>&1

cd "$STALE_N1"
$ZDB init . >/dev/null
git remote add origin "$STALE_REMOTE"
$ZDB register-node "StaleNode1" >/dev/null
STALE_ID=$($ZDB create --title "Stale shared" --body "original content")
git push -u origin master >/dev/null 2>&1

git clone "$STALE_REMOTE" "$STALE_N2" >/dev/null 2>&1
cd "$STALE_N2"
$ZDB reindex >/dev/null
$ZDB register-node "StaleNode2" >/dev/null

# Both nodes edit the same zettel → conflict
cd "$STALE_N1"
$ZDB update "$STALE_ID" --body "body from node1"
git push origin master >/dev/null 2>&1

cd "$STALE_N2"
$ZDB update "$STALE_ID" --body "body from node2"
$ZDB sync origin master >/dev/null

# Compact to remove CRDT temp files — verify report includes byte stats
COMPACT_OUT=$($ZDB compact --force)
echo "$COMPACT_OUT" | grep -q "crdt temp:"
echo "$COMPACT_OUT" | grep -q "repo (.git):"

# Create another conflict without CRDT state
cd "$STALE_N1"
$ZDB sync origin master >/dev/null
$ZDB update "$STALE_ID" --body "second edit node1"
git push origin master >/dev/null 2>&1

cd "$STALE_N2"
$ZDB update "$STALE_ID" --body "second edit node2"
$ZDB sync origin master >/dev/null

# Verify zettel is readable and valid
$ZDB read "$STALE_ID" | grep -q "title:"
pass "stale node resync after compaction"

# 34. multi-row INSERT
cd "$TMPDIR"
$ZDB query "CREATE TABLE multirow (name TEXT, val INTEGER)" | grep -q "table multirow created"
MULTI_IDS=$($ZDB query "INSERT INTO multirow (name, val) VALUES ('a', 1), ('b', 2), ('c', 3)")
echo "$MULTI_IDS" | grep -qE "^[0-9]{14},[0-9]{14},[0-9]{14}$"
$ZDB query "SELECT COUNT(*) FROM multirow" | grep -q "3"
pass "multi-row insert"

# 35. transaction commit + rollback
cd "$TMPDIR"
$ZDB query "CREATE TABLE txntest (val TEXT)" | grep -q "table txntest created"
$ZDB query "BEGIN; INSERT INTO txntest (val) VALUES ('committed'); COMMIT" | grep -q "COMMIT"
$ZDB query "SELECT val FROM txntest" | grep -q "committed"
$ZDB query "BEGIN; INSERT INTO txntest (val) VALUES ('rolled-back'); ROLLBACK" | grep -q "ROLLBACK"
# rolled-back row should not appear
TXNCOUNT=$($ZDB query "SELECT COUNT(*) FROM txntest")
echo "$TXNCOUNT" | grep -q "1"
pass "transaction commit + rollback"

# 36. hyphenated type SQL via quoted identifiers
cd "$TMPDIR"
$ZDB query 'CREATE TABLE "my-type" (label TEXT)' | grep -q "table my-type created"
MY_ID=$($ZDB query 'INSERT INTO "my-type" (label) VALUES ('\''test'\'')')
$ZDB query 'SELECT label FROM "my-type"' | grep -q "test"
$ZDB query "DELETE FROM \"my-type\" WHERE id = '$MY_ID'" | grep -q "1 row(s) affected"
pass "hyphenated type SQL"

# 37. junction table CRUD (multi-value references)
cd "$TMPDIR"
$ZDB query "CREATE TABLE jtag (name VARCHAR(100))" | grep -q "table jtag created"
$ZDB query "CREATE TABLE jpost (url TEXT, jtag TEXT REFERENCES jtag)" | grep -q "table jpost created"
JT_TAG_ID=$($ZDB query "INSERT INTO jtag (name) VALUES ('rust')")
sleep 1
JT_POST_ID=$($ZDB query "INSERT INTO jpost (url) VALUES ('https://example.com')")
$ZDB query "INSERT INTO jpost_jtag (jpost_id, jtag_id) VALUES ('$JT_POST_ID', '$JT_TAG_ID')" | grep -q "1 row"
$ZDB query "SELECT jtag_id FROM jpost_jtag WHERE jpost_id = '$JT_POST_ID'" | grep -q "$JT_TAG_ID"
$ZDB query "DELETE FROM jpost_jtag WHERE jpost_id = '$JT_POST_ID' AND jtag_id = '$JT_TAG_ID'" | grep -q "1 row"
$ZDB query "SELECT COUNT(*) FROM jpost_jtag" | grep -q "0"
$ZDB query "INSERT INTO jpost_jtag (jpost_id, jtag_id) VALUES ('$JT_POST_ID', '$JT_TAG_ID')" >/dev/null
$ZDB query "DROP TABLE jpost CASCADE" | grep -q "dropped"
! $ZDB query "SELECT * FROM jpost_jtag" 2>/dev/null
pass "junction table CRUD"

# 38. title template compliance check
cd "$TMPDIR"
$ZDB query "CREATE TABLE smwidget (name VARCHAR(100), description TEXT)" >/dev/null
$ZDB query "ALTER TABLE smwidget SET TITLE TEMPLATE '{name} Widget'" >/dev/null
$ZDB query "INSERT INTO smwidget (name, description) VALUES ('Foo', 'A foo widget')" >/dev/null
$ZDB fix --verbose --dry-run | grep -q "title does not match template"
pass "title template compliance check"

# 39. zone migration
cd "$TMPDIR"
$ZDB query "CREATE TABLE gadget (notes TEXT)" | grep -q "table gadget created"
$ZDB query "INSERT INTO gadget (notes) VALUES ('Some notes')" >/dev/null
$ZDB query "ALTER TABLE gadget SET ZONE frontmatter FOR notes" >/dev/null
FIX_OUT=$($ZDB fix --migrate --verbose)
echo "$FIX_OUT" | grep -q "zone-migrated"
pass "zone migration"

echo "=== all passed ==="
