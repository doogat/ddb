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

RESULT=$(gql '{"query":"{ search(query: \"Smoke\") { totalCount hits { id title tags type fields created_at } } }"}')
echo "$RESULT" | grep -q '"search"'
echo "$RESULT" | grep -q '"tags"'
echo "$RESULT" | grep -q '"created_at"'
pass "serve: graphql search with enriched fields"

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

# 18c2. GraphQL updated_at and created_at fields
TS_RESULT=$(gql '{"query":"mutation { createDoogat(input: { title: \"Timestamp Test\" }) { id } }"}')
TS_ID=$(echo "$TS_RESULT" | sed -n 's/.*"id":"\([^"]*\)".*/\1/p')
TS_QUERY=$(gql "{\"query\":\"{ doogat(id: \\\"$TS_ID\\\") { updated_at created_at date } }\"}")
echo "$TS_QUERY" | grep -q '"updated_at"'
echo "$TS_QUERY" | grep -q '"created_at"'
pass "serve: graphql updated_at and created_at fields"

# Verify created_at equals date
TS_DATE=$(echo "$TS_QUERY" | sed -n 's/.*"date":"\([^"]*\)".*/\1/p')
TS_CREATED=$(echo "$TS_QUERY" | sed -n 's/.*"created_at":"\([^"]*\)".*/\1/p')
[ "$TS_DATE" = "$TS_CREATED" ]
pass "serve: created_at equals date"

# Search hits include updated_at
TS_SEARCH=$(gql '{"query":"{ search(query: \"Timestamp Test\") { hits { id updated_at } } }"}')
echo "$TS_SEARCH" | grep -q '"updated_at"'
pass "serve: search hits include updated_at"

gql "{\"query\":\"mutation { deleteDoogat(id: \\\"$TS_ID\\\") }\"}" >/dev/null

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

# 18d2. Search where field filters (materialized columns + tag)
gql '{"query":"mutation { executeSql(sql: \"CREATE TABLE wflink (url TEXT NOT NULL)\") { message } }"}' >/dev/null
WF1=$(gql '{"query":"mutation { executeSql(sql: \"INSERT INTO wflink (title, url) VALUES ('"'"'WFLink Alpha'"'"', '"'"'https://example.com'"'"')\") { message } }"}')
WF1_ID=$(echo "$WF1" | sed -n 's/.*"message":"\([^"]*\)".*/\1/p' | tr -d ' ')
WF2=$(gql '{"query":"mutation { executeSql(sql: \"INSERT INTO wflink (title, url) VALUES ('"'"'WFLink Beta'"'"', '"'"'https://other.org'"'"')\") { message } }"}')
WF2_ID=$(echo "$WF2" | sed -n 's/.*"message":"\([^"]*\)".*/\1/p' | tr -d ' ')
WF3=$(gql '{"query":"mutation { executeSql(sql: \"INSERT INTO wflink (title, url) VALUES ('"'"'WFLink Gamma'"'"', '"'"'https://example.com/page'"'"')\") { message } }"}')
WF3_ID=$(echo "$WF3" | sed -n 's/.*"message":"\([^"]*\)".*/\1/p' | tr -d ' ')

RESULT=$(gql '{"query":"{ search(query: \"WFLink\", where: [{field: \"url\", eq: \"https://example.com\"}]) { totalCount } }"}')
COUNT=$(echo "$RESULT" | sed -n 's/.*"totalCount":\([0-9]*\).*/\1/p')
[ "$COUNT" = "1" ]
pass "serve: search where filter materialized column eq"

RESULT=$(gql '{"query":"{ search(query: \"WFLink\", where: [{field: \"url\", contains: \"example\"}]) { totalCount } }"}')
COUNT=$(echo "$RESULT" | sed -n 's/.*"totalCount":\([0-9]*\).*/\1/p')
[ "$COUNT" = "2" ]
pass "serve: search where filter materialized column contains"

# Tag via where filter
WFT1=$(gql '{"query":"mutation { createDoogat(input: { title: \"WFTag Alpha\", tags: [\"wf-rust\"] }) { id } }"}')
WFT1_ID=$(echo "$WFT1" | sed -n 's/.*"id":"\([^"]*\)".*/\1/p')
WFT2=$(gql '{"query":"mutation { createDoogat(input: { title: \"WFTag Beta\", tags: [\"wf-python\"] }) { id } }"}')
WFT2_ID=$(echo "$WFT2" | sed -n 's/.*"id":"\([^"]*\)".*/\1/p')

RESULT=$(gql '{"query":"{ search(query: \"WFTag\", where: [{field: \"tag\", eq: \"wf-rust\"}]) { totalCount } }"}')
COUNT=$(echo "$RESULT" | sed -n 's/.*"totalCount":\([0-9]*\).*/\1/p')
[ "$COUNT" = "1" ]
pass "serve: search where filter tag eq"

# Combined type + where field filter
RESULT=$(gql '{"query":"{ search(query: \"WFLink\", types: [\"wflink\"], where: [{field: \"url\", eq: \"https://example.com\"}]) { totalCount } }"}')
COUNT=$(echo "$RESULT" | sed -n 's/.*"totalCount":\([0-9]*\).*/\1/p')
[ "$COUNT" = "1" ]
pass "serve: search where filter combined type+field"

gql "{\"query\":\"mutation { deleteDoogat(id: \\\"$WF1_ID\\\") }\"}" >/dev/null
gql "{\"query\":\"mutation { deleteDoogat(id: \\\"$WF2_ID\\\") }\"}" >/dev/null
gql "{\"query\":\"mutation { deleteDoogat(id: \\\"$WF3_ID\\\") }\"}" >/dev/null
gql "{\"query\":\"mutation { deleteDoogat(id: \\\"$WFT1_ID\\\") }\"}" >/dev/null
gql "{\"query\":\"mutation { deleteDoogat(id: \\\"$WFT2_ID\\\") }\"}" >/dev/null

