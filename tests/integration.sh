#!/usr/bin/env bash
set -euo pipefail

# Integration tests: server, sync, CRDT conflicts, bundles, advanced SQL.
# Runs smoke tests first, then continues with full integration suite.

# --- Run smoke tests first ---
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"

# Build + lint when DDB_BIN is not injected.
PREP_LABEL="prebuilt binary"
if [ -z "${DDB_BIN:-}" ]; then
  cargo build --quiet
  cargo clippy --workspace --quiet
  cargo bench --no-run --quiet 2>/dev/null
  PREP_LABEL="clippy + bench compile"
fi
export DDB_BIN="${DDB_BIN:-$(cargo metadata --format-version=1 --no-deps | sed -n 's/.*"target_directory":"\([^"]*\)".*/\1/p')/debug/ddb}"

"$SCRIPT_DIR/smoke.sh"

# --- Integration tests ---
DDB="$DDB_BIN"

TMPDIR="$(mktemp -d)"
REMOTE_DIR="$(mktemp -d)"
NODE1_DIR="$(mktemp -d)"
NODE2_DIR="$(mktemp -d)"
NODE3_DIR="$(mktemp -d)"
trap 'rm -rf "$TMPDIR" "$REMOTE_DIR" "$NODE1_DIR" "$NODE2_DIR" "$NODE3_DIR"' EXIT
cd "$TMPDIR"

pass() { printf '  ✓ %s\n' "$1"; }

echo "=== integration tests ==="

# Init a repo for server and CLI integration tests
$DDB init . >/dev/null
ID1=$($DDB create --title "First note" --tags "test,smoke" --body "Hello world")
ID2=$($DDB create --title "Links to first" --body "See [[$ID1]]")
$DDB update "$ID1" --title "First note (edited)" --tags "test,smoke,updated"
$DDB update "$ID1" --body "- [ ] open task\n- [x] done task\n- [i] 2026-01-01 10:00 - info note"
$DDB reindex >/dev/null
$DDB query "CREATE TABLE foo (bar TEXT, baz INTEGER)" >/dev/null
$DDB query "INSERT INTO foo (title, bar, baz) VALUES ('for suggest', 'val', 1)" >/dev/null
$DDB register-node "integ-node" >/dev/null

# 17. GraphQL server
SERVER_PORT=$((19200 + (RANDOM % 800)))
PG_PORT=$((SERVER_PORT + 1))
$DDB serve --port "$SERVER_PORT" --pg-port "$PG_PORT" &
SERVER_PID=$!
# Wait for server to start
for i in $(seq 1 20); do
  if curl -sf "http://127.0.0.1:$SERVER_PORT/graphql" \
    -H "Authorization: Bearer $(cat ~/.config/ddb/token 2>/dev/null || echo '')" \
    -H "Content-Type: application/json" \
    -d '{"query":"{ typeDefs { name } }"}' >/dev/null 2>&1; then
    break
  fi
  sleep 0.2
done
TOKEN=$(cat ~/.config/ddb/token 2>/dev/null || echo '')
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
RESULT=$(gql '{"query":"mutation { createDoogat(input: { title: \"Smoke Server\" }) { id title } }"}')
echo "$RESULT" | grep -q '"Smoke Server"'
GQL_ID=$(echo "$RESULT" | sed -n 's/.*"id":"\([^"]*\)".*/\1/p')
pass "serve: graphql create"

# 18. expanded GraphQL operations
RESULT=$(gql "{\"query\":\"mutation { updateDoogat(input: { id: \\\"$GQL_ID\\\", title: \\\"Smoke Updated\\\" }) { id title } }\"}")
echo "$RESULT" | grep -q '"Smoke Updated"'
pass "serve: graphql update"

RESULT=$(gql '{"query":"{ search(query: \"Smoke\") { totalCount hits { id title } } }"}')
echo "$RESULT" | grep -q '"search"'
pass "serve: graphql search"

RESULT=$(gql '{"query":"{ doogats { id title } }"}')
echo "$RESULT" | grep -q '"doogats"'
pass "serve: graphql doogats"

RESULT=$(gql "{\"query\":\"mutation { deleteDoogat(id: \\\"$GQL_ID\\\") }\"}")
echo "$RESULT" | grep -q "true"
pass "serve: graphql delete"

# 18b. GraphQL checkbox queries
RESULT=$(gql '{"query":"{ openActions { state content } }"}')
echo "$RESULT" | grep -q '"openActions"'
pass "serve: graphql openActions"

# 18c. GraphQL tag queries
TAG_RESULT=$(gql '{"query":"mutation { createDoogat(input: { title: \"Tag Test\", tags: [\"alpha\", \"beta\"] }) { id title tags } }"}')
echo "$TAG_RESULT" | grep -q '"alpha"'
TAG_ID=$(echo "$TAG_RESULT" | sed -n 's/.*"id":"\([^"]*\)".*/\1/p')
pass "serve: graphql create with tags"

TAGS_RESULT=$(gql '{"query":"{ tags { name count } }"}')
echo "$TAGS_RESULT" | grep -q '"alpha"'
echo "$TAGS_RESULT" | grep -q '"beta"'
pass "serve: graphql tags query"

FILTERED=$(gql '{"query":"{ doogats(tag: \"alpha\") { id title tags } }"}')
echo "$FILTERED" | grep -q "$TAG_ID"
pass "serve: graphql doogats tag filter"

gql "{\"query\":\"mutation { deleteDoogat(id: \\\"$TAG_ID\\\") }\"}" >/dev/null

# 18d. GraphQL search filters
SF1=$(gql '{"query":"mutation { createDoogat(input: { title: \"SearchFilter Alpha\", type: \"link\", tags: [\"sf-tag\"] }) { id } }"}')
SF1_ID=$(echo "$SF1" | sed -n 's/.*"id":"\([^"]*\)".*/\1/p')
SF2=$(gql '{"query":"mutation { createDoogat(input: { title: \"SearchFilter Beta\", type: \"note\", tags: [\"sf-tag\"] }) { id } }"}')
SF2_ID=$(echo "$SF2" | sed -n 's/.*"id":"\([^"]*\)".*/\1/p')
SF3=$(gql '{"query":"mutation { createDoogat(input: { title: \"SearchFilter Gamma\", type: \"link\" }) { id } }"}')
SF3_ID=$(echo "$SF3" | sed -n 's/.*"id":"\([^"]*\)".*/\1/p')

RESULT=$(gql '{"query":"{ search(query: \"SearchFilter\", types: [\"link\"]) { totalCount hits { id } } }"}')
COUNT=$(echo "$RESULT" | sed -n 's/.*"totalCount":\([0-9]*\).*/\1/p')
[ "$COUNT" = "2" ]
pass "serve: search filter by type"

RESULT=$(gql '{"query":"{ search(query: \"SearchFilter\", tag: \"sf-tag\") { totalCount hits { id } } }"}')
COUNT=$(echo "$RESULT" | sed -n 's/.*"totalCount":\([0-9]*\).*/\1/p')
[ "$COUNT" = "2" ]
pass "serve: search filter by tag"

RESULT=$(gql '{"query":"{ search(query: \"SearchFilter\", types: [\"link\"], tag: \"sf-tag\") { totalCount hits { id } } }"}')
COUNT=$(echo "$RESULT" | sed -n 's/.*"totalCount":\([0-9]*\).*/\1/p')
[ "$COUNT" = "1" ]
pass "serve: search filter combined type+tag"

gql "{\"query\":\"mutation { deleteDoogat(id: \\\"$SF1_ID\\\") }\"}" >/dev/null
gql "{\"query\":\"mutation { deleteDoogat(id: \\\"$SF2_ID\\\") }\"}" >/dev/null
gql "{\"query\":\"mutation { deleteDoogat(id: \\\"$SF3_ID\\\") }\"}" >/dev/null

# 19. REST API CRUD
HTTP_CODE=$(curl -so /dev/null -w "%{http_code}" "$REST_URL/doogats" \
  -H "Content-Type: application/json" \
  -d '{"title":"REST No Auth"}')
[ "$HTTP_CODE" = "401" ]
pass "rest: auth rejects missing token"

RESULT=$(curl -sf -w "\n%{http_code}" "$REST_URL/doogats" \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"title":"REST Smoke","body":"rest body","tags":["rest"]}')
HTTP_CODE=$(echo "$RESULT" | tail -1)
BODY=$(echo "$RESULT" | sed '$d')
[ "$HTTP_CODE" = "201" ]
REST_ID=$(echo "$BODY" | sed -n 's/.*"id":"\([^"]*\)".*/\1/p')
pass "rest: create"

RESULT=$(rest "/doogats/$REST_ID")
echo "$RESULT" | grep -q "REST Smoke"
pass "rest: get"

rest "/doogats/$REST_ID" -X PUT -d '{"title":"REST Updated"}' | grep -q "REST Updated"
pass "rest: update"

RESULT=$(rest "/doogats?tag=rest")
echo "$RESULT" | grep -q "$REST_ID"
pass "rest: list with filter"

# Field filtering: create typed doogat via SQL, filter by field
gql '{"query":"mutation{executeSql(sql:\"CREATE TABLE smokeitem (label TEXT NOT NULL, priority INTEGER)\"){message}}"}'
gql '{"query":"mutation{executeSql(sql:\"INSERT INTO smokeitem (label, priority) VALUES ('\''Smoke1'\'', 7)\"){message}}"}'
RESULT=$(rest "/doogats?field.priority=7")
echo "$RESULT" | grep -q "Smoke1"
pass "rest: field filter"

