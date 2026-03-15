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

# 8. full-text search
$ZDB search "Hello" | grep -q "$ID1"
pass "search"

# 8b. paginated search
$ZDB search "Hello" --limit 1 --offset 0 | grep -q "Showing 1-1 of"
pass "paginated search"

# 9. SQL queries
$ZDB query "SELECT id, title FROM zettels" | grep -q "First note (edited)"
$ZDB query "SELECT z.id, z.title FROM zettels z JOIN _zdb_tags t ON t.zettel_id = z.id WHERE t.tag LIKE '%smoke%'" | grep -q "$ID1"
pass "sql queries"

# 10. wikilinks
$ZDB query "SELECT * FROM _zdb_links" | grep -q "$ID1"
pass "wikilinks"

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

# 29. update-bin help
$ZDB update-bin --help | grep -q "Update zdb"
pass "update-bin --help"

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
trap 'rm -rf "$TMPDIR" "$REMOTE_DIR" "$NODE1_DIR" "$NODE2_DIR" "$NODE3_DIR" "$STALE_REMOTE" "$STALE_N1" "$STALE_N2"' EXIT

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

echo "=== all passed ==="