# 18d3. Search where filter: in operator
IN1=$(gql '{"query":"mutation { createDoogat(input: { title: \"InOp Alpha\", tags: [\"in-rust\", \"in-systems\"] }) { id } }"}')
IN1_ID=$(echo "$IN1" | sed -n 's/.*"id":"\([^"]*\)".*/\1/p')
IN2=$(gql '{"query":"mutation { createDoogat(input: { title: \"InOp Beta\", tags: [\"in-python\"] }) { id } }"}')
IN2_ID=$(echo "$IN2" | sed -n 's/.*"id":"\([^"]*\)".*/\1/p')
IN3=$(gql '{"query":"mutation { createDoogat(input: { title: \"InOp Gamma\", tags: [\"in-go\"] }) { id } }"}')
IN3_ID=$(echo "$IN3" | sed -n 's/.*"id":"\([^"]*\)".*/\1/p')

# in with multiple values — should match Alpha (in-rust) and Beta (in-python)
RESULT=$(gql '{"query":"{ search(query: \"InOp\", where: [{field: \"tag\", in: [\"in-rust\", \"in-python\"]}]) { totalCount hits { id } } }"}')
COUNT=$(echo "$RESULT" | sed -n 's/.*"totalCount":\([0-9]*\).*/\1/p')
[ "$COUNT" = "2" ]
echo "$RESULT" | grep -q "$IN1_ID"
echo "$RESULT" | grep -q "$IN2_ID"
pass "serve: search where filter in operator (multiple values)"

# in with single value — should match Gamma only
RESULT=$(gql '{"query":"{ search(query: \"InOp\", where: [{field: \"tag\", in: [\"in-go\"]}]) { totalCount } }"}')
COUNT=$(echo "$RESULT" | sed -n 's/.*"totalCount":\([0-9]*\).*/\1/p')
[ "$COUNT" = "1" ]
pass "serve: search where filter in operator (single value)"

# in with empty array — should match nothing
RESULT=$(gql '{"query":"{ search(query: \"InOp\", where: [{field: \"tag\", in: []}]) { totalCount } }"}')
COUNT=$(echo "$RESULT" | sed -n 's/.*"totalCount":\([0-9]*\).*/\1/p')
[ "$COUNT" = "0" ]
pass "serve: search where filter in operator (empty array)"

# in on materialized column
gql '{"query":"mutation { executeSql(sql: \"CREATE TABLE inlink (url TEXT NOT NULL)\") { message } }"}' >/dev/null
INL1=$(gql '{"query":"mutation { executeSql(sql: \"INSERT INTO inlink (title, url) VALUES ('"'"'InLink A'"'"', '"'"'https://a.example.com'"'"')\") { message } }"}')
INL1_ID=$(echo "$INL1" | sed -n 's/.*"message":"\([^"]*\)".*/\1/p' | tr -d ' ')
INL2=$(gql '{"query":"mutation { executeSql(sql: \"INSERT INTO inlink (title, url) VALUES ('"'"'InLink B'"'"', '"'"'https://b.example.com'"'"')\") { message } }"}')
INL2_ID=$(echo "$INL2" | sed -n 's/.*"message":"\([^"]*\)".*/\1/p' | tr -d ' ')
INL3=$(gql '{"query":"mutation { executeSql(sql: \"INSERT INTO inlink (title, url) VALUES ('"'"'InLink C'"'"', '"'"'https://c.example.com'"'"')\") { message } }"}')
INL3_ID=$(echo "$INL3" | sed -n 's/.*"message":"\([^"]*\)".*/\1/p' | tr -d ' ')

RESULT=$(gql '{"query":"{ search(query: \"InLink\", where: [{field: \"url\", in: [\"https://a.example.com\", \"https://c.example.com\"]}]) { totalCount hits { id } } }"}')
COUNT=$(echo "$RESULT" | sed -n 's/.*"totalCount":\([0-9]*\).*/\1/p')
[ "$COUNT" = "2" ]
echo "$RESULT" | grep -q "$INL1_ID"
echo "$RESULT" | grep -q "$INL3_ID"
pass "serve: search where filter in operator (materialized column)"

gql "{\"query\":\"mutation { deleteDoogat(id: \\\"$IN1_ID\\\") }\"}" >/dev/null
gql "{\"query\":\"mutation { deleteDoogat(id: \\\"$IN2_ID\\\") }\"}" >/dev/null
gql "{\"query\":\"mutation { deleteDoogat(id: \\\"$IN3_ID\\\") }\"}" >/dev/null
gql "{\"query\":\"mutation { deleteDoogat(id: \\\"$INL1_ID\\\") }\"}" >/dev/null
gql "{\"query\":\"mutation { deleteDoogat(id: \\\"$INL2_ID\\\") }\"}" >/dev/null
gql "{\"query\":\"mutation { deleteDoogat(id: \\\"$INL3_ID\\\") }\"}" >/dev/null
gql '{"query":"mutation { executeSql(sql: \"DROP TABLE inlink CASCADE\") { message } }"}' >/dev/null

# 18e. Boolean and phrase search queries
BQ1=$(gql '{"query":"mutation { createDoogat(input: { title: \"BoolSearch Rust CRDT\", content: \"rust crdt patterns\" }) { id } }"}')
BQ1_ID=$(echo "$BQ1" | sed -n 's/.*"id":"\([^"]*\)".*/\1/p')
BQ2=$(gql '{"query":"mutation { createDoogat(input: { title: \"BoolSearch Rust Only\", content: \"rust programming\" }) { id } }"}')
BQ2_ID=$(echo "$BQ2" | sed -n 's/.*"id":"\([^"]*\)".*/\1/p')
BQ3=$(gql '{"query":"mutation { createDoogat(input: { title: \"BoolSearch Golang\", content: \"golang programming\" }) { id } }"}')
BQ3_ID=$(echo "$BQ3" | sed -n 's/.*"id":"\([^"]*\)".*/\1/p')

RESULT=$(gql '{"query":"{ search(query: \"rust AND crdt\") { totalCount } }"}')
COUNT=$(echo "$RESULT" | sed -n 's/.*"totalCount":\([0-9]*\).*/\1/p')
[ "$COUNT" = "1" ]
pass "serve: search boolean AND"