# Field filter with nonexistent value returns empty
RESULT=$(rest "/doogats?field.priority=999")
echo "$RESULT" | grep -q '"data":\[\]'
pass "rest: field filter no match"

HTTP_CODE=$(curl -so /dev/null -w "%{http_code}" "$REST_URL/doogats/$REST_ID" \
  -H "Authorization: Bearer $TOKEN" -X DELETE)
[ "$HTTP_CODE" = "204" ]
pass "rest: delete"

HTTP_CODE=$(curl -so /dev/null -w "%{http_code}" "$REST_URL/doogats/$REST_ID" \
  -H "Authorization: Bearer $TOKEN")
[ "$HTTP_CODE" = "404" ]
pass "rest: get after delete returns 404"

# 20. PgWire basic query
if command -v psql >/dev/null 2>&1; then
  PGPASSWORD="$TOKEN" psql -h 127.0.0.1 -p "$PG_PORT" -U ddb -d ddb -t -c "SELECT id, title FROM doogats" | grep -q "First note"
  pass "pgwire: select"

  ! PGPASSWORD="wrong" psql -h 127.0.0.1 -p "$PG_PORT" -U ddb -d ddb -c "SELECT 1" 2>/dev/null
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
HTTP_CODE=$(curl -so /dev/null -w "%{http_code}" "$REST_URL/doogats/99990101000000" \
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
gql '{"query":"mutation { createDoogat(input: { title: \"ReadPoolWrite\" }) { id } }"}' > "$WRITE_TMP" &
WRITE_PID=$!
READ_RESULT=$(gql '{"query":"{ doogats { id title } }"}')
echo "$READ_RESULT" | grep -q '"doogats"'
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
RESULT=$(gql '{"query":"{ mvbookmarks { items { id mvcategories { id } } } }"}')
echo "$RESULT" | grep -q "$MV_CAT1"
echo "$RESULT" | grep -q "$MV_CAT2"
pass "serve: graphql multi-value ref list field"
# REST — structured references object
RESULT=$(rest "/doogats/$MV_BM")
echo "$RESULT" | grep -q '"references"'
echo "$RESULT" | grep -q '"mvcategory"'
pass "serve: rest multi-value ref structured json"

# 38b2. REFERENCES relation resolution
gql '{"query":"mutation { executeSql(sql: \"CREATE TABLE smokecat (label TEXT)\") { message } }"}' >/dev/null
gql '{"query":"mutation { executeSql(sql: \"CREATE TABLE smokebm (url TEXT, smokecat TEXT REFERENCES smokecat)\") { message } }"}' >/dev/null
SCAT=$(gql "{\"query\":\"mutation { executeSql(sql: \\\"INSERT INTO smokecat (title, label) VALUES ('Tech', 'tech')\\\") { message } }\"}")
SCAT_ID=$(echo "$SCAT" | sed -n 's/.*"message":"\([^"]*\)".*/\1/p')
sleep 1
SBM=$(gql "{\"query\":\"mutation { executeSql(sql: \\\"INSERT INTO smokebm (title, url) VALUES ('Example', 'https://example.com')\\\") { message } }\"}")
SBM_ID=$(echo "$SBM" | sed -n 's/.*"message":"\([^"]*\)".*/\1/p')
gql "{\"query\":\"mutation { executeSql(sql: \\\"INSERT INTO smokebm_smokecat (smokebm_id, smokecat_id) VALUES ('$SBM_ID', '$SCAT_ID')\\\") { message } }\"}" >/dev/null
RESULT=$(gql '{"query":"{ smokebms { items { smokecat { id label } } } }"}')
echo "$RESULT" | grep -q "\"label\":\"tech\""
pass "serve: relation singular resolves object"
RESULT=$(gql '{"query":"{ smokebms { items { smokecats { id label } } } }"}')
echo "$RESULT" | grep -q "\"label\":\"tech\""
pass "serve: relation plural resolves object list"
sleep 1
gql "{\"query\":\"mutation { executeSql(sql: \\\"INSERT INTO smokebm (title, url) VALUES ('No Cat', 'https://nocat.com')\\\") { message } }\"}" >/dev/null
RESULT=$(gql '{"query":"{ smokebms { items { id smokecat { id } smokecats { id } } } }"}')
echo "$RESULT" | grep -q '"smokecat":null'
echo "$RESULT" | grep -q '"smokecats":\[\]'
pass "serve: relation null returns null and empty list"
gql '{"query":"mutation { executeSql(sql: \"DROP TABLE smokebm CASCADE\") { message } }"}' >/dev/null
gql '{"query":"mutation { executeSql(sql: \"DROP TABLE smokecat CASCADE\") { message } }"}' >/dev/null

# 38c. sql-materialization (columns, boolean normalization, core fields)
RESULT=$(gql '{"query":"{ sql(query: \"SELECT id, title FROM doogats\") { columns rows } }"}')
echo "$RESULT" | grep -q '"columns"'
echo "$RESULT" | grep -q '"id"'
echo "$RESULT" | grep -q '"title"'
pass "serve: sql columns in response"

sleep 1
gql '{"query":"mutation{executeSql(sql:\"CREATE TABLE smokepin (pinned BOOLEAN)\"){message}}"}' >/dev/null
sleep 1
SMOKEPIN_ID=$(gql "{\"query\":\"mutation{executeSql(sql:\\\"INSERT INTO smokepin (title, pinned) VALUES ('PinTest', true)\\\"){message}}\"}" | sed -n 's/.*"message":"\([0-9]*\)".*/\1/p')
[ -n "$SMOKEPIN_ID" ]
RESULT=$(gql "{\"query\":\"{ sql(query: \\\"SELECT pinned FROM smokepin WHERE pinned = 1\\\") { rows } }\"}")
echo "$RESULT" | grep -q '\\"1\\"'
pass "serve: boolean normalized to 1/0"

RESULT=$(gql '{"query":"{ sql(query: \"SELECT title FROM smokepin\") { rows } }"}')
echo "$RESULT" | grep -q 'PinTest'
pass "serve: core fields in type table"

kill "$SERVER_PID" 2>/dev/null || true
wait "$SERVER_PID" 2>/dev/null || true
pass "serve: clean shutdown"

echo "=== sync conflict scenarios ==="

# --- Two-node setup ---
# bare remote
git init --bare "$REMOTE_DIR" >/dev/null 2>&1

# node1: init + push
cd "$NODE1_DIR"
$DDB init . >/dev/null
git remote add origin "$REMOTE_DIR"
$DDB register-node "Laptop" >/dev/null

# 21. fast-forward sync
SYNC_ID=$($DDB create --title "Shared note" --tags "shared" --body "Original body")
git push -u origin master >/dev/null 2>&1

# clone to node2
git clone "$REMOTE_DIR" "$NODE2_DIR" >/dev/null 2>&1
cd "$NODE2_DIR"
# init ddb index without reinitializing git
$DDB reindex >/dev/null
$DDB register-node "Desktop" >/dev/null

$DDB read "$SYNC_ID" | grep -q "Shared note"
pass "fast-forward sync"

# 22. non-overlapping edits (clean git merge, no CRDT)
cd "$NODE1_DIR"
$DDB update "$SYNC_ID" --title "Updated Title" --tags "shared,laptop"

cd "$NODE2_DIR"
$DDB update "$SYNC_ID" --body "Modified body"

cd "$NODE1_DIR"
$DDB sync origin master >/dev/null

cd "$NODE2_DIR"
SYNC_OUT=$($DDB sync origin master)
echo "$SYNC_OUT" | grep -q "conflicts resolved: 0"

$DDB read "$SYNC_ID" | grep -q "Updated Title"
$DDB read "$SYNC_ID" | grep -q "Modified body"
pass "non-overlapping edits (clean merge)"

# 23. frontmatter scalar conflict (title) — CRDT resolves
cd "$NODE1_DIR"
$DDB sync origin master >/dev/null
$DDB update "$SYNC_ID" --title "Laptop Title"

cd "$NODE2_DIR"
$DDB update "$SYNC_ID" --title "Desktop Title"

cd "$NODE1_DIR"
$DDB sync origin master >/dev/null

cd "$NODE2_DIR"
SYNC_OUT=$($DDB sync origin master)
echo "$SYNC_OUT" | grep -q "conflicts resolved: 1"

TITLE=$($DDB read "$SYNC_ID" | grep "^title:")
echo "$TITLE" | grep -qE "(Laptop Title|Desktop Title)"
pass "frontmatter scalar conflict (CRDT)"

# 24. frontmatter list conflict (tags) — CRDT set-union
cd "$NODE1_DIR"
$DDB sync origin master >/dev/null
$DDB update "$SYNC_ID" --tags "base,alpha"

cd "$NODE2_DIR"
$DDB update "$SYNC_ID" --tags "base,beta"

cd "$NODE1_DIR"
$DDB sync origin master >/dev/null

cd "$NODE2_DIR"
$DDB sync origin master >/dev/null

READ_OUT=$($DDB read "$SYNC_ID")
echo "$READ_OUT" | grep -q "alpha"
echo "$READ_OUT" | grep -q "beta"
pass "frontmatter list conflict (tag union)"

# 25. body conflict — Automerge Text CRDT
cd "$NODE1_DIR"
$DDB sync origin master >/dev/null
$DDB update "$SYNC_ID" --body $'Line one LAPTOP.\nLine two.\nLine three.'

cd "$NODE2_DIR"
$DDB update "$SYNC_ID" --body $'Line one.\nLine two DESKTOP.\nLine three.'

cd "$NODE1_DIR"
$DDB sync origin master >/dev/null

cd "$NODE2_DIR"
SYNC_OUT=$($DDB sync origin master)
echo "$SYNC_OUT" | grep -q "conflicts resolved: 1"

READ_OUT=$($DDB read "$SYNC_ID")
echo "$READ_OUT" | grep -q "LAPTOP"
echo "$READ_OUT" | grep -q "DESKTOP"
pass "body conflict (CRDT text merge)"

# 26. reference section conflict — write files directly, CRDT union
cd "$NODE1_DIR"
$DDB sync origin master >/dev/null

DOOGAT_FILE="ddb/${SYNC_ID}.md"

# node1: append reference section with laptop-specific field
CONTENT=$(cat "$DOOGAT_FILE")
printf '%s\n---\n- laptop note:: Added from laptop\n' "$CONTENT" > "$DOOGAT_FILE"
git add "$DOOGAT_FILE" && git commit -m "node1 add reference" >/dev/null 2>&1
git push origin master >/dev/null 2>&1

# node2: append different reference field (from its pre-push version)
cd "$NODE2_DIR"
CONTENT=$(cat "$DOOGAT_FILE")
printf '%s\n---\n- desktop note:: Added from desktop\n' "$CONTENT" > "$DOOGAT_FILE"
git add "$DOOGAT_FILE" && git commit -m "node2 add reference" >/dev/null 2>&1

SYNC_OUT=$($DDB sync origin master)
echo "$SYNC_OUT" | grep -q "conflicts resolved: 1"

READ_OUT=$($DDB read "$SYNC_ID")
echo "$READ_OUT" | grep -q "laptop note"
echo "$READ_OUT" | grep -q "desktop note"
pass "reference section conflict (CRDT union)"

# 27b. delete-vs-edit conflict — edit wins, doogat resurrected
cd "$NODE1_DIR"
$DDB sync origin master >/dev/null
DEL_ID=$($DDB create --title "To be deleted" --body "Original content")
$DDB sync origin master >/dev/null

cd "$NODE2_DIR"
$DDB sync origin master >/dev/null
$DDB read "$DEL_ID" | grep -q "To be deleted"

# node1 deletes, node2 edits
cd "$NODE1_DIR"
$DDB delete "$DEL_ID"

cd "$NODE2_DIR"
$DDB update "$DEL_ID" --body "Edited on desktop"

cd "$NODE1_DIR"
$DDB sync origin master >/dev/null

cd "$NODE2_DIR"
$DDB sync origin master >/dev/null

# Edit wins: doogat exists and is marked resurrected
$DDB read "$DEL_ID" | grep -q "Edited on desktop"
$DDB status | grep -q "resurrected"
pass "delete-vs-edit conflict (edit wins, resurrected)"

echo "=== id collision detection ==="

# 27a. add-add collision: both doogats survive
# Use a fresh pair of repos to avoid interference with earlier sync state.
COLL_REMOTE="$(mktemp -d)"
COLL_A="$(mktemp -d)"
COLL_B_PARENT="$(mktemp -d)"
git init --bare "$COLL_REMOTE" >/dev/null 2>&1

# Node A: init, push, register
cd "$COLL_A"
$DDB init . >/dev/null
git remote add origin "$COLL_REMOTE"
$DDB register-node "CollA" >/dev/null
git push -u origin master >/dev/null 2>&1

# Node B: clone, register
git clone "$COLL_REMOTE" "$COLL_B_PARENT/repo" >/dev/null 2>&1
COLL_B="$COLL_B_PARENT/repo"
cd "$COLL_B"
$DDB reindex >/dev/null
$DDB register-node "CollB" >/dev/null
git push origin master >/dev/null 2>&1

# Sync A to pick up B's node
cd "$COLL_A"
$DDB sync origin master >/dev/null

# Both create the same-ID doogat independently
COLL_ID="20260101120000"
cd "$COLL_A"
mkdir -p ddb
printf -- '---\nid: %s\ntitle: From A\ndate: 2026-01-01\n---\nBody A\n' "$COLL_ID" > "ddb/${COLL_ID}.md"
git add "ddb/${COLL_ID}.md"
git commit -m "A creates $COLL_ID" >/dev/null 2>&1
git push origin master >/dev/null 2>&1

cd "$COLL_B"
mkdir -p ddb
printf -- '---\nid: %s\ntitle: From B\ndate: 2026-01-01\n---\nBody B\n' "$COLL_ID" > "ddb/${COLL_ID}.md"
git add "ddb/${COLL_ID}.md"
git commit -m "B creates $COLL_ID" >/dev/null 2>&1

# B syncs — collision detected and resolved
COLL_OUT=$($DDB sync origin master)
echo "$COLL_OUT" | grep -q "collisions reassigned: 1"
COLL_COUNT=$(find ddb -name '*.md' ! -name '_*' | wc -l | tr -d ' ')
[ "$COLL_COUNT" -eq 2 ]
pass "add-add collision: both doogats survive"

echo "=== bundle sync ==="

# 27. bundle export --full + import
cd "$NODE1_DIR"
$DDB sync origin master >/dev/null
$DDB bundle export --full --output "$TMPDIR/full-bundle.tar"
echo "$TMPDIR/full-bundle.tar" | grep -q "full-bundle.tar"

cd "$NODE3_DIR"
$DDB init . >/dev/null
$DDB register-node "Tablet" >/dev/null
$DDB bundle import "$TMPDIR/full-bundle.tar" | grep -q "imported"
$DDB read "$SYNC_ID" | grep -q "laptop note"
pass "bundle export --full + import"

# 28. delta bundle export + import
cd "$NODE1_DIR"
DELTA_ID=$($DDB create --title "Delta note" --body "only in delta")

NODE2_UUID=$(cd "$NODE2_DIR" && cat .git/ddb-node)
$DDB bundle export --target "$NODE2_UUID" --output "$TMPDIR/delta-bundle.tar"

cd "$NODE2_DIR"
$DDB bundle import "$TMPDIR/delta-bundle.tar" | grep -q "imported"
$DDB read "$DELTA_ID" | grep -q "Delta note"
pass "delta bundle export + import"

# 29. update-bin help + rollback
$DDB update-bin --help | grep -q "Update ddb"
$DDB update-bin --help | grep -q "\-\-rollback"
pass "update-bin --help (includes --rollback)"

# rollback with no backup should fail gracefully
ROLLBACK_OUT=$($DDB update-bin --rollback 2>&1 || true)
echo "$ROLLBACK_OUT" | grep -q "no backup"
pass "update-bin --rollback (no backup error)"

# 30. ALTER TABLE + DROP TABLE + bulk UPDATE/DELETE
cd "$TMPDIR"
$DDB query "CREATE TABLE smokealt (name TEXT, score INTEGER)" | grep -q "table smokealt created"
$DDB query "INSERT INTO smokealt (name, score) VALUES ('a', 1)" >/dev/null
sleep 1
$DDB query "INSERT INTO smokealt (name, score) VALUES ('b', 2)" >/dev/null
$DDB query "ALTER TABLE smokealt ADD COLUMN tag TEXT" | grep -q "altered"
$DDB query "SELECT name, tag FROM smokealt" | grep -q "NULL"
$DDB query "ALTER TABLE smokealt RENAME COLUMN tag TO label" | grep -q "renamed"
$DDB query "SELECT name, label FROM smokealt" | grep -q "a"
$DDB query "UPDATE smokealt SET score = 99 WHERE name = 'a'" | grep -q "1 row(s) affected"
$DDB query "DELETE FROM smokealt WHERE name = 'b'" | grep -q "1 row(s) affected"
$DDB query "DROP TABLE smokealt CASCADE" | grep -q "dropped"
pass "alter/drop table + bulk ops"

# 31. file attachments
cd "$TMPDIR"
echo "hello attachment" > $TMPDIR/ddb-smoke-attach.txt
$DDB attach "$ID1" $TMPDIR/ddb-smoke-attach.txt | grep -q "attached"
$DDB attachments "$ID1" | grep -q "ddb-smoke-attach.txt"
$DDB attachments "$ID1" | grep -q "text/plain"
$DDB query "SELECT name, mime FROM _ddb_attachments WHERE doogat_id = '$ID1'" | grep -q "ddb-smoke-attach.txt"
$DDB detach "$ID1" "ddb-smoke-attach.txt" | grep -q "detached"
$DDB attachments "$ID1" | grep -q "no attachments"
rm -f $TMPDIR/ddb-smoke-attach.txt
pass "file attachments (attach/list/query/detach)"

# 32. NoSQL CLI commands
cd "$TMPDIR"
$DDB get "$ID1" | grep -q "First note (edited)"
pass "nosql: get"

$DDB scan --tag test | grep -q "$ID1"
pass "nosql: scan --tag"

$DDB scan --type foo | grep -qE "^[0-9]{14}$"
pass "nosql: scan --type"

$DDB backlinks "$ID1" | grep -q "$ID2"
pass "nosql: backlinks"

# 33. stale node resync after compaction
echo "=== stale node resync ==="
STALE_REMOTE="$(mktemp -d)"
STALE_N1="$(mktemp -d)"
STALE_N2="$(mktemp -d)"
trap 'rm -rf "$TMPDIR" "$REMOTE_DIR" "$NODE1_DIR" "$NODE2_DIR" "$NODE3_DIR" "$STALE_REMOTE" "$STALE_N1" "$STALE_N2" "$COLL_REMOTE" "$COLL_A" "$COLL_B_PARENT"' EXIT

git init --bare "$STALE_REMOTE" >/dev/null 2>&1

cd "$STALE_N1"
$DDB init . >/dev/null
git remote add origin "$STALE_REMOTE"
$DDB register-node "StaleNode1" >/dev/null
STALE_ID=$($DDB create --title "Stale shared" --body "original content")
git push -u origin master >/dev/null 2>&1

git clone "$STALE_REMOTE" "$STALE_N2" >/dev/null 2>&1
cd "$STALE_N2"
$DDB reindex >/dev/null
$DDB register-node "StaleNode2" >/dev/null

# Both nodes edit the same doogat → conflict
cd "$STALE_N1"
$DDB update "$STALE_ID" --body "body from node1"
git push origin master >/dev/null 2>&1

cd "$STALE_N2"
$DDB update "$STALE_ID" --body "body from node2"
$DDB sync origin master >/dev/null

# Compact to remove CRDT temp files — verify report includes byte stats
COMPACT_OUT=$($DDB compact --force)
echo "$COMPACT_OUT" | grep -q "crdt temp:"
echo "$COMPACT_OUT" | grep -q "repo (.git):"

# Create another conflict without CRDT state
cd "$STALE_N1"
$DDB sync origin master >/dev/null
$DDB update "$STALE_ID" --body "second edit node1"
git push origin master >/dev/null 2>&1

cd "$STALE_N2"
$DDB update "$STALE_ID" --body "second edit node2"
$DDB sync origin master >/dev/null

# Verify doogat is readable and valid
$DDB read "$STALE_ID" | grep -q "title:"
pass "stale node resync after compaction"

# 34. multi-row INSERT
cd "$TMPDIR"
$DDB query "CREATE TABLE multirow (name TEXT, val INTEGER)" | grep -q "table multirow created"
MULTI_IDS=$($DDB query "INSERT INTO multirow (name, val) VALUES ('a', 1), ('b', 2), ('c', 3)")
echo "$MULTI_IDS" | grep -qE "^[0-9]{14},[0-9]{14},[0-9]{14}$"
$DDB query "SELECT COUNT(*) FROM multirow" | grep -q "3"
pass "multi-row insert"

# 35. transaction commit + rollback
cd "$TMPDIR"
$DDB query "CREATE TABLE txntest (val TEXT)" | grep -q "table txntest created"
$DDB query "BEGIN; INSERT INTO txntest (val) VALUES ('committed'); COMMIT" | grep -q "COMMIT"
$DDB query "SELECT val FROM txntest" | grep -q "committed"
$DDB query "BEGIN; INSERT INTO txntest (val) VALUES ('rolled-back'); ROLLBACK" | grep -q "ROLLBACK"
# rolled-back row should not appear
TXNCOUNT=$($DDB query "SELECT COUNT(*) FROM txntest")
echo "$TXNCOUNT" | grep -q "1"
pass "transaction commit + rollback"

# 36. hyphenated type SQL via quoted identifiers
cd "$TMPDIR"
$DDB query 'CREATE TABLE "my-type" (label TEXT)' | grep -q "table my-type created"
MY_ID=$($DDB query 'INSERT INTO "my-type" (label) VALUES ('\''test'\'')')
$DDB query 'SELECT label FROM "my-type"' | grep -q "test"
$DDB query "DELETE FROM \"my-type\" WHERE id = '$MY_ID'" | grep -q "1 row(s) affected"
pass "hyphenated type SQL"

# 37. junction table CRUD (multi-value references)
cd "$TMPDIR"
$DDB query "CREATE TABLE jtag (name VARCHAR(100))" | grep -q "table jtag created"
$DDB query "CREATE TABLE jpost (url TEXT, jtag TEXT REFERENCES jtag)" | grep -q "table jpost created"
JT_TAG_ID=$($DDB query "INSERT INTO jtag (name) VALUES ('rust')")
sleep 1
JT_POST_ID=$($DDB query "INSERT INTO jpost (url) VALUES ('https://example.com')")
$DDB query "INSERT INTO jpost_jtag (jpost_id, jtag_id) VALUES ('$JT_POST_ID', '$JT_TAG_ID')" | grep -q "1 row"
$DDB query "SELECT jtag_id FROM jpost_jtag WHERE jpost_id = '$JT_POST_ID'" | grep -q "$JT_TAG_ID"
$DDB query "DELETE FROM jpost_jtag WHERE jpost_id = '$JT_POST_ID' AND jtag_id = '$JT_TAG_ID'" | grep -q "1 row"
$DDB query "SELECT COUNT(*) FROM jpost_jtag" | grep -q "0"
$DDB query "INSERT INTO jpost_jtag (jpost_id, jtag_id) VALUES ('$JT_POST_ID', '$JT_TAG_ID')" >/dev/null
$DDB query "DROP TABLE jpost CASCADE" | grep -q "dropped"
! $DDB query "SELECT * FROM jpost_jtag" 2>/dev/null
pass "junction table CRUD"

# 38. title template compliance check
cd "$TMPDIR"
$DDB query "CREATE TABLE smwidget (name VARCHAR(100), description TEXT)" >/dev/null
$DDB query "ALTER TABLE smwidget SET TITLE TEMPLATE '{name} Widget'" >/dev/null
$DDB query "INSERT INTO smwidget (name, description) VALUES ('Foo', 'A foo widget')" >/dev/null
$DDB fix --verbose --dry-run | grep -q "title does not match template"
pass "title template compliance check"

# 39. zone migration
cd "$TMPDIR"
$DDB query "CREATE TABLE gadget (notes TEXT)" | grep -q "table gadget created"
$DDB query "INSERT INTO gadget (notes) VALUES ('Some notes')" >/dev/null
$DDB query "ALTER TABLE gadget SET ZONE frontmatter FOR notes" >/dev/null
FIX_OUT=$($DDB fix --migrate --verbose)
echo "$FIX_OUT" | grep -q "zone-migrated"
pass "zone migration"

echo "=== all integration tests passed ==="