RESULT=$(gql '{"query":"{ search(query: \"rust OR golang\") { totalCount } }"}')
COUNT=$(echo "$RESULT" | sed -n 's/.*"totalCount":\([0-9]*\).*/\1/p')
[ "$COUNT" = "3" ]
pass "serve: search boolean OR"

RESULT=$(gql '{"query":"{ search(query: \"rust NOT crdt\") { totalCount } }"}')
COUNT=$(echo "$RESULT" | sed -n 's/.*"totalCount":\([0-9]*\).*/\1/p')
[ "$COUNT" = "1" ]
pass "serve: search boolean NOT"

RESULT=$(gql '{"query":"{ search(query: \"\\\"rust crdt\\\"\") { totalCount } }"}')
COUNT=$(echo "$RESULT" | sed -n 's/.*"totalCount":\([0-9]*\).*/\1/p')
[ "$COUNT" = "1" ]
pass "serve: search quoted phrase"

RESULT=$(gql '{"query":"{ search(query: \"AND AND\") { totalCount } }"}' 2>&1 || true)
echo "$RESULT" | grep -q "invalid search query"
pass "serve: search malformed query returns error"

gql "{\"query\":\"mutation { deleteDoogat(id: \\\"$BQ1_ID\\\") }\"}" >/dev/null
gql "{\"query\":\"mutation { deleteDoogat(id: \\\"$BQ2_ID\\\") }\"}" >/dev/null
gql "{\"query\":\"mutation { deleteDoogat(id: \\\"$BQ3_ID\\\") }\"}" >/dev/null

# 18g. Search query normalization
RESULT=$(gql '{"query":"{ normalizeSearchQuery(query: \"B AND A\") }"}')
echo "$RESULT" | grep -q '"a and b"'
pass "serve: normalizeSearchQuery sorts AND operands"

RESULT=$(gql '{"query":"{ normalizeSearchQuery(query: \"Tag=svelte AND category=work.portals\") }"}')
echo "$RESULT" | grep -q '"category=work.portals and tag=svelte"'
pass "serve: normalizeSearchQuery sorts field filters"

RESULT=$(gql '{"query":"{ normalizeSearchQuery(query: \"  MEETING   Minutes  \") }"}')
echo "$RESULT" | grep -q '"meeting and minutes"'
pass "serve: normalizeSearchQuery implicit AND and lowercase"

RESULT=$(gql '{"query":"{ search(query: \"rust AND crdt\") { queryNormalized } }"}')
echo "$RESULT" | grep -q '"queryNormalized"'
echo "$RESULT" | grep -q '"crdt and rust"'
pass "serve: search returns queryNormalized"

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

# REST sort: create doogats with distinct titles for sorting
SORT_A=$(curl -sf "$REST_URL/doogats" \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"title":"Charlie Sort","tags":["sorttest"]}' | sed -n 's/.*"id":"\([^"]*\)".*/\1/p')
sleep 0.1
SORT_B=$(curl -sf "$REST_URL/doogats" \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"title":"Alpha Sort","tags":["sorttest"]}' | sed -n 's/.*"id":"\([^"]*\)".*/\1/p')
sleep 0.1
SORT_C=$(curl -sf "$REST_URL/doogats" \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"title":"Bravo Sort","tags":["sorttest"]}' | sed -n 's/.*"id":"\([^"]*\)".*/\1/p')

RESULT=$(rest "/doogats?tag=sorttest&sort=title")
FIRST_TITLE=$(echo "$RESULT" | sed -n 's/.*"data":\[{[^}]*"title":"\([^"]*\)".*/\1/p')
[ "$FIRST_TITLE" = "Alpha Sort" ]
pass "rest: sort by title ascending"

RESULT=$(rest "/doogats?tag=sorttest&sort=-title")
FIRST_TITLE=$(echo "$RESULT" | sed -n 's/.*"data":\[{[^}]*"title":"\([^"]*\)".*/\1/p')
[ "$FIRST_TITLE" = "Charlie Sort" ]
pass "rest: sort by title descending"

# sort=date defaults to descending (newest first)
RESULT=$(rest "/doogats?tag=sorttest&sort=date")
FIRST_ID=$(echo "$RESULT" | sed -n 's/.*"data":\[{[^}]*"id":"\([^"]*\)".*/\1/p')
[ "$FIRST_ID" = "$SORT_C" ]
pass "rest: sort by date descending (default)"

# invalid sort field returns 400
HTTP_CODE=$(curl -so /dev/null -w "%{http_code}" "$REST_URL/doogats?sort=invalid" \
  -H "Authorization: Bearer $TOKEN")
[ "$HTTP_CODE" = "400" ]
pass "rest: sort invalid field returns 400"

# Clean up sort test doogats
curl -sf "$REST_URL/doogats/$SORT_A" -H "Authorization: Bearer $TOKEN" -X DELETE >/dev/null
curl -sf "$REST_URL/doogats/$SORT_B" -H "Authorization: Bearer $TOKEN" -X DELETE >/dev/null
curl -sf "$REST_URL/doogats/$SORT_C" -H "Authorization: Bearer $TOKEN" -X DELETE >/dev/null

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

  # pgwire boolean type: BOOLEAN columns should return t/f
  gql '{"query":"mutation{executeSql(sql:\"CREATE TABLE pgbooltest (label TEXT, active BOOLEAN)\"){message}}"}' >/dev/null
  sleep 1
  gql '{"query":"mutation{executeSql(sql:\"INSERT INTO pgbooltest (label, active) VALUES ('\''yes'\'', true)\"){message}}"}' >/dev/null
  PG_BOOL=$(PGPASSWORD="$TOKEN" psql -h 127.0.0.1 -p "$PG_PORT" -U ddb -d ddb -t -A -c "SELECT active FROM pgbooltest WHERE label = 'yes'")
  echo "$PG_BOOL" | grep -q "t"
  pass "pgwire: boolean type"
  gql '{"query":"mutation{executeSql(sql:\"DROP TABLE pgbooltest CASCADE\"){message}}"}' >/dev/null

  # pgwire \dt hides internal tables
  DT_OUT=$(PGPASSWORD="$TOKEN" psql -h 127.0.0.1 -p "$PG_PORT" -U ddb -d ddb -t -A -c "SELECT relname FROM pg_catalog.pg_class WHERE relkind = 'r'")
  ! echo "$DT_OUT" | grep -q "_ddb_"
  ! echo "$DT_OUT" | grep -q "^doogats$"
  pass "pgwire: \\dt hides internal tables"

  # pgwire: direct access to internal tables still works (hidden, not blocked)
  PGPASSWORD="$TOKEN" psql -h 127.0.0.1 -p "$PG_PORT" -U ddb -d ddb -t -A -c "SELECT COUNT(*) FROM _ddb_tags" | grep -qE "^[0-9]+$"
  pass "pgwire: direct access to internal tables works"
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

# GraphQL introspection hides internal tables
INTRO=$(gql '{"query":"{ __schema { queryType { fields { name } } } }"}')
! echo "$INTRO" | grep -q "_ddb_"
pass "serve: introspection hides internal tables"

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

# 38b2b. raw ID scalar + orderBy/limit on plural references
gql '{"query":"mutation { executeSql(sql: \"CREATE TABLE rlcat (label TEXT)\") { message } }"}' >/dev/null
gql '{"query":"mutation { executeSql(sql: \"CREATE TABLE rlbm (url TEXT, rlcat TEXT REFERENCES rlcat)\") { message } }"}' >/dev/null
RL_C1=$(gql '{"query":"mutation { executeSql(sql: \"INSERT INTO rlcat (label) VALUES (\\\"cherry\\\")\") { message } }"}' | sed -n 's/.*"message":"\([^"]*\)".*/\1/p')
sleep 1
RL_C2=$(gql '{"query":"mutation { executeSql(sql: \"INSERT INTO rlcat (label) VALUES (\\\"apple\\\")\") { message } }"}' | sed -n 's/.*"message":"\([^"]*\)".*/\1/p')
sleep 1
RL_C3=$(gql '{"query":"mutation { executeSql(sql: \"INSERT INTO rlcat (label) VALUES (\\\"banana\\\")\") { message } }"}' | sed -n 's/.*"message":"\([^"]*\)".*/\1/p')
sleep 1
RL_BM=$(gql '{"query":"mutation { executeSql(sql: \"INSERT INTO rlbm (url) VALUES (\\\"https://example.com\\\")\") { message } }"}' | sed -n 's/.*"message":"\([^"]*\)".*/\1/p')
for CID in "$RL_C1" "$RL_C2" "$RL_C3"; do
  gql "{\"query\":\"mutation { executeSql(sql: \\\"INSERT INTO rlbm_rlcat (rlbm_id, rlcat_id) VALUES ('$RL_BM', '$CID')\\\") { message } }\"}" >/dev/null
done
# raw ID scalar
RESULT=$(gql '{"query":"{ rlbms { items { rlcat_id rlcat { id label } } } }"}')
echo "$RESULT" | grep -q "\"rlcat_id\":\"$RL_C1\""
echo "$RESULT" | grep -q "\"label\":\"cherry\""
pass "serve: relation raw ID scalar coexists with object resolver"
# orderBy ASC
RESULT=$(gql '{"query":"{ rlbms { items { rlcats(orderBy: \"label\") { label } } } }"}')
LABELS=$(echo "$RESULT" | sed -n 's/.*"rlcats":\[\(.*\)\].*/\1/p')
echo "$LABELS" | grep -q '"apple".*"banana".*"cherry"'
pass "serve: relation plural orderBy ASC"
# orderBy DESC + limit
RESULT=$(gql '{"query":"{ rlbms { items { rlcats(orderBy: \"label\", orderDir: \"DESC\", limit: 2) { label } } } }"}')
LABELS=$(echo "$RESULT" | sed -n 's/.*"rlcats":\[\(.*\)\].*/\1/p')
echo "$LABELS" | grep -q '"cherry".*"banana"'
# make sure only 2 items (no "apple")
echo "$LABELS" | grep -qv '"apple"'
pass "serve: relation plural orderBy DESC + limit"
gql '{"query":"mutation { executeSql(sql: \"DROP TABLE rlbm CASCADE\") { message } }"}' >/dev/null
gql '{"query":"mutation { executeSql(sql: \"DROP TABLE rlcat CASCADE\") { message } }"}' >/dev/null

# 38b3. typed connection includes tags
gql '{"query":"mutation { executeSql(sql: \"CREATE TABLE tagarticle (topic TEXT)\") { message } }"}' >/dev/null
TA_ID=$(gql '{"query":"mutation { executeSql(sql: \"INSERT INTO tagarticle (topic) VALUES (\\\"rust\\\")\") { message } }"}' | sed -n 's/.*"message":"\([^"]*\)".*/\1/p')
gql "{\"query\":\"mutation { updateDoogat(input: { id: \\\"$TA_ID\\\", tags: [\\\"coding\\\", \\\"systems\\\"] }) { id } }\"}" >/dev/null
RESULT=$(gql '{"query":"{ tagarticles { items { id tags topic } } }"}')
echo "$RESULT" | grep -q '"coding"'
echo "$RESULT" | grep -q '"systems"'
pass "serve: typed connection includes tags"
gql '{"query":"mutation { executeSql(sql: \"DROP TABLE tagarticle CASCADE\") { message } }"}' >/dev/null

# 38b4. tagEntries query with filters
TE1=$(gql '{"query":"mutation { createDoogat(input: { title: \"TagEntry A\", tags: [\"te-rust\", \"te-cli\"] }) { id } }"}' | sed -n 's/.*"id":"\([^"]*\)".*/\1/p')
sleep 1
TE2=$(gql '{"query":"mutation { createDoogat(input: { title: \"TagEntry B\", tags: [\"te-rust\"] }) { id } }"}' | sed -n 's/.*"id":"\([^"]*\)".*/\1/p')
RESULT=$(gql "{\"query\":\"{ tagEntries(where: { doogatId: { eq: \\\"$TE1\\\" } }) { items { doogatId tag } totalCount } }\"}")
echo "$RESULT" | grep -q '"totalCount":2'
pass "serve: tagEntries filter by doogatId eq"

RESULT=$(gql '{"query":"{ tagEntries(where: { tag: { eq: \"te-rust\" } }) { items { tag } totalCount } }"}')
echo "$RESULT" | grep -q '"te-rust"'
TECOUNT=$(echo "$RESULT" | sed -n 's/.*"totalCount":\([0-9]*\).*/\1/p')
[ "$TECOUNT" -ge 2 ]
pass "serve: tagEntries filter by tag eq"

RESULT=$(gql '{"query":"{ tagEntries(where: { tag: { contains: \"te-\" } }) { totalCount } }"}')
TECOUNT=$(echo "$RESULT" | sed -n 's/.*"totalCount":\([0-9]*\).*/\1/p')
[ "$TECOUNT" -ge 3 ]
pass "serve: tagEntries filter by tag contains"

# cleanup
gql "{\"query\":\"mutation { deleteDoogat(id: \\\"$TE1\\\") { id } }\"}" >/dev/null
gql "{\"query\":\"mutation { deleteDoogat(id: \\\"$TE2\\\") { id } }\"}" >/dev/null

# 38c. sql-materialization (columns, boolean normalization, core fields)
RESULT=$(gql '{"query":"{ sql(query: \"SELECT id, title FROM doogats\") { columns rows } }"}')
echo "$RESULT" | grep -q '"columns"'
echo "$RESULT" | grep -q '"id"'
echo "$RESULT" | grep -q '"title"'
pass "serve: sql columns in response"

# 38c2. sql format:objects returns keyed rows
RESULT=$(gql '{"query":"{ sql(query: \"SELECT id, title FROM doogats\", format: \"objects\") { columns rows } }"}')
echo "$RESULT" | grep -q '"id":'
echo "$RESULT" | grep -q '"title":'
pass "serve: sql format objects returns keyed rows"

sleep 1
gql '{"query":"mutation{executeSql(sql:\"CREATE TABLE smokepin (pinned BOOLEAN)\"){message}}"}' >/dev/null
sleep 1
SMOKEPIN_ID=$(gql "{\"query\":\"mutation{executeSql(sql:\\\"INSERT INTO smokepin (title, pinned) VALUES ('PinTest', true)\\\"){message}}\"}" | sed -n 's/.*"message":"\([0-9]*\)".*/\1/p')
[ -n "$SMOKEPIN_ID" ]
RESULT=$(gql "{\"query\":\"{ sql(query: \\\"SELECT pinned FROM smokepin WHERE pinned = 1\\\") { rows } }\"}")
echo "$RESULT" | grep -q '\\"true\\"'
pass "serve: boolean coerced to true/false"

# Boolean false
sleep 1
gql "{\"query\":\"mutation{executeSql(sql:\\\"INSERT INTO smokepin (title, pinned) VALUES ('FalseTest', false)\\\"){message}}\"}" >/dev/null
RESULT=$(gql "{\"query\":\"{ sql(query: \\\"SELECT pinned FROM smokepin WHERE pinned = 0\\\") { rows } }\"}")
echo "$RESULT" | grep -q '\\"false\\"'
pass "serve: boolean false coerced"

RESULT=$(gql '{"query":"{ sql(query: \"SELECT title FROM smokepin\") { rows } }"}')
echo "$RESULT" | grep -q 'PinTest'
pass "serve: core fields in type table"

# 38d. DISTINCT on typed connection queries
# foo table already has one row with bar='val', baz=1. Add more.
gql "{\"query\":\"mutation{executeSql(sql:\\\"INSERT INTO foo (title, bar, baz) VALUES ('dup1', 'val', 2)\\\"){message}}\"}" >/dev/null
gql "{\"query\":\"mutation{executeSql(sql:\\\"INSERT INTO foo (title, bar, baz) VALUES ('uniq', 'other', 3)\\\"){message}}\"}" >/dev/null
sleep 1
RESULT=$(gql '{"query":"{ foos(distinct: \"bar\") { items { bar } totalCount } }"}')
echo "$RESULT" | grep -q '"totalCount":2'
pass "serve: distinct deduplicates and totalCount reflects unique count"

RESULT=$(gql '{"query":"{ foos(distinct: \"bar\", where: { baz: { gte: 2 } }) { totalCount } }"}')
echo "$RESULT" | grep -q '"totalCount":2'
pass "serve: distinct with where filter"

# 38e. GROUP BY on typed aggregate queries
RESULT=$(gql '{"query":"{ foosAggregate(groupBy: \"bar\") { groups { key count } } }"}')
echo "$RESULT" | grep -q '"key":"val"'
echo "$RESULT" | grep -q '"key":"other"'
pass "serve: groupBy returns per-group counts"

RESULT=$(gql '{"query":"{ foosAggregate(groupBy: \"bar\") { groups { key count minBaz maxBaz } } }"}')
echo "$RESULT" | grep -q '"minBaz"'
echo "$RESULT" | grep -q '"maxBaz"'
pass "serve: groupBy with numeric aggregates"

RESULT=$(gql '{"query":"{ foosAggregate(groupBy: \"bar\", where: { baz: { gte: 2 } }) { groups { key count } } }"}')
echo "$RESULT" | grep -q '"key"'
pass "serve: groupBy with where filter"

# Non-grouped still works
RESULT=$(gql '{"query":"{ foosAggregate { count } }"}')
echo "$RESULT" | grep -q '"count":3'
pass "serve: aggregate without groupBy still works"

# 38f. executeBatch mutation
RESULT=$(gql '{"query":"mutation { executeBatch(statements: [\"INSERT INTO foo (title, bar, baz) VALUES ('"'"'batch1'"'"', '"'"'b1'"'"', 10)\", \"INSERT INTO foo (title, bar, baz) VALUES ('"'"'batch2'"'"', '"'"'b2'"'"', 20)\"]) { message affected } }"}')
echo "$RESULT" | grep -qv '"errors"'
pass "serve: executeBatch multiple INSERTs"

# executeBatch with DDL triggers schema reload
RESULT=$(gql '{"query":"mutation { executeBatch(statements: [\"CREATE TABLE batchtest (col1 TEXT)\"]) { message } }"}')
echo "$RESULT" | grep -q '"message"'
sleep 1
RESULT=$(gql '{"query":"{ batchtests { totalCount } }"}')
echo "$RESULT" | grep -q '"totalCount":0'
pass "serve: executeBatch DDL triggers schema reload"

# executeBatch failure rolls back: second INSERT targets non-existent table, first should not persist
PRE_COUNT=$(gql '{"query":"{ foosAggregate { count } }"}' | sed -n 's/.*"count":\([0-9]*\).*/\1/p')
RESULT=$(gql '{"query":"mutation { executeBatch(statements: [\"INSERT INTO foo (title, bar, baz) VALUES ('"'"'rollback_test'"'"', '"'"'rb'"'"', 99)\", \"INSERT INTO no_such_table (title) VALUES ('"'"'bad'"'"')\"]) { message } }"}' || true)
echo "$RESULT" | grep -q '"errors"'
sleep 1
POST_COUNT=$(gql '{"query":"{ foosAggregate { count } }"}' | sed -n 's/.*"count":\([0-9]*\).*/\1/p')
[ "$PRE_COUNT" = "$POST_COUNT" ]
pass "serve: executeBatch failure rolls back all statements"

# cleanup
gql '{"query":"mutation { executeSql(sql: \"DROP TABLE batchtest CASCADE\") { message } }"}' >/dev/null

# 38g. batchUpdate mutation
BU1=$(gql '{"query":"mutation { createDoogat(input: { title: \"BatchUp Alpha\" }) { id } }"}')
BU1_ID=$(echo "$BU1" | sed -n 's/.*"id":"\([^"]*\)".*/\1/p')
BU2=$(gql '{"query":"mutation { createDoogat(input: { title: \"BatchUp Beta\" }) { id } }"}')
BU2_ID=$(echo "$BU2" | sed -n 's/.*"id":"\([^"]*\)".*/\1/p')
BU3=$(gql '{"query":"mutation { createDoogat(input: { title: \"BatchUp Gamma\" }) { id } }"}')
BU3_ID=$(echo "$BU3" | sed -n 's/.*"id":"\([^"]*\)".*/\1/p')

RESULT=$(gql "{\"query\":\"mutation { batchUpdate(updates: [{id: \\\"$BU1_ID\\\", title: \\\"Updated Alpha\\\"}, {id: \\\"$BU2_ID\\\", title: \\\"Updated Beta\\\"}, {id: \\\"$BU3_ID\\\", title: \\\"Updated Gamma\\\"}]) { id title } }\"}")
COUNT=$(echo "$RESULT" | jq '.data.batchUpdate | length')
[ "$COUNT" = "3" ]
pass "serve: batchUpdate returns 3 items"

echo "$RESULT" | jq -e '.data.batchUpdate[] | select(.id == "'"$BU1_ID"'" and .title == "Updated Alpha")' >/dev/null
echo "$RESULT" | jq -e '.data.batchUpdate[] | select(.id == "'"$BU2_ID"'" and .title == "Updated Beta")' >/dev/null
echo "$RESULT" | jq -e '.data.batchUpdate[] | select(.id == "'"$BU3_ID"'" and .title == "Updated Gamma")' >/dev/null
pass "serve: batchUpdate correct titles"

gql "{\"query\":\"mutation { deleteDoogat(id: \\\"$BU1_ID\\\") }\"}" >/dev/null
gql "{\"query\":\"mutation { deleteDoogat(id: \\\"$BU2_ID\\\") }\"}" >/dev/null
gql "{\"query\":\"mutation { deleteDoogat(id: \\\"$BU3_ID\\\") }\"}" >/dev/null

# 38h. createMany mutation
CM_RESULT=$(gql '{"query":"mutation { createMany(inputs: [{title: \"Bulk A\"}, {title: \"Bulk B\"}, {title: \"Bulk C\"}]) { id title } }"}')
COUNT=$(echo "$CM_RESULT" | jq '.data.createMany | length')
[ "$COUNT" = "3" ]
echo "$CM_RESULT" | jq -e '.data.createMany[0].title == "Bulk A"' >/dev/null
echo "$CM_RESULT" | jq -e '.data.createMany[1].title == "Bulk B"' >/dev/null
echo "$CM_RESULT" | jq -e '.data.createMany[2].title == "Bulk C"' >/dev/null
pass "serve: createMany returns 3 items in order"

# verify persistence
CM_ID0=$(echo "$CM_RESULT" | jq -r '.data.createMany[0].id')
VERIFY=$(gql "{\"query\":\"{ doogat(id: \\\"$CM_ID0\\\") { title } }\"}")
echo "$VERIFY" | jq -e '.data.doogat.title == "Bulk A"' >/dev/null
pass "serve: createMany persists records"

# createMany empty
RESULT=$(gql '{"query":"mutation { createMany(inputs: []) { id } }"}')
COUNT=$(echo "$RESULT" | jq '.data.createMany | length')
[ "$COUNT" = "0" ]
pass "serve: createMany empty input"

# cleanup
CM_ID1=$(echo "$CM_RESULT" | jq -r '.data.createMany[1].id')
CM_ID2=$(echo "$CM_RESULT" | jq -r '.data.createMany[2].id')
gql "{\"query\":\"mutation { deleteDoogat(id: \\\"$CM_ID0\\\") }\"}" >/dev/null
gql "{\"query\":\"mutation { deleteDoogat(id: \\\"$CM_ID1\\\") }\"}" >/dev/null
gql "{\"query\":\"mutation { deleteDoogat(id: \\\"$CM_ID2\\\") }\"}" >/dev/null

# Hyphenated type names in GraphQL
gql '{"query":"mutation { executeSql(sql: \"CREATE TABLE \\\"test-widget\\\" (status TEXT, priority INTEGER)\") { message } }"}' >/dev/null
sleep 1
gql '{"query":"mutation { executeSql(sql: \"INSERT INTO \\\"test-widget\\\" (status, priority) VALUES ('"'"'active'"'"', 1)\") { message } }"}' >/dev/null
RESULT=$(gql '{"query":"{ testWidgets { items { id status priority } totalCount } }"}')
echo "$RESULT" | jq -e '.data.testWidgets.totalCount == 1' >/dev/null
echo "$RESULT" | jq -e '.data.testWidgets.items[0].status == "active"' >/dev/null
echo "$RESULT" | jq -e '.data.testWidgets.items[0].priority == 1' >/dev/null
pass "serve: hyphenated type typed query"

# 42. base field filters on typed queries (id, title)
# Insert a row with known title into the existing test-widget type
gql '{"query":"mutation { executeSql(sql: \"INSERT INTO \\\"test-widget\\\" (title, status, priority) VALUES ('"'"'FilterTarget'"'"', '"'"'pending'"'"', 5)\") { message } }"}' >/dev/null
sleep 1

# Get the ID of the row we just inserted
BF_RESULT=$(gql '{"query":"{ testWidgets(where: { title: { eq: \"FilterTarget\" } }) { items { id title } totalCount } }"}')
echo "$BF_RESULT" | jq -e '.data.testWidgets.totalCount == 1' >/dev/null
BF_ID=$(echo "$BF_RESULT" | jq -r '.data.testWidgets.items[0].id')
pass "serve: base field title eq filter"

# Filter by id eq
BF_RESULT=$(gql "{\"query\":\"{ testWidgets(where: { id: { eq: \\\"$BF_ID\\\" } }) { items { id title } totalCount } }\"}")
echo "$BF_RESULT" | jq -e '.data.testWidgets.totalCount == 1' >/dev/null
echo "$BF_RESULT" | jq -e ".data.testWidgets.items[0].id == \"$BF_ID\"" >/dev/null
pass "serve: base field id eq filter"

# Filter by title contains
BF_RESULT=$(gql '{"query":"{ testWidgets(where: { title: { contains: \"Target\" } }) { items { id } totalCount } }"}')
echo "$BF_RESULT" | jq -e '.data.testWidgets.totalCount == 1' >/dev/null
pass "serve: base field title contains filter"

# Nonexistent ID returns empty
BF_RESULT=$(gql '{"query":"{ testWidgets(where: { id: { eq: \"99999999999999\" } }) { items { id } totalCount } }"}')
echo "$BF_RESULT" | jq -e '.data.testWidgets.totalCount == 0' >/dev/null
pass "serve: base field id nonexistent returns empty"

# Cleanup
gql "{\"query\":\"mutation { deleteDoogat(id: \\\"$BF_ID\\\") }\"}" >/dev/null
gql '{"query":"mutation { executeSql(sql: \"DROP TABLE \\\"test-widget\\\"\") { message } }"}' >/dev/null

# 43. SQL INSERT via executeSql defaults date, created_at non-null
gql '{"query":"mutation{executeSql(sql:\"CREATE TABLE datecheck (name TEXT)\"){message}}"}' >/dev/null
DC_RESULT=$(gql '{"query":"mutation{executeSql(sql:\"INSERT INTO datecheck (name) VALUES (\\\"DateTest\\\")\"){message}}"}')
DC_ID=$(echo "$DC_RESULT" | sed -n 's/.*"message":"\([0-9]*\)".*/\1/p')
DC_EXPECTED="$(echo "$DC_ID" | cut -c1-4)-$(echo "$DC_ID" | cut -c5-6)-$(echo "$DC_ID" | cut -c7-8)"
DC_QUERY=$(gql "{\"query\":\"{ datechecks { items { id created_at } } }\"}")
DC_CREATED=$(echo "$DC_QUERY" | sed -n 's/.*"created_at":"\([^"]*\)".*/\1/p')
[ "$DC_CREATED" = "$DC_EXPECTED" ]
pass "serve: SQL INSERT defaults date, created_at matches ID"

# executeBatch also defaults date
EB_RESULT=$(gql '{"query":"mutation{executeBatch(statements:[\"INSERT INTO datecheck (name) VALUES (\\\"BatchTest\\\")\"]){message}}"}')
EB_ID=$(echo "$EB_RESULT" | sed -n 's/.*"message":"\([0-9]*\)".*/\1/p')
EB_EXPECTED="$(echo "$EB_ID" | cut -c1-4)-$(echo "$EB_ID" | cut -c5-6)-$(echo "$EB_ID" | cut -c7-8)"
EB_QUERY=$(gql "{\"query\":\"{ doogat(id: \\\"$EB_ID\\\") { created_at } }\"}")
EB_CREATED=$(echo "$EB_QUERY" | sed -n 's/.*"created_at":"\([^"]*\)".*/\1/p')
[ "$EB_CREATED" = "$EB_EXPECTED" ]
pass "serve: executeBatch INSERT defaults date, created_at matches ID"

# 44. DDL response consistency (no spurious errors)
RESULT=$(gql '{"query":"mutation { executeSql(sql: \"CREATE TABLE ddltest (name VARCHAR(100))\") { columns rows message } }"}')
echo "$RESULT" | grep -qv '"errors"'
echo "$RESULT" | grep -q '"columns":\[\]'
echo "$RESULT" | grep -q '"rows":\[\]'
echo "$RESULT" | grep -q '"message"'
pass "serve: CREATE TABLE response has no errors"

sleep 1
RESULT=$(gql '{"query":"mutation { executeSql(sql: \"ALTER TABLE ddltest ADD COLUMN age INTEGER\") { columns rows message } }"}')
echo "$RESULT" | grep -qv '"errors"'
echo "$RESULT" | grep -q '"columns":\[\]'
echo "$RESULT" | grep -q '"rows":\[\]'
pass "serve: ALTER TABLE response has no errors"

sleep 1
RESULT=$(gql '{"query":"mutation { executeSql(sql: \"DROP TABLE ddltest\") { columns rows message } }"}')
echo "$RESULT" | grep -qv '"errors"'
echo "$RESULT" | grep -q '"columns":\[\]'
echo "$RESULT" | grep -q '"rows":\[\]'
pass "serve: DROP TABLE response has no errors"

RESULT=$(gql '{"query":"mutation { executeBatch(statements: [\"CREATE TABLE ddlbatch1 (name VARCHAR)\", \"CREATE TABLE ddlbatch2 (val INTEGER)\"]) { columns rows message } }"}')
echo "$RESULT" | grep -qv '"errors"'
echo "$RESULT" | grep -q '"columns":\[\]'
echo "$RESULT" | grep -q '"rows":\[\]'
pass "serve: executeBatch DDL responses have no errors"

sleep 1
gql '{"query":"mutation { executeSql(sql: \"DROP TABLE ddlbatch1 CASCADE\") { message } }"}' >/dev/null
gql '{"query":"mutation { executeSql(sql: \"DROP TABLE ddlbatch2 CASCADE\") { message } }"}' >/dev/null

# DML regression: INSERT still returns affected count
DML_RESULT=$(gql '{"query":"mutation { executeSql(sql: \"INSERT INTO datecheck (name) VALUES (\\\"DmlRegression\\\")\") { affected message } }"}')
echo "$DML_RESULT" | grep -qv '"errors"'
echo "$DML_RESULT" | grep -q '"message"'
pass "serve: DML INSERT response unchanged"

# 45. createMany onConflict: IGNORE (upsert via GraphQL)
gql '{"query":"mutation { executeSql(sql: \"CREATE TABLE upsertgql (code TEXT, label TEXT)\") { message } }"}' >/dev/null
sleep 1
# Patch typedef to add unique_together on [code]
cd "$TMPDIR"
UPSERT_TYPEDEF=$(find ddb/_typedef -name '*.md' -exec grep -l 'title: upsertgql' {} \;)
sed -i.bak 's/type: _typedef/type: _typedef\nunique_together:\n  - - code/' "$UPSERT_TYPEDEF"
rm -f "${UPSERT_TYPEDEF}.bak"
git add -A && git commit -m "add unique_together to upsertgql" --quiet
$DDB reindex >/dev/null
CM1=$(gql '{"query":"mutation { createMany(inputs: [{title: \"UpsertA\", type: \"upsertgql\", fields: \"{\\\"code\\\":\\\"X1\\\",\\\"label\\\":\\\"first\\\"}\"}]) { id title } }"}')
CM1_ID=$(echo "$CM1" | jq -r '.data.createMany[0].id')
[ -n "$CM1_ID" ]
CM2=$(gql '{"query":"mutation { createMany(inputs: [{title: \"UpsertA Dup\", type: \"upsertgql\", fields: \"{\\\"code\\\":\\\"X1\\\",\\\"label\\\":\\\"second\\\"}\"}], onConflict: IGNORE) { id title } }"}')
CM2_ID=$(echo "$CM2" | jq -r '.data.createMany[0].id')
[ "$CM2_ID" = "$CM1_ID" ]
CM2_TITLE=$(echo "$CM2" | jq -r '.data.createMany[0].title')
[ "$CM2_TITLE" = "UpsertA" ]
pass "serve: createMany onConflict IGNORE returns existing"
gql '{"query":"mutation { executeSql(sql: \"DROP TABLE upsertgql CASCADE\") { message } }"}' >/dev/null

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

# 40. binary asset LWW conflict resolution
echo "=== binary asset LWW ==="
BIN_REMOTE="$(mktemp -d)"
BIN_NODE1="$(mktemp -d)"
BIN_NODE2="$(mktemp -d)"
git init --bare "$BIN_REMOTE" >/dev/null 2>&1

cd "$BIN_NODE1"
$DDB init . >/dev/null
git remote add origin "$BIN_REMOTE"
$DDB register-node "BinNode1" >/dev/null
# Create a doogat so the repo has content
BIN_DDB_ID=$($DDB create --title "Binary test")
# Create initial binary asset
mkdir -p reference/test
printf '\x89PNG\r\n' > reference/test/photo.bin
git add reference/test/photo.bin
git commit -m "add binary asset" >/dev/null
git push -u origin master >/dev/null 2>&1

# Clone to node2
git clone "$BIN_REMOTE" "$BIN_NODE2" >/dev/null 2>&1
cd "$BIN_NODE2"
$DDB reindex >/dev/null
$DDB register-node "BinNode2" >/dev/null

# Node1: modify binary with higher HLC (later wall_ms)
cd "$BIN_NODE1"
printf 'NODE1_WINS_CONTENT' > reference/test/photo.bin
git add reference/test/photo.bin
git commit -m "node1 update binary

ddb-hlc: 9999999999999.0.BinNode1" >/dev/null
git push origin master >/dev/null 2>&1

# Node2: modify same binary with lower HLC
cd "$BIN_NODE2"
printf 'NODE2_LOSES_CONTENT' > reference/test/photo.bin
git add reference/test/photo.bin
git commit -m "node2 update binary

ddb-hlc: 1000000000000.0.BinNode2" >/dev/null

# Sync node2 — should resolve conflict via LWW, node1 (higher HLC) wins
SYNC_OUT=$($DDB sync origin master)
echo "$SYNC_OUT" | grep -q "conflicts resolved: 1"
RESOLVED=$(cat reference/test/photo.bin)
[ "$RESOLVED" = "NODE1_WINS_CONTENT" ]
pass "binary asset LWW (higher HLC wins)"

# Verify a merge commit exists in recent history (loser preserved)
git log --merges --oneline -1 | grep -q "resolve merge"
pass "binary asset LWW (loser preserved in history)"

rm -rf "$BIN_REMOTE" "$BIN_NODE1" "$BIN_NODE2"

# 41. auto-register node on first sync
AR_REMOTE="$(mktemp -d)"
AR_NODE="$(mktemp -d)"
git init --bare "$AR_REMOTE" >/dev/null
$DDB init "$AR_NODE" >/dev/null
cd "$AR_NODE"
git remote add origin "$AR_REMOTE"
git push -u origin master >/dev/null 2>&1

# No register-node — sync should auto-register
[ ! -f .git/ddb-node ]
$DDB sync origin master >/dev/null
[ -f .git/ddb-node ]
pass "auto-register node on first sync"

# Subsequent sync reuses registration
UUID_BEFORE=$(cat .git/ddb-node)
$DDB sync origin master >/dev/null
UUID_AFTER=$(cat .git/ddb-node)
[ "$UUID_BEFORE" = "$UUID_AFTER" ]
pass "auto-register reuses existing registration"

rm -rf "$AR_REMOTE" "$AR_NODE"

echo "=== all integration tests passed ==="
