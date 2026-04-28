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
# Extract the "message" field of a successful executeSql response (which is the
# new doogat id). Mirrors the helper from
# dev/local/specs/jink-feedback/ddb-repros/_lib.sh:91 so jink-port checks can
# use it verbatim.
extract_id() {
  grep -o '"message":"[^"]*"' | head -1 | sed 's/"message":"//;s/"$//'
}
# Assert that the given response contains "errors" (i.e. GraphQL rejected the
# call). Returns 0 if errors are present, 1 otherwise. Prints the response on
# failure so set -e callers get diagnostic context.
assert_gql_errors() {
  local resp="$1"
  if printf '%s' "$resp" | grep -q '"errors"'; then
    return 0
  fi
  printf '  ✗ assert_gql_errors: response had no "errors" key\n    response: %s\n' "$resp" >&2
  return 1
}
# Assert that the given response is a successful GraphQL response (has "data"
# and no "errors"). Returns 0 if both hold, 1 otherwise.
assert_gql_ok() {
  local resp="$1"
  if printf '%s' "$resp" | grep -q '"errors"'; then
    printf '  ✗ assert_gql_ok: response had "errors"\n    response: %s\n' "$resp" >&2
    return 1
  fi
  if ! printf '%s' "$resp" | grep -q '"data"'; then
    printf '  ✗ assert_gql_ok: response had no "data" key\n    response: %s\n' "$resp" >&2
    return 1
  fi
  return 0
}
rest() {
  curl -sf "$REST_URL$1" \
    -H "Authorization: Bearer $TOKEN" \
    -H "Content-Type: application/json" \
    "${@:2}"
}
# Wait for the GraphQL schema to reload after a DDL statement. Polls
# schemaVersion until it exceeds $1 (the version captured before the DDL).
# Times out after 4 seconds (40 x 100ms).
wait_schema_reload() {
  local before="$1"
  for i in $(seq 1 40); do
    local ver
    ver=$(gql '{"query":"{ schemaVersion }"}' | sed -n 's/.*"schemaVersion":\([0-9]*\).*/\1/p')
    ver=${ver:-0}
    [ "$ver" -gt "$before" ] && return 0
    sleep 0.1
  done
  printf '  ✗ wait_schema_reload: version did not advance past %s within 4s\n' "$before" >&2
  return 1
}
ddl() {
  local ver
  ver=$(gql '{"query":"{ schemaVersion }"}' | sed -n 's/.*"schemaVersion":\([0-9]*\).*/\1/p')
  ver=${ver:-0}
  gql "$1" >/dev/null
  wait_schema_reload "$ver"
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

# 17.J1 — Jink schema CREATE TABLE definitions (#9 jink full-sweep section 1).
# Seven table definitions ported from
# dev/local/specs/jink-feedback/ddb-repros/validate-full-sweep.sh lines 34-60.
# The tables persist through sub-blocks 17.J2, 18z8, 18z9, 18z10 below; task
# 20 drops them all at the end. DO NOT add DROP TABLE statements between
# these sub-blocks.
VER=$(gql '{"query":"{ schemaVersion }"}' | sed -n 's/.*"schemaVersion":\([0-9]*\).*/\1/p')
VER=${VER:-0}
J1_LINK=$(gql '{"query":"mutation { executeSql(sql: \"CREATE TABLE link (title VARCHAR(255) NOT NULL, url VARCHAR(255) NOT NULL, subtitle VARCHAR(255), favicon_path VARCHAR(255), favicon_origin VARCHAR(255), bookmark_source VARCHAR(255), last_opened_at VARCHAR(255), description TEXT)\") { message } }"}')
assert_gql_ok "$J1_LINK"
printf '%s' "$J1_LINK" | grep -q '"message":"table link'
pass "j1: created link table"

J1_CAT=$(gql '{"query":"mutation { executeSql(sql: \"CREATE TABLE category (title VARCHAR(255) NOT NULL, fqn VARCHAR(255) NOT NULL, space VARCHAR(255) NOT NULL, sort_order INTEGER DEFAULT 0)\") { message } }"}')
assert_gql_ok "$J1_CAT"
printf '%s' "$J1_CAT" | grep -q '"message":"table category'
pass "j1: created category table"

J1_CM=$(gql '{"query":"mutation { executeSql(sql: \"CREATE TABLE \\\"category-membership\\\" (title VARCHAR(255) NOT NULL, link_id VARCHAR(255) NOT NULL, category_fqn VARCHAR(255) NOT NULL, pinned BOOLEAN DEFAULT FALSE, sort_order INTEGER DEFAULT 0, UNIQUE(link_id, category_fqn))\") { message } }"}')
assert_gql_ok "$J1_CM"
printf '%s' "$J1_CM" | grep -q '"message":"table category-membership'
pass "j1: created category-membership table with composite UNIQUE"

J1_Q=$(gql '{"query":"mutation { executeSql(sql: \"CREATE TABLE quote (title VARCHAR(255) NOT NULL, author VARCHAR(255), source VARCHAR(255), favorited BOOLEAN DEFAULT FALSE, text TEXT)\") { message } }"}')
assert_gql_ok "$J1_Q"
printf '%s' "$J1_Q" | grep -q '"message":"table quote'
pass "j1: created quote table"

J1_SS=$(gql '{"query":"mutation { executeSql(sql: \"CREATE TABLE \\\"saved-search\\\" (title VARCHAR(255) NOT NULL, query_raw VARCHAR(255) NOT NULL, query_normalized VARCHAR(255) NOT NULL)\") { message } }"}')
assert_gql_ok "$J1_SS"
printf '%s' "$J1_SS" | grep -q '"message":"table saved-search'
pass "j1: created saved-search table"

J1_PR=$(gql '{"query":"mutation { executeSql(sql: \"CREATE TABLE \\\"pinned-result\\\" (title VARCHAR(255) NOT NULL, query_normalized VARCHAR(255) NOT NULL, link_id VARCHAR(255) NOT NULL, sort_order INTEGER DEFAULT 0)\") { message } }"}')
assert_gql_ok "$J1_PR"
printf '%s' "$J1_PR" | grep -q '"message":"table pinned-result'
pass "j1: created pinned-result table"

J1_JC=$(gql '{"query":"mutation { executeSql(sql: \"CREATE TABLE \\\"jink-config\\\" (dashboard_title VARCHAR(255) DEFAULT '\''Bobs Battlestation'\'', quote_rotation_minutes INTEGER DEFAULT 30, links_per_category INTEGER DEFAULT 8, frontend_version VARCHAR(255))\") { message } }"}')
assert_gql_ok "$J1_JC"
printf '%s' "$J1_JC" | grep -q '"message":"table jink-config'
pass "j1: created jink-config table"

wait_schema_reload "$VER"

# 17.J2 — jink-config singleton + Link CRUD (#9 jink full-sweep sections 3-4).
# Ported from validate-full-sweep.sh lines 70-100. Uses the jink tables from
# J1; captures JINK_LINK_ID for use by later sub-blocks (18z8, 18z10).

# jink-config singleton (3 checks from sweep section 3).
J2_JC_EMPTY=$(gql '{"query":"{ sql(query: \"SELECT id FROM \\\"jink-config\\\" LIMIT 1\") { rows } }"}')
assert_gql_ok "$J2_JC_EMPTY"
printf '%s' "$J2_JC_EMPTY" | grep -q '"rows"'
pass "j2: SELECT from empty jink-config"

J2_JC_INS=$(gql '{"query":"mutation { executeSql(sql: \"INSERT INTO \\\"jink-config\\\" (title, dashboard_title, quote_rotation_minutes, links_per_category) VALUES (\\\"jink-config\\\", \\\"Bobs Battlestation\\\", 30, 8)\") { message } }"}')
assert_gql_ok "$J2_JC_INS"
printf '%s' "$J2_JC_INS" | grep -qE '"message":"[0-9]+"'
pass "j2: INSERT jink-config singleton"

J2_JC_SEL=$(gql '{"query":"{ sql(query: \"SELECT quote_rotation_minutes FROM \\\"jink-config\\\" LIMIT 1\") { rows } }"}')
assert_gql_ok "$J2_JC_SEL"
printf '%s' "$J2_JC_SEL" | grep -qF '\"30\"'
pass "j2: SELECT quote_rotation_minutes returns 30"

# Link CRUD (3 checks from sweep section 4).
J2_LINK_INS=$(gql '{"query":"mutation { executeSql(sql: \"INSERT INTO link (title, url, description) VALUES (\\\"Test Link\\\", \\\"https://example.com\\\", \\\"a test link\\\")\") { message } }"}')
assert_gql_ok "$J2_LINK_INS"
JINK_LINK_ID=$(printf '%s' "$J2_LINK_INS" | extract_id)
[ -n "$JINK_LINK_ID" ]
printf '%s' "$J2_LINK_INS" | grep -qE '"message":"[0-9]+"'
pass "j2: INSERT link returns id"

J2_LINK_GQL=$(gql "{\"query\":\"{ links(where: {id: {eq: \\\"$JINK_LINK_ID\\\"}}) { items { id title url description tags } } }\"}")
assert_gql_ok "$J2_LINK_GQL"
printf '%s' "$J2_LINK_GQL" | grep -q '"Test Link"'
pass "j2: query links via GraphQL"

J2_LINK_UPD=$(gql "{\"query\":\"mutation { executeSql(sql: \\\"UPDATE link SET favicon_path = 'favicon/x.png', favicon_origin = 'fetched' WHERE id = '$JINK_LINK_ID' AND url = 'https://example.com'\\\") { message affected } }\"}")
assert_gql_ok "$J2_LINK_UPD"
printf '%s' "$J2_LINK_UPD" | grep -q '"affected":1'
pass "j2: UPDATE link favicon via compound-predicate SQL"

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
# Section j1 above registered the `link` typedef with url NOT NULL, so typed
# creates must supply url. SF2 stays untyped since `note` is not a registered
# typedef and PRD 00129 rejects unregistered types from GraphQL createDoogat.
SF1=$(gql '{"query":"mutation { createDoogat(input: { title: \"SearchFilter Alpha\", type: \"link\", tags: [\"sf-tag\"], fields: \"{\\\"url\\\":\\\"https://example.com/sf1\\\"}\" }) { id } }"}')
SF1_ID=$(echo "$SF1" | sed -n 's/.*"id":"\([^"]*\)".*/\1/p')
SF2=$(gql '{"query":"mutation { createDoogat(input: { title: \"SearchFilter Beta\", tags: [\"sf-tag\"] }) { id } }"}')
SF2_ID=$(echo "$SF2" | sed -n 's/.*"id":"\([^"]*\)".*/\1/p')
SF3=$(gql '{"query":"mutation { createDoogat(input: { title: \"SearchFilter Gamma\", type: \"link\", fields: \"{\\\"url\\\":\\\"https://example.com/sf3\\\"}\" }) { id } }"}')
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

# 18h. In-query field-filter alignment + error-class consistency (PRD 00121)
PRD121A=$(gql '{"query":"mutation { createDoogat(input: { title: \"PRD121 Alpha\", tags: [\"prd121-rust\"] }) { id } }"}')
PRD121A_ID=$(echo "$PRD121A" | sed -n 's/.*"id":"\([^"]*\)".*/\1/p')
PRD121B=$(gql '{"query":"mutation { createDoogat(input: { title: \"PRD121 Beta\", tags: [\"prd121-python\"] }) { id } }"}')
PRD121B_ID=$(echo "$PRD121B" | sed -n 's/.*"id":"\([^"]*\)".*/\1/p')
PRD121G=$(gql '{"query":"mutation { createDoogat(input: { title: \"PRD121 Gamma\", tags: [\"prd121-rust\", \"prd121-cli\"] }) { id } }"}')
PRD121G_ID=$(echo "$PRD121G" | sed -n 's/.*"id":"\([^"]*\)".*/\1/p')

RESULT=$(gql '{"query":"{ search(query: \"tag=prd121-rust\") { totalCount hits { id } } }"}')
COUNT=$(echo "$RESULT" | sed -n 's/.*"totalCount":\([0-9]*\).*/\1/p')
[ "$COUNT" = "2" ]
pass "serve: search in-query tag filter returns matching set"

RESULT_INQ=$(gql '{"query":"{ search(query: \"tag=prd121-rust\") { hits { id } } }"}')
RESULT_WHERE=$(gql '{"query":"{ search(query: \"\", where: [{field: \"tag\", eq: \"prd121-rust\"}]) { hits { id } } }"}')
IDS_INQ=$(echo "$RESULT_INQ" | sed -n 's/.*"id":"\([^"]*\)".*/\1/p' | sort)
IDS_WHERE=$(echo "$RESULT_WHERE" | sed -n 's/.*"id":"\([^"]*\)".*/\1/p' | sort)
[ "$IDS_INQ" = "$IDS_WHERE" ]
pass "serve: in-query tag filter matches where-arg tag filter"

RESULT=$(gql '{"query":"{ search(query: \"PRD121 tag=prd121-rust\") { totalCount } }"}')
COUNT=$(echo "$RESULT" | sed -n 's/.*"totalCount":\([0-9]*\).*/\1/p')
[ "$COUNT" = "2" ]
pass "serve: search text AND in-query tag filter"

RESULT=$(gql '{"query":"{ search(query: \"tag=prd121-rust\", where: [{field: \"tag\", eq: \"prd121-python\"}]) { totalCount } }"}')
COUNT=$(echo "$RESULT" | sed -n 's/.*"totalCount":\([0-9]*\).*/\1/p')
[ "$COUNT" = "0" ]
pass "serve: search in-query + where tag filters intersect (AND)"

RESULT=$(gql '{"query":"{ search(query: \"*\") { totalCount } }"}' || true)
echo "$RESULT" | grep -q "invalid search query"
! echo "$RESULT" | grep -q "internal error"
pass "serve: search bare asterisk returns bad request (not internal)"

RESULT=$(gql '{"query":"{ search(query: \"**\") { totalCount } }"}' || true)
echo "$RESULT" | grep -q "invalid search query"
! echo "$RESULT" | grep -q "internal error"
pass "serve: search double asterisk returns bad request (not internal)"

RESULT=$(gql '{"query":"{ search(query: \"(unbalanced\") { totalCount } }"}' || true)
echo "$RESULT" | grep -q "invalid search query"
! echo "$RESULT" | grep -q "internal error"
pass "serve: search unbalanced paren returns bad request (not internal)"

RESULT=$(gql '{"query":"{ search(query: \"AND\") { totalCount } }"}' || true)
echo "$RESULT" | grep -q "invalid search query"
! echo "$RESULT" | grep -q "internal error"
pass "serve: search bare AND returns bad request (not internal)"

RESULT=$(gql '{"query":"{ normalizeSearchQuery(query: \"tag=prd121-rust\") }"}')
echo "$RESULT" | grep -q '"tag=prd121-rust"'
pass "serve: normalizeSearchQuery preserves in-query tag filter"

NORMALIZED=$(gql '{"query":"{ normalizeSearchQuery(query: \"tag=prd121-rust AND category=work.dev\") }"}' | sed -n 's/.*"normalizeSearchQuery":"\([^"]*\)".*/\1/p')
[ "$NORMALIZED" = "category=work.dev and tag=prd121-rust" ]
RESULT=$(gql "{\"query\":\"{ search(query: \\\"$NORMALIZED\\\") { totalCount } }\"}")
! echo "$RESULT" | grep -q "invalid search query"
! echo "$RESULT" | grep -q "internal error"
pass "serve: search accepts normalized query round-trip"

gql "{\"query\":\"mutation { deleteDoogat(id: \\\"$PRD121A_ID\\\") }\"}" >/dev/null
gql "{\"query\":\"mutation { deleteDoogat(id: \\\"$PRD121B_ID\\\") }\"}" >/dev/null
gql "{\"query\":\"mutation { deleteDoogat(id: \\\"$PRD121G_ID\\\") }\"}" >/dev/null

# 18z. UPDATE/DELETE WHERE id no-match GraphQL parity (issue #5 group B).
# Rust unit tests and smoke CLI checks already cover B1-B5 at the lower layers;
# this sub-block pins the same behavior at the GraphQL executeSql surface.
ddl '{"query":"mutation { executeSql(sql: \"CREATE TABLE link_b1 (url VARCHAR(255))\") { message } }"}'
B1_SEED=$(gql '{"query":"mutation { executeSql(sql: \"INSERT INTO link_b1 (title, url) VALUES (\\\"A\\\", \\\"https://a.com\\\")\") { message } }"}')
B1_SEED_ID=$(printf '%s' "$B1_SEED" | extract_id)
[ -n "$B1_SEED_ID" ]
# B1: UPDATE WHERE id = nonexistent returns no errors and affected=0.
B1_RESULT=$(gql '{"query":"mutation { executeSql(sql: \"UPDATE link_b1 SET title = \\\"x\\\" WHERE id = \\\"does_not_exist_b1\\\"\") { affected message } }"}')
assert_gql_ok "$B1_RESULT"
printf '%s' "$B1_RESULT" | grep -q '"affected":0'
# B2: DELETE WHERE id = nonexistent returns no errors and affected=0.
B2_RESULT=$(gql '{"query":"mutation { executeSql(sql: \"DELETE FROM link_b1 WHERE id = \\\"does_not_exist_b2\\\"\") { affected message } }"}')
assert_gql_ok "$B2_RESULT"
printf '%s' "$B2_RESULT" | grep -q '"affected":0'
# B3: UPDATE WHERE non-id-column = nonexistent returns affected=0 (pin the
# working fallthrough path so a regression can't silently remove it).
B3_RESULT=$(gql '{"query":"mutation { executeSql(sql: \"UPDATE link_b1 SET title = \\\"x\\\" WHERE url = \\\"https://nope.com\\\"\") { affected message } }"}')
assert_gql_ok "$B3_RESULT"
printf '%s' "$B3_RESULT" | grep -q '"affected":0'
# B4: UPDATE WHERE id = valid AND other = wrong returns affected=0.
B4_RESULT=$(gql "{\"query\":\"mutation { executeSql(sql: \\\"UPDATE link_b1 SET title = 'x' WHERE id = '$B1_SEED_ID' AND url = 'https://wrong.com'\\\") { affected message } }\"}")
assert_gql_ok "$B4_RESULT"
printf '%s' "$B4_RESULT" | grep -q '"affected":0'
# B5: UPDATE WHERE id IN (nonexistent, valid) affects exactly the valid one.
B5_RESULT=$(gql "{\"query\":\"mutation { executeSql(sql: \\\"UPDATE link_b1 SET title = 'from_in_clause' WHERE id IN ('nope', '$B1_SEED_ID')\\\") { affected message } }\"}")
assert_gql_ok "$B5_RESULT"
printf '%s' "$B5_RESULT" | grep -q '"affected":1'
# Pin the working fast path: valid id -> affected=1.
B5_FAST=$(gql "{\"query\":\"mutation { executeSql(sql: \\\"UPDATE link_b1 SET title = 'final' WHERE id = '$B1_SEED_ID'\\\") { affected message } }\"}")
assert_gql_ok "$B5_FAST"
printf '%s' "$B5_FAST" | grep -q '"affected":1'
gql '{"query":"mutation { executeSql(sql: \"DROP TABLE link_b1 CASCADE\") { message } }"}' >/dev/null
pass "issue-5-B1..B5: UPDATE/DELETE no-match GraphQL parity"

# 18z2. executeBatch atomicity (issue #9 group F4). A batch where the second
# statement fails on a UNIQUE constraint must roll back the first statement's
# effect. Jink relies on this so partial writes can't leak out.
VER=$(gql '{"query":"{ schemaVersion }"}' | sed -n 's/.*"schemaVersion":\([0-9]*\).*/\1/p')
VER=${VER:-0}
gql '{"query":"mutation { executeSql(sql: \"CREATE TABLE link_f4 (url VARCHAR(255))\") { message } }"}' >/dev/null
gql '{"query":"mutation { executeSql(sql: \"CREATE TABLE membership_f4 (link_id VARCHAR(255), category VARCHAR(255), UNIQUE(link_id, category))\") { message } }"}' >/dev/null
wait_schema_reload "$VER"
F4_LINK=$(gql '{"query":"mutation { executeSql(sql: \"INSERT INTO link_f4 (title, url) VALUES (\\\"initial\\\", \\\"https://f4.com\\\")\") { message } }"}')
F4_LINK_ID=$(printf '%s' "$F4_LINK" | extract_id)
[ -n "$F4_LINK_ID" ]
gql "{\"query\":\"mutation { executeSql(sql: \\\"INSERT INTO membership_f4 (title, link_id, category) VALUES ('m', '$F4_LINK_ID', 'work')\\\") { message } }\"}" >/dev/null
# The batch: first UPDATE changes link title; second INSERT duplicates the
# membership (UNIQUE violation). Expected: the entire batch is rolled back, so
# link title stays "initial".
F4_BATCH=$(gql "{\"query\":\"mutation { executeBatch(statements: [\\\"UPDATE link_f4 SET title = 'batched' WHERE id = '$F4_LINK_ID' AND url = 'https://f4.com'\\\", \\\"INSERT INTO membership_f4 (title, link_id, category) VALUES ('dup', '$F4_LINK_ID', 'work')\\\"]) { message } }\"}")
assert_gql_errors "$F4_BATCH"
printf '%s' "$F4_BATCH" | grep -q 'UNIQUE'
# Verify the UPDATE was rolled back: title must still be "initial". The SQL
# response wraps row values in nested JSON-escaped strings ("rows":["[\"initial\"]"])
# so grep for the bare token without surrounding quotes.
F4_AFTER=$(gql "{\"query\":\"{ sql(query: \\\"SELECT title FROM link_f4 WHERE id = '$F4_LINK_ID'\\\") { rows } }\"}")
assert_gql_ok "$F4_AFTER"
printf '%s' "$F4_AFTER" | grep -q 'initial'
! printf '%s' "$F4_AFTER" | grep -q 'batched'
gql '{"query":"mutation { executeSql(sql: \"DROP TABLE link_f4 CASCADE\") { message } }"}' >/dev/null
gql '{"query":"mutation { executeSql(sql: \"DROP TABLE membership_f4 CASCADE\") { message } }"}' >/dev/null
pass "issue-9-F4: executeBatch rolls back all statements when one fails"

# 18z3. updateDoogat tag semantics (#9 F5/F6/F7). Pins three invariants that
# jink relies on: tags: [] clears, duplicate inputs dedupe, unicode round-trips.

# F5 — tags: [] clears all tags.
F5=$(gql '{"query":"mutation { createDoogat(input: { title: \"F5 tag clear\", tags: [\"a\", \"b\", \"c\"] }) { id tags } }"}')
F5_ID=$(printf '%s' "$F5" | sed -n 's/.*"id":"\([^"]*\)".*/\1/p')
[ -n "$F5_ID" ]
printf '%s' "$F5" | grep -q '"a"'
gql "{\"query\":\"mutation { updateDoogat(input: { id: \\\"$F5_ID\\\", tags: [] }) { id } }\"}" >/dev/null
F5_CHECK=$(gql "{\"query\":\"{ doogat(id: \\\"$F5_ID\\\") { id tags } }\"}")
printf '%s' "$F5_CHECK" | grep -q '"tags":\[\]'
gql "{\"query\":\"mutation { deleteDoogat(id: \\\"$F5_ID\\\") }\"}" >/dev/null
pass "issue-9-F5: updateDoogat tags: [] clears all tags"

# F6 — duplicate input tags are deduplicated.
F6=$(gql '{"query":"mutation { createDoogat(input: { title: \"F6 dedupe\" }) { id } }"}')
F6_ID=$(printf '%s' "$F6" | sed -n 's/.*"id":"\([^"]*\)".*/\1/p')
[ -n "$F6_ID" ]
gql "{\"query\":\"mutation { updateDoogat(input: { id: \\\"$F6_ID\\\", tags: [\\\"x\\\", \\\"y\\\", \\\"x\\\", \\\"y\\\", \\\"x\\\"] }) { id tags } }\"}" >/dev/null
F6_CHECK=$(gql "{\"query\":\"{ doogat(id: \\\"$F6_ID\\\") { id tags } }\"}")
# Count occurrences of "x" and "y" in the tags array. Should be exactly 1 each.
F6_X=$(printf '%s' "$F6_CHECK" | grep -o '"x"' | wc -l | tr -d ' ')
F6_Y=$(printf '%s' "$F6_CHECK" | grep -o '"y"' | wc -l | tr -d ' ')
[ "$F6_X" = "1" ]
[ "$F6_Y" = "1" ]
gql "{\"query\":\"mutation { deleteDoogat(id: \\\"$F6_ID\\\") }\"}" >/dev/null
pass "issue-9-F6: updateDoogat dedupes input tags"

# F7 — unicode tags round-trip intact.
F7=$(gql '{"query":"mutation { createDoogat(input: { title: \"F7 unicode\", tags: [\"日本語\", \"café\", \"ñoño\"] }) { id tags } }"}')
F7_ID=$(printf '%s' "$F7" | sed -n 's/.*"id":"\([^"]*\)".*/\1/p')
[ -n "$F7_ID" ]
F7_CHECK=$(gql "{\"query\":\"{ doogat(id: \\\"$F7_ID\\\") { id tags } }\"}")
# Some JSON encoders escape non-ASCII as \uXXXX. Check for either the raw
# codepoint or its \u escape so the test is resilient to either representation.
printf '%s' "$F7_CHECK" | grep -qE '日本語|\\u65e5\\u672c\\u8a9e'
printf '%s' "$F7_CHECK" | grep -qE 'café|caf\\u00e9'
printf '%s' "$F7_CHECK" | grep -qE 'ñoño|\\u00f1o\\u00f1o'
gql "{\"query\":\"mutation { deleteDoogat(id: \\\"$F7_ID\\\") }\"}" >/dev/null
pass "issue-9-F7: updateDoogat preserves unicode tags"

# 18z4. SQL feature coverage pins (#9 F9). Single per-feature check so a
# regression in any one of COUNT, GROUP BY, ORDER BY, LIMIT, OFFSET, IS NULL,
# LIKE is immediately attributable.
ddl '{"query":"mutation { executeSql(sql: \"CREATE TABLE feat (val INTEGER, label VARCHAR(255), maybe_null VARCHAR(255))\") { message } }"}'
gql '{"query":"mutation { executeSql(sql: \"INSERT INTO feat (title, val, label, maybe_null) VALUES (\\\"r1\\\", 1, \\\"a\\\", \\\"x\\\")\") { message } }"}' >/dev/null
gql '{"query":"mutation { executeSql(sql: \"INSERT INTO feat (title, val, label) VALUES (\\\"r2\\\", 2, \\\"a\\\")\") { message } }"}' >/dev/null
gql '{"query":"mutation { executeSql(sql: \"INSERT INTO feat (title, val, label, maybe_null) VALUES (\\\"r3\\\", 3, \\\"b\\\", \\\"y\\\")\") { message } }"}' >/dev/null
gql '{"query":"mutation { executeSql(sql: \"INSERT INTO feat (title, val, label) VALUES (\\\"r4\\\", 4, \\\"b\\\")\") { message } }"}' >/dev/null
gql '{"query":"mutation { executeSql(sql: \"INSERT INTO feat (title, val, label, maybe_null) VALUES (\\\"r5\\\", 5, \\\"c\\\", \\\"z\\\")\") { message } }"}' >/dev/null
# COUNT(*) — SQL responses wrap row values as nested JSON-escaped arrays, so
# grep -qF '[\"<val>\"]' is the matching pattern (see existing 43.D pattern).
F9_CNT=$(gql '{"query":"{ sql(query: \"SELECT COUNT(*) FROM feat\") { rows } }"}')
printf '%s' "$F9_CNT" | grep -qF '[\"5\"]'
# GROUP BY label → three groups (a, b, c). `rows` is an array of stringified
# arrays; format=array doesn't surface column names in the response, so only
# check the values.
F9_GRP=$(gql '{"query":"{ sql(query: \"SELECT label, COUNT(*) FROM feat GROUP BY label ORDER BY label\") { rows } }"}')
assert_gql_ok "$F9_GRP"
printf '%s' "$F9_GRP" | grep -qF '\"a\"'
printf '%s' "$F9_GRP" | grep -qF '\"b\"'
printf '%s' "$F9_GRP" | grep -qF '\"c\"'
# ORDER BY DESC LIMIT 2 → rows with val 5, 4 (labels c, b)
F9_ORD=$(gql '{"query":"{ sql(query: \"SELECT label FROM feat ORDER BY val DESC LIMIT 2\") { rows } }"}')
printf '%s' "$F9_ORD" | grep -qF '[\"c\"]'
printf '%s' "$F9_ORD" | grep -qF '[\"b\"]'
! printf '%s' "$F9_ORD" | grep -qF '[\"a\"]'
# OFFSET skipping first 3 → rows with val 4, 5
F9_OFF=$(gql '{"query":"{ sql(query: \"SELECT val FROM feat ORDER BY val ASC LIMIT 10 OFFSET 3\") { rows } }"}')
printf '%s' "$F9_OFF" | grep -qF '[\"4\"]'
printf '%s' "$F9_OFF" | grep -qF '[\"5\"]'
! printf '%s' "$F9_OFF" | grep -qF '[\"1\"]'
# IS NULL → two rows (val 2, val 4)
F9_NUL=$(gql '{"query":"{ sql(query: \"SELECT COUNT(*) FROM feat WHERE maybe_null IS NULL\") { rows } }"}')
printf '%s' "$F9_NUL" | grep -qF '[\"2\"]'
# LIKE → two rows (label a)
F9_LIK=$(gql '{"query":"{ sql(query: \"SELECT COUNT(*) FROM feat WHERE label LIKE \\\"a%\\\"\") { rows } }"}')
printf '%s' "$F9_LIK" | grep -qF '[\"2\"]'
gql '{"query":"mutation { executeSql(sql: \"DROP TABLE feat CASCADE\") { message } }"}' >/dev/null
pass "issue-9-F9: SQL feature coverage (COUNT, GROUP BY, ORDER BY, LIMIT, OFFSET, IS NULL, LIKE)"

# 18z5. search() limit boundary pins (#9 F10). Seed three distinguishing
# doogats, then probe 0 / 10000 / 10001 / -1 to capture current behavior.
# The server enforces a hard max of 10000 so 10001 is rejected with a clear
# message, not an internal error. This is a pin-existing-behavior check.
F10A=$(gql '{"query":"mutation { createDoogat(input: { title: \"F10boundary alpha\" }) { id } }"}')
F10A_ID=$(printf '%s' "$F10A" | sed -n 's/.*"id":"\([^"]*\)".*/\1/p')
F10B=$(gql '{"query":"mutation { createDoogat(input: { title: \"F10boundary beta\" }) { id } }"}')
F10B_ID=$(printf '%s' "$F10B" | sed -n 's/.*"id":"\([^"]*\)".*/\1/p')
F10C=$(gql '{"query":"mutation { createDoogat(input: { title: \"F10boundary gamma\" }) { id } }"}')
F10C_ID=$(printf '%s' "$F10C" | sed -n 's/.*"id":"\([^"]*\)".*/\1/p')
sleep 1
# limit: 0 — pin current behavior. Must not surface as an "internal error".
F10_ZERO=$(gql '{"query":"{ search(query: \"F10boundary\", limit: 0) { totalCount hits { id } } }"}' || true)
! printf '%s' "$F10_ZERO" | grep -q 'internal error'
# limit: 10000 — max allowed. Must succeed and return all seeded rows.
F10_MAX=$(gql '{"query":"{ search(query: \"F10boundary\", limit: 10000) { totalCount hits { id } } }"}')
assert_gql_ok "$F10_MAX"
printf '%s' "$F10_MAX" | grep -q '"totalCount":3'
# limit: 10001 — one above the max. Pin the clear-error rejection.
F10_OVER=$(gql '{"query":"{ search(query: \"F10boundary\", limit: 10001) { totalCount } }"}' || true)
printf '%s' "$F10_OVER" | grep -q 'limit must not exceed'
! printf '%s' "$F10_OVER" | grep -q 'internal error'
# limit: -1 — GraphQL Int type may accept or reject. Either way: never an
# internal error.
F10_NEG=$(gql '{"query":"{ search(query: \"F10boundary\", limit: -1) { totalCount } }"}' || true)
! printf '%s' "$F10_NEG" | grep -q 'internal error'
gql "{\"query\":\"mutation { deleteDoogat(id: \\\"$F10A_ID\\\") }\"}" >/dev/null
gql "{\"query\":\"mutation { deleteDoogat(id: \\\"$F10B_ID\\\") }\"}" >/dev/null
gql "{\"query\":\"mutation { deleteDoogat(id: \\\"$F10C_ID\\\") }\"}" >/dev/null
pass "issue-9-F10: search limit boundaries (0, 10000, 10001, -1)"

# 18z6. ALTER TABLE ADD COLUMN surfaces in the typeDefs introspection query
# (#9 F11). Jink relies on typeDefs to reflect the live schema so its client
# code can validate columns before writing.
ddl '{"query":"mutation { executeSql(sql: \"CREATE TABLE altschema_f11 (a VARCHAR(255))\") { message } }"}'
F11_BEFORE=$(gql '{"query":"{ typeDefs { name columns { name dataType } } }"}')
assert_gql_ok "$F11_BEFORE"
printf '%s' "$F11_BEFORE" | grep -q '"altschema_f11"'
# Column "a" should already be there; "b" should not exist yet.
printf '%s' "$F11_BEFORE" | python3 -c "
import json, sys
resp = json.loads(sys.stdin.read())
schemas = {t['name']: t['columns'] for t in resp['data']['typeDefs']}
assert 'altschema_f11' in schemas, 'altschema_f11 missing from typeDefs'
col_names = [c['name'] for c in schemas['altschema_f11']]
assert 'a' in col_names, f\"column a missing from altschema_f11, got: {col_names}\"
assert 'b' not in col_names, f\"column b unexpectedly present before ALTER, got: {col_names}\"
"
ddl '{"query":"mutation { executeSql(sql: \"ALTER TABLE altschema_f11 ADD COLUMN b INTEGER\") { message } }"}'
F11_AFTER=$(gql '{"query":"{ typeDefs { name columns { name dataType } } }"}')
assert_gql_ok "$F11_AFTER"
printf '%s' "$F11_AFTER" | python3 -c "
import json, sys
resp = json.loads(sys.stdin.read())
schemas = {t['name']: t['columns'] for t in resp['data']['typeDefs']}
assert 'altschema_f11' in schemas, 'altschema_f11 missing from typeDefs after ALTER'
col_names = [c['name'] for c in schemas['altschema_f11']]
assert 'a' in col_names, f\"column a missing after ALTER, got: {col_names}\"
assert 'b' in col_names, f\"column b missing from typeDefs after ALTER, got: {col_names}\"
"
gql '{"query":"mutation { executeSql(sql: \"DROP TABLE altschema_f11 CASCADE\") { message } }"}' >/dev/null
pass "issue-9-F11: ALTER TABLE ADD COLUMN appears in typeDefs introspection"

# 18z7. GraphQL schema introspection contract (#9 group G). Two invariants:
# G1 — every typed table has a plural query field AND an Aggregate field.
# G2 — every *Connection type exposes items and totalCount.
# Naming rules (per ddb-server/src/schema/base_types.rs):
#  - capitalize() upper-cases the first char → type name `Gqtesta`
#  - pluralize() appends `s` to the lowercased name → query field `gqtestas`
#  - Aggregate field is `<plural>Aggregate` → `gqtestasAggregate`
#  - Connection type is `<Type>Connection` → `GqtestaConnection`
VER=$(gql '{"query":"{ schemaVersion }"}' | sed -n 's/.*"schemaVersion":\([0-9]*\).*/\1/p')
VER=${VER:-0}
gql '{"query":"mutation { executeSql(sql: \"CREATE TABLE gqtesta (label VARCHAR(255))\") { message } }"}' >/dev/null
gql '{"query":"mutation { executeSql(sql: \"CREATE TABLE gqtestb (label VARCHAR(255))\") { message } }"}' >/dev/null
wait_schema_reload "$VER"
G_INTRO=$(gql '{"query":"{ __schema { queryType { fields { name } } types { name fields { name } } } }"}')
assert_gql_ok "$G_INTRO"
# G1: query fields `gqtestas` and `gqtestasAggregate` must exist (Gqtesta).
printf '%s' "$G_INTRO" | grep -q '"name":"gqtestas"'
printf '%s' "$G_INTRO" | grep -q '"name":"gqtestasAggregate"'
printf '%s' "$G_INTRO" | grep -q '"name":"gqtestbs"'
printf '%s' "$G_INTRO" | grep -q '"name":"gqtestbsAggregate"'
# G2: Connection types must exist for both, and each must carry items +
# totalCount. Use Python for structural parsing to avoid substring
# ambiguity with the raw introspection JSON.
printf '%s' "$G_INTRO" | python3 -c "
import json, sys
resp = json.loads(sys.stdin.read())
types = {t['name']: t for t in resp['data']['__schema']['types']}
for conn in ('GqtestaConnection', 'GqtestbConnection'):
    assert conn in types, f'{conn} missing from schema types'
    fields = {f['name'] for f in (types[conn].get('fields') or [])}
    assert 'items' in fields, f'{conn} missing items field, got: {fields}'
    assert 'totalCount' in fields, f'{conn} missing totalCount field, got: {fields}'
"
gql '{"query":"mutation { executeSql(sql: \"DROP TABLE gqtesta CASCADE\") { message } }"}' >/dev/null
gql '{"query":"mutation { executeSql(sql: \"DROP TABLE gqtestb CASCADE\") { message } }"}' >/dev/null
pass "issue-9-G1: every typed table has plural and Aggregate query fields"
pass "issue-9-G2: every Connection type has items and totalCount fields"

# 18z8. Category + membership jink port (#9 jink full-sweep section 5).
# Ported from validate-full-sweep.sh lines 102-132. Uses JINK_LINK_ID from
# sub-block 17.J2. Includes the COALESCE+subquery INSERT pattern jink relies
# on for sort_order computation.
J3_CAT=$(gql '{"query":"mutation { executeSql(sql: \"INSERT INTO category (title, fqn, space, sort_order) VALUES (\\\"Dev\\\", \\\"work.dev\\\", \\\"work\\\", 0)\") { message } }"}')
assert_gql_ok "$J3_CAT"
JINK_CAT_ID=$(printf '%s' "$J3_CAT" | extract_id)
[ -n "$JINK_CAT_ID" ]
pass "j3: INSERT category"

J3_CAT_SPACE=$(gql '{"query":"{ categories(where: {space: {eq: \"work\"}}) { items { id fqn title space sort_order } } }"}')
assert_gql_ok "$J3_CAT_SPACE"
printf '%s' "$J3_CAT_SPACE" | grep -q '"work.dev"'
pass "j3: categories GraphQL query by space"

J3_CAT_IN=$(gql '{"query":"{ categories(where: {fqn: {in: [\"work.dev\"]}}) { items { fqn title space } } }"}')
assert_gql_ok "$J3_CAT_IN"
printf '%s' "$J3_CAT_IN" | grep -q '"work.dev"'
pass "j3: categories GraphQL query by fqn IN list"

J3_CM=$(gql "{\"query\":\"mutation { executeSql(sql: \\\"INSERT INTO \\\\\\\"category-membership\\\\\\\" (title, link_id, category_fqn, sort_order) VALUES ('Test in work.dev', '$JINK_LINK_ID', 'work.dev', COALESCE((SELECT MAX(sort_order) + 1 FROM \\\\\\\"category-membership\\\\\\\" WHERE category_fqn = 'work.dev'), 0))\\\") { message } }\"}")
assert_gql_ok "$J3_CM"
printf '%s' "$J3_CM" | grep -qE '"message":"[0-9]+"'
pass "j3: INSERT category-membership with COALESCE+subquery sort_order"

J3_CM_BOTH=$(gql "{\"query\":\"{ categoryMemberships(where: {link_id: {eq: \\\"$JINK_LINK_ID\\\"}, category_fqn: {eq: \\\"work.dev\\\"}}) { items { id link_id category_fqn pinned sort_order } } }\"}")
assert_gql_ok "$J3_CM_BOTH"
printf '%s' "$J3_CM_BOTH" | grep -q '"work.dev"'
pass "j3: categoryMemberships by link_id + category_fqn"

J3_CM_LINK=$(gql "{\"query\":\"{ categoryMemberships(where: {link_id: {eq: \\\"$JINK_LINK_ID\\\"}}) { items { category_fqn } } }\"}")
assert_gql_ok "$J3_CM_LINK"
printf '%s' "$J3_CM_LINK" | grep -q '"work.dev"'
pass "j3: categoryMemberships by link_id only"

J3_CM_CAT=$(gql '{"query":"{ categoryMemberships(where: {category_fqn: {eq: \"work.dev\"}}) { items { link_id pinned sort_order } } }"}')
assert_gql_ok "$J3_CM_CAT"
printf '%s' "$J3_CM_CAT" | grep -q "$JINK_LINK_ID"
pass "j3: categoryMemberships by category_fqn only"

# 18z9. Jink quotes + saved-searches + pinned-results + jinkConfigs port
# (#9 jink full-sweep sections 10-12). Ported from validate-full-sweep.sh
# lines 188-241.

# Quotes (4 checks).
J4_Q=$(gql '{"query":"mutation { executeSql(sql: \"INSERT INTO quote (title, author, text) VALUES (\\\"First\\\", \\\"Anon\\\", \\\"Hello world\\\")\") { message } }"}')
assert_gql_ok "$J4_Q"
JINK_QUOTE_ID=$(printf '%s' "$J4_Q" | extract_id)
[ -n "$JINK_QUOTE_ID" ]
pass "j4: INSERT quote"

J4_Q_ID=$(gql "{\"query\":\"{ quotes(where: {id: {eq: \\\"$JINK_QUOTE_ID\\\"}}) { items { id title text author } } }\"}")
assert_gql_ok "$J4_Q_ID"
printf '%s' "$J4_Q_ID" | grep -q 'Hello world'
pass "j4: quotes query by id"

J4_Q_ALL=$(gql '{"query":"{ quotes { items { id } } }"}')
assert_gql_ok "$J4_Q_ALL"
printf '%s' "$J4_Q_ALL" | grep -q '"quotes"'
pass "j4: quotes query all"

J4_Q_UPD=$(gql "{\"query\":\"mutation { executeSql(sql: \\\"UPDATE quote SET favorited = 'true' WHERE id = '$JINK_QUOTE_ID' AND title = 'First'\\\") { affected } }\"}")
assert_gql_ok "$J4_Q_UPD"
printf '%s' "$J4_Q_UPD" | grep -q '"affected":1'
pass "j4: UPDATE quote SET favorited (compound predicate)"

# Saved-search (2 checks).
J4_SS=$(gql '{"query":"mutation { executeSql(sql: \"INSERT INTO \\\"saved-search\\\" (title, query_raw, query_normalized) VALUES (\\\"rust stuff\\\", \\\"Rust\\\", \\\"rust\\\")\") { message } }"}')
assert_gql_ok "$J4_SS"
JINK_SS_ID=$(printf '%s' "$J4_SS" | extract_id)
[ -n "$JINK_SS_ID" ]
pass "j4: INSERT saved-search"

J4_SS_Q=$(gql "{\"query\":\"{ savedSearches(where: {id: {eq: \\\"$JINK_SS_ID\\\"}}) { items { id title query_raw query_normalized } } }\"}")
assert_gql_ok "$J4_SS_Q"
printf '%s' "$J4_SS_Q" | grep -q '"rust"'
pass "j4: savedSearches query by id"

# Pinned-result (2 checks).
J4_PR=$(gql "{\"query\":\"mutation { executeSql(sql: \\\"INSERT INTO \\\\\\\"pinned-result\\\\\\\" (title, query_normalized, link_id, sort_order) VALUES ('pinned test', 'rust', '$JINK_LINK_ID', 0)\\\") { message } }\"}")
assert_gql_ok "$J4_PR"
printf '%s' "$J4_PR" | grep -qE '"message":"[0-9]+"'
pass "j4: INSERT pinned-result"

J4_PR_Q=$(gql '{"query":"{ pinnedResults(where: {query_normalized: {eq: \"rust\"}}) { items { id query_normalized link_id sort_order } } }"}')
assert_gql_ok "$J4_PR_Q"
printf '%s' "$J4_PR_Q" | grep -q "$JINK_LINK_ID"
pass "j4: pinnedResults query by query_normalized"

# jinkConfigs (3 checks).
J4_JC=$(gql '{"query":"{ jinkConfigs { items { id dashboard_title quote_rotation_minutes links_per_category frontend_version } } }"}')
assert_gql_ok "$J4_JC"
printf '%s' "$J4_JC" | grep -q 'dashboard_title'
pass "j4: jinkConfigs query"

J4_JC_UPD=$(gql '{"query":"mutation { executeSql(sql: \"UPDATE \\\"jink-config\\\" SET frontend_version = '\''1.0.0'\'' WHERE title = '\''jink-config'\''\") { affected } }"}')
assert_gql_ok "$J4_JC_UPD"
printf '%s' "$J4_JC_UPD" | grep -q '"affected":1'
pass "j4: UPDATE jink-config frontend_version (compound predicate)"

J4_JC_SEL=$(gql '{"query":"{ sql(query: \"SELECT frontend_version FROM \\\"jink-config\\\" LIMIT 1\") { rows } }"}')
assert_gql_ok "$J4_JC_SEL"
printf '%s' "$J4_JC_SEL" | grep -qF '\"1.0.0\"'
pass "j4: SELECT frontend_version returns 1.0.0"

# 18z10. Composite UNIQUE duplicate + compound-predicate DELETE jink port
# (#9 jink full-sweep sections 6 + 13). Ported from validate-full-sweep.sh
# lines 137-139 and 246-256. Also drops all jink tables at the end so no
# jink-port state leaks into later sections.

# Sweep section 6: composite UNIQUE duplicate must be rejected with a
# descriptive error. (Complements the F1 CLI check in section 30.)
J5_DUP=$(gql "{\"query\":\"mutation { executeSql(sql: \\\"INSERT INTO \\\\\\\"category-membership\\\\\\\" (title, link_id, category_fqn) VALUES ('dup', '$JINK_LINK_ID', 'work.dev')\\\") { message } }\"}")
assert_gql_errors "$J5_DUP"
printf '%s' "$J5_DUP" | grep -q 'UNIQUE'
pass "j5: duplicate category-membership rejected with UNIQUE error"

# Sweep section 13: executeBatch DELETE with compound predicates.
J5_BATCH=$(gql "{\"query\":\"mutation { executeBatch(statements: [\\\"DELETE FROM \\\\\\\"category-membership\\\\\\\" WHERE link_id = '$JINK_LINK_ID' AND category_fqn = 'work.dev'\\\", \\\"DELETE FROM link WHERE id = '$JINK_LINK_ID' AND url = 'https://example.com'\\\"]) { message } }\"}")
assert_gql_ok "$J5_BATCH"
printf '%s' "$J5_BATCH" | grep -q '"executeBatch"'
pass "j5: executeBatch DELETE category-membership + link (compound predicates)"

J5_LINK_GONE=$(gql "{\"query\":\"{ links(where: {id: {eq: \\\"$JINK_LINK_ID\\\"}}) { items { id } } }\"}")
assert_gql_ok "$J5_LINK_GONE"
printf '%s' "$J5_LINK_GONE" | grep -qE '"items":\[\]'
pass "j5: link is gone after batch delete"

J5_Q_DEL=$(gql "{\"query\":\"mutation { executeSql(sql: \\\"DELETE FROM quote WHERE id = '$JINK_QUOTE_ID' AND title = 'First'\\\") { affected } }\"}")
assert_gql_ok "$J5_Q_DEL"
printf '%s' "$J5_Q_DEL" | grep -q '"affected":1'
pass "j5: DELETE quote (compound predicate)"

# Final cleanup: drop all jink tables from sub-block J1. Silenced so DROP
# output doesn't pollute subsequent pass lines.
gql '{"query":"mutation { executeSql(sql: \"DROP TABLE link CASCADE\") { message } }"}' >/dev/null
gql '{"query":"mutation { executeSql(sql: \"DROP TABLE category CASCADE\") { message } }"}' >/dev/null
gql '{"query":"mutation { executeSql(sql: \"DROP TABLE \\\"category-membership\\\" CASCADE\") { message } }"}' >/dev/null
gql '{"query":"mutation { executeSql(sql: \"DROP TABLE quote CASCADE\") { message } }"}' >/dev/null
gql '{"query":"mutation { executeSql(sql: \"DROP TABLE \\\"saved-search\\\" CASCADE\") { message } }"}' >/dev/null
gql '{"query":"mutation { executeSql(sql: \"DROP TABLE \\\"pinned-result\\\" CASCADE\") { message } }"}' >/dev/null
gql '{"query":"mutation { executeSql(sql: \"DROP TABLE \\\"jink-config\\\" CASCADE\") { message } }"}' >/dev/null
pass "j5: jink port cleanup (all tables dropped)"

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
  ddl '{"query":"mutation{executeSql(sql:\"CREATE TABLE pgbooltest (label TEXT, active BOOLEAN)\"){message}}"}'
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

# error response — SQL engine errors are descriptive (user-actionable)
RESULT=$(gql '{"query":"mutation { executeSql(sql: \"SELCT * FORM oops\") { message } }"}' || true)
if [ -z "$RESULT" ]; then echo "FAIL: gql returned empty response" >&2; exit 1; fi
echo "$RESULT" | grep -q '"errors"'
echo "$RESULT" | grep -qi "parse:"
pass "serve: sql engine error is descriptive"

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
VER=$(gql '{"query":"{ schemaVersion }"}' | sed -n 's/.*"schemaVersion":\([0-9]*\).*/\1/p')
VER=${VER:-0}
gql '{"query":"mutation{executeSql(sql:\"CREATE TABLE mvcategory (name VARCHAR(100))\"){message}}"}'
gql '{"query":"mutation{executeSql(sql:\"CREATE TABLE mvbookmark (mvcategory TEXT REFERENCES mvcategory)\"){message}}"}'
wait_schema_reload "$VER"
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
VER=$(gql '{"query":"{ schemaVersion }"}' | sed -n 's/.*"schemaVersion":\([0-9]*\).*/\1/p')
VER=${VER:-0}
gql '{"query":"mutation { executeSql(sql: \"CREATE TABLE smokecat (label TEXT)\") { message } }"}' >/dev/null
gql '{"query":"mutation { executeSql(sql: \"CREATE TABLE smokebm (url TEXT, smokecat TEXT REFERENCES smokecat)\") { message } }"}' >/dev/null
wait_schema_reload "$VER"
SCAT=$(gql "{\"query\":\"mutation { executeSql(sql: \\\"INSERT INTO smokecat (title, label) VALUES ('Tech', 'tech')\\\") { message } }\"}")
SCAT_ID=$(echo "$SCAT" | sed -n 's/.*"message":"\([^"]*\)".*/\1/p')
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
VER=$(gql '{"query":"{ schemaVersion }"}' | sed -n 's/.*"schemaVersion":\([0-9]*\).*/\1/p')
VER=${VER:-0}
gql '{"query":"mutation { executeSql(sql: \"CREATE TABLE rlcat (label TEXT)\") { message } }"}' >/dev/null
gql '{"query":"mutation { executeSql(sql: \"CREATE TABLE rlbm (url TEXT, rlcat TEXT REFERENCES rlcat)\") { message } }"}' >/dev/null
wait_schema_reload "$VER"
RL_C1=$(gql '{"query":"mutation { executeSql(sql: \"INSERT INTO rlcat (label) VALUES (\\\"cherry\\\")\") { message } }"}' | sed -n 's/.*"message":"\([^"]*\)".*/\1/p')
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
echo "$RESULT" | grep -qF '\"id\":'
echo "$RESULT" | grep -qF '\"title\":'
pass "serve: sql format objects returns keyed rows"

ddl '{"query":"mutation{executeSql(sql:\"CREATE TABLE smokepin (pinned BOOLEAN)\"){message}}"}'
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
VER=$(gql '{"query":"{ schemaVersion }"}' | sed -n 's/.*"schemaVersion":\([0-9]*\).*/\1/p')
VER=${VER:-0}
RESULT=$(gql '{"query":"mutation { executeBatch(statements: [\"CREATE TABLE batchtest (col1 TEXT)\"]) { message } }"}')
echo "$RESULT" | grep -q '"message"'
wait_schema_reload "$VER"
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

# 38i. typed field updates via GraphQL (updateDoogat fields/unsetFields, deleteDoogat cleanup)
VER=$(gql '{"query":"{ schemaVersion }"}' | sed -n 's/.*"schemaVersion":\([0-9]*\).*/\1/p')
VER=${VER:-0}
gql '{"query":"mutation { executeSql(sql: \"CREATE TABLE tfubookmark (url VARCHAR(200))\") { message } }"}' | grep -q "table tfubookmark created"
wait_schema_reload "$VER"
TFU_ID=$(gql '{"query":"mutation { executeSql(sql: \"INSERT INTO tfubookmark (title, url) VALUES (\\\"TFU Test\\\", \\\"https://old.com\\\")\") { message } }"}' | sed -n 's/.*"message":"\([^"]*\)".*/\1/p')
echo "$TFU_ID" | grep -qE "^[0-9]{14}$"
# Verify initial materialized row
gql "{\"query\":\"mutation { executeSql(sql: \\\"SELECT url FROM tfubookmark WHERE id = '$TFU_ID'\\\", format: \\\"objects\\\") { rows } }\"}" | grep -q "https://old.com"
# updateDoogat with fields to change url
TFU_UPD=$(gql "{\"query\":\"mutation { updateDoogat(input: { id: \\\"$TFU_ID\\\", fields: \\\"{\\\\\\\"url\\\\\\\":\\\\\\\"https://updated.com\\\\\\\"}\\\" }) { id } }\"}")
echo "$TFU_UPD" | grep -q "$TFU_ID"
# Verify via SQL SELECT that materialized row has updated url
gql "{\"query\":\"mutation { executeSql(sql: \\\"SELECT url FROM tfubookmark WHERE id = '$TFU_ID'\\\", format: \\\"objects\\\") { rows } }\"}" | grep -q "https://updated.com"
pass "serve: typed field update via GraphQL updateDoogat"
# updateDoogat with unsetFields to remove url
TFU_UNSET=$(gql "{\"query\":\"mutation { updateDoogat(input: { id: \\\"$TFU_ID\\\", unsetFields: [\\\"url\\\"] }) { id } }\"}")
echo "$TFU_UNSET" | grep -q "$TFU_ID"
# Verify url is gone (NULL)
TFU_AFTER=$(gql "{\"query\":\"mutation { executeSql(sql: \\\"SELECT url FROM tfubookmark WHERE id = '$TFU_ID'\\\", format: \\\"objects\\\") { rows } }\"}")
if echo "$TFU_AFTER" | grep -q "https://"; then
  echo "FAIL: url should be unset after unsetFields" >&2; exit 1
fi
pass "serve: typed field unset via GraphQL updateDoogat"
# Delete the doogat and verify materialized row is gone
gql "{\"query\":\"mutation { deleteDoogat(id: \\\"$TFU_ID\\\") }\"}" >/dev/null
TFU_COUNT=$(gql "{\"query\":\"mutation { executeSql(sql: \\\"SELECT COUNT(*) FROM tfubookmark WHERE id = '$TFU_ID'\\\") { rows } }\"}")
echo "$TFU_COUNT" | grep -qF '[\"0\"]'
pass "serve: deleteDoogat cleans materialized type table row"
# Clean up typedef
gql '{"query":"mutation { executeSql(sql: \"DROP TABLE tfubookmark CASCADE\") { message } }"}' >/dev/null

# Hyphenated type names in GraphQL
ddl '{"query":"mutation { executeSql(sql: \"CREATE TABLE \\\"test-widget\\\" (status TEXT, priority INTEGER)\") { message } }"}'
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

# 43.D SQL constraint enforcement on executeSql write path (PRD 00122 / issue #7)
# Six checks (D1-D6) extending the existing INSERT-validation neighborhood.

# Setup: a NOT NULL link table for D1-D5 and a numeric table for D3.
VER=$(gql '{"query":"{ schemaVersion }"}' | sed -n 's/.*"schemaVersion":\([0-9]*\).*/\1/p')
VER=${VER:-0}
gql '{"query":"mutation{executeSql(sql:\"CREATE TABLE link_d1 (title VARCHAR(255) NOT NULL, url VARCHAR(255) NOT NULL)\"){message}}"}' >/dev/null
gql '{"query":"mutation{executeSql(sql:\"CREATE TABLE numeric_d3 (title VARCHAR(255) NOT NULL, count INTEGER)\"){message}}"}' >/dev/null
wait_schema_reload "$VER"

# D1. NOT NULL: INSERT with NULL title is rejected and no row is created.
D1_RESULT=$(gql '{"query":"mutation{executeSql(sql:\"INSERT INTO link_d1 (title, url) VALUES (NULL, \\\"https://n.com\\\")\"){message}}"}')
echo "$D1_RESULT" | grep -q "NOT NULL constraint violated: link_d1.title"
D1_COUNT=$(gql '{"query":"mutation{executeSql(sql:\"SELECT COUNT(*) FROM link_d1\"){rows}}"}')
echo "$D1_COUNT" | grep -qF '[\"0\"]'
pass "serve: D1 INSERT NULL on NOT NULL is rejected, no row created"

# D2. VARCHAR(N) overflow: 300-char title against VARCHAR(255) is rejected.
LONG=$(printf 'x%.0s' {1..300})
D2_RESULT=$(gql "{\"query\":\"mutation{executeSql(sql:\\\"INSERT INTO link_d1 (title, url) VALUES (\\\\\\\"$LONG\\\\\\\", \\\\\\\"https://v.com\\\\\\\")\\\"){message}}\"}")
echo "$D2_RESULT" | grep -q "value too long for link_d1.title"
D2_COUNT=$(gql '{"query":"mutation{executeSql(sql:\"SELECT COUNT(*) FROM link_d1\"){rows}}"}')
echo "$D2_COUNT" | grep -qF '[\"0\"]'
pass "serve: D2 VARCHAR(N) overflow is rejected, no row created"

# D3. INTEGER type mismatch: non-numeric value into INTEGER column is rejected.
D3_RESULT=$(gql '{"query":"mutation{executeSql(sql:\"INSERT INTO numeric_d3 (title, count) VALUES (\\\"a\\\", \\\"not_a_number\\\")\"){message}}"}')
echo "$D3_RESULT" | grep -q "type mismatch for numeric_d3.count: expected INTEGER"
D3_COUNT=$(gql '{"query":"mutation{executeSql(sql:\"SELECT COUNT(*) FROM numeric_d3\"){rows}}"}')
echo "$D3_COUNT" | grep -qF '[\"0\"]'
pass "serve: D3 INTEGER type mismatch is rejected, no row created"

# D4. Unknown column on INSERT: column not in schema is rejected.
D4_RESULT=$(gql '{"query":"mutation{executeSql(sql:\"INSERT INTO link_d1 (title, url, unknown_col) VALUES (\\\"t\\\", \\\"https://u.com\\\", \\\"dropped\\\")\"){message}}"}')
echo "$D4_RESULT" | grep -q "unknown column: link_d1.unknown_col"
D4_COUNT=$(gql '{"query":"mutation{executeSql(sql:\"SELECT COUNT(*) FROM link_d1\"){rows}}"}')
echo "$D4_COUNT" | grep -qF '[\"0\"]'
pass "serve: D4 unknown column on INSERT is rejected, no row created"

# D5. Unknown column on UPDATE: insert one valid row, then UPDATE with bogus
# column. The original row's title must be unchanged after the rejection.
D5_VALID=$(gql '{"query":"mutation{executeSql(sql:\"INSERT INTO link_d1 (title, url) VALUES (\\\"keep\\\", \\\"https://k.com\\\")\"){message}}"}')
D5_ID=$(echo "$D5_VALID" | sed -n 's/.*"message":"\([0-9]*\)".*/\1/p')
D5_RESULT=$(gql "{\"query\":\"mutation{executeSql(sql:\\\"UPDATE link_d1 SET unknown_col = 'x' WHERE id = '$D5_ID'\\\"){message}}\"}")
echo "$D5_RESULT" | grep -q "unknown column: link_d1.unknown_col"
D5_TITLE=$(gql "{\"query\":\"mutation{executeSql(sql:\\\"SELECT title FROM link_d1 WHERE id = '$D5_ID'\\\"){rows}}\"}")
echo "$D5_TITLE" | grep -q 'keep'
pass "serve: D5 unknown column on UPDATE is rejected, row unchanged"

# D6. Silent title fallback removed: title NOT NULL with no template, INSERT
# omitting title now fails instead of coercing url/description into title.
ddl '{"query":"mutation{executeSql(sql:\"CREATE TABLE link_d6 (title VARCHAR(255) NOT NULL, url VARCHAR(255), description TEXT)\"){message}}"}'
D6_RESULT=$(gql '{"query":"mutation{executeSql(sql:\"INSERT INTO link_d6 (url) VALUES (\\\"https://notitle.com\\\")\"){message}}"}')
echo "$D6_RESULT" | grep -q "NOT NULL constraint violated: link_d6.title"
D6_COUNT=$(gql '{"query":"mutation{executeSql(sql:\"SELECT COUNT(*) FROM link_d6\"){rows}}"}')
echo "$D6_COUNT" | grep -qF '[\"0\"]'
pass "serve: D6 silent title fallback removed, missing title rejected"

# Cleanup D-tables
gql '{"query":"mutation{executeSql(sql:\"DROP TABLE link_d1 CASCADE\"){message}}"}' >/dev/null
gql '{"query":"mutation{executeSql(sql:\"DROP TABLE numeric_d3 CASCADE\"){message}}"}' >/dev/null
gql '{"query":"mutation{executeSql(sql:\"DROP TABLE link_d6 CASCADE\"){message}}"}' >/dev/null

# 44.E1 — Pin JOIN as working (issue #8 group E1). PRD 00123 was archived
# because JOIN actually works; this check pins the behavior at the GraphQL
# surface so a regression can't silently drop joined rows.
VER=$(gql '{"query":"{ schemaVersion }"}' | sed -n 's/.*"schemaVersion":\([0-9]*\).*/\1/p')
VER=${VER:-0}
gql '{"query":"mutation { executeSql(sql: \"CREATE TABLE e1_link (url VARCHAR(255))\") { message } }"}' >/dev/null
gql '{"query":"mutation { executeSql(sql: \"CREATE TABLE e1_num (count INTEGER)\") { message } }"}' >/dev/null
wait_schema_reload "$VER"
gql '{"query":"mutation { executeSql(sql: \"INSERT INTO e1_link (title, url) VALUES (\\\"a\\\", \\\"https://a.com\\\")\") { message } }"}' >/dev/null
gql '{"query":"mutation { executeSql(sql: \"INSERT INTO e1_num (title, count) VALUES (\\\"a\\\", 1)\") { message } }"}' >/dev/null
E1_JOIN=$(gql '{"query":"{ sql(query: \"SELECT l.title, n.count FROM e1_link l JOIN e1_num n ON l.title = n.title\") { rows } }"}')
assert_gql_ok "$E1_JOIN"
# SQL row responses are nested JSON-escaped (["[\"a\",\"1\"]"]); grep -F for
# the escaped form.
printf '%s' "$E1_JOIN" | grep -qF '\"a\"'
printf '%s' "$E1_JOIN" | grep -qF '\"1\"'
gql '{"query":"mutation { executeSql(sql: \"DROP TABLE e1_link CASCADE\") { message } }"}' >/dev/null
gql '{"query":"mutation { executeSql(sql: \"DROP TABLE e1_num CASCADE\") { message } }"}' >/dev/null
pass "issue-8-E1: SELECT ... JOIN returns joined rows (PRD 00123 archived as obsolete)"

# 44. DDL response consistency (no spurious errors)
VER=$(gql '{"query":"{ schemaVersion }"}' | sed -n 's/.*"schemaVersion":\([0-9]*\).*/\1/p')
VER=${VER:-0}
RESULT=$(gql '{"query":"mutation { executeSql(sql: \"CREATE TABLE ddltest (name VARCHAR(100))\") { columns rows message } }"}')
echo "$RESULT" | grep -qv '"errors"'
echo "$RESULT" | grep -q '"columns":\[\]'
echo "$RESULT" | grep -q '"rows":\[\]'
echo "$RESULT" | grep -q '"message"'
pass "serve: CREATE TABLE response has no errors"

wait_schema_reload "$VER"
VER=$(gql '{"query":"{ schemaVersion }"}' | sed -n 's/.*"schemaVersion":\([0-9]*\).*/\1/p')
VER=${VER:-0}
RESULT=$(gql '{"query":"mutation { executeSql(sql: \"ALTER TABLE ddltest ADD COLUMN age INTEGER\") { columns rows message } }"}')
echo "$RESULT" | grep -qv '"errors"'
echo "$RESULT" | grep -q '"columns":\[\]'
echo "$RESULT" | grep -q '"rows":\[\]'
pass "serve: ALTER TABLE response has no errors"

wait_schema_reload "$VER"
RESULT=$(gql '{"query":"mutation { executeSql(sql: \"DROP TABLE ddltest\") { columns rows message } }"}')
echo "$RESULT" | grep -qv '"errors"'
echo "$RESULT" | grep -q '"columns":\[\]'
echo "$RESULT" | grep -q '"rows":\[\]'
pass "serve: DROP TABLE response has no errors"

VER=$(gql '{"query":"{ schemaVersion }"}' | sed -n 's/.*"schemaVersion":\([0-9]*\).*/\1/p')
VER=${VER:-0}
RESULT=$(gql '{"query":"mutation { executeBatch(statements: [\"CREATE TABLE ddlbatch1 (name VARCHAR)\", \"CREATE TABLE ddlbatch2 (val INTEGER)\"]) { columns rows message } }"}')
echo "$RESULT" | grep -qv '"errors"'
echo "$RESULT" | grep -q '"columns":\[\]'
echo "$RESULT" | grep -q '"rows":\[\]'
pass "serve: executeBatch DDL responses have no errors"

wait_schema_reload "$VER"
gql '{"query":"mutation { executeSql(sql: \"DROP TABLE ddlbatch1 CASCADE\") { message } }"}' >/dev/null
gql '{"query":"mutation { executeSql(sql: \"DROP TABLE ddlbatch2 CASCADE\") { message } }"}' >/dev/null

# DML regression: INSERT still returns affected count
DML_RESULT=$(gql '{"query":"mutation { executeSql(sql: \"INSERT INTO datecheck (name) VALUES (\\\"DmlRegression\\\")\") { affected message } }"}')
echo "$DML_RESULT" | grep -qv '"errors"'
echo "$DML_RESULT" | grep -q '"message"'
pass "serve: DML INSERT response unchanged"

# 45. createMany onConflict: IGNORE (upsert via GraphQL)
ddl '{"query":"mutation { executeSql(sql: \"CREATE TABLE upsertgql (code TEXT, label TEXT)\") { message } }"}'
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

# === PRD 00130: GraphQL typed-write polish (issues #11/#12/#13) ===

# 45.G13 — issue #13: createDoogat must omit `title` when the typedef has a
# title_template, rendering it server-side. Pre-PRD-00130 the schema marked
# title NON_NULL so the template never fired through GraphQL.
ddl '{"query":"mutation { executeSql(sql: \"CREATE TABLE g13link (title TEXT, url VARCHAR(255))\") { message } }"}'
ddl "{\"query\":\"mutation { executeSql(sql: \\\"ALTER TABLE g13link SET TITLE TEMPLATE 'link-{url}'\\\") { message } }\"}"
G13_OMIT=$(gql '{"query":"mutation { createDoogat(input: {type: \"g13link\", fields: \"{\\\"url\\\":\\\"https://example.com\\\"}\"}) { id title } }"}')
assert_gql_ok "$G13_OMIT"
G13_TITLE=$(echo "$G13_OMIT" | jq -r '.data.createDoogat.title')
[ "$G13_TITLE" = "link-https://example.com" ]
pass "issue-13: createDoogat omits title when typedef has title_template"

# Negative: typedef without a template + omitted title → NOT_NULL_VIOLATION
ddl '{"query":"mutation { executeSql(sql: \"CREATE TABLE g13plain (title TEXT, url VARCHAR(255))\") { message } }"}'
G13_NULL=$(gql '{"query":"mutation { createDoogat(input: {type: \"g13plain\", fields: \"{\\\"url\\\":\\\"https://x\\\"}\"}) { id } }"}')
assert_gql_errors "$G13_NULL"
printf '%s' "$G13_NULL" | grep -q "NOT NULL constraint violated: g13plain.title"
pass "issue-13: createDoogat without title or template rejects with NOT_NULL_VIOLATION"

gql '{"query":"mutation { executeSql(sql: \"DROP TABLE g13link CASCADE\") { message } }"}' >/dev/null
gql '{"query":"mutation { executeSql(sql: \"DROP TABLE g13plain CASCADE\") { message } }"}' >/dev/null

# 45.G12 — issue #12: createMany(onConflict: IGNORE) returns the surviving
# row's ID for skipped rows when both duplicates appear in the same batch.
# The pre-PRD-00130 path returned the rejected (rolled-back) ID, which
# does not exist anywhere — callers using the returned ID for follow-up
# reads silently missed.
ddl '{"query":"mutation { executeSql(sql: \"CREATE TABLE g12item (code TEXT, label TEXT)\") { message } }"}'
G12_TYPEDEF=$(find ddb/_typedef -name '*.md' -exec grep -l 'title: g12item' {} \;)
sed -i.bak 's/type: _typedef/type: _typedef\nunique_together:\n  - - code/' "$G12_TYPEDEF"
rm -f "${G12_TYPEDEF}.bak"
git add -A && git commit -m "add unique_together to g12item" --quiet
$DDB reindex >/dev/null
G12_BATCH=$(gql '{"query":"mutation { createMany(inputs: [{title: \"A\", type: \"g12item\", fields: \"{\\\"code\\\":\\\"K1\\\",\\\"label\\\":\\\"first\\\"}\"}, {title: \"A Dup\", type: \"g12item\", fields: \"{\\\"code\\\":\\\"K1\\\",\\\"label\\\":\\\"second\\\"}\"}], onConflict: IGNORE) { id title } }"}')
assert_gql_ok "$G12_BATCH"
G12_ID0=$(echo "$G12_BATCH" | jq -r '.data.createMany[0].id')
G12_ID1=$(echo "$G12_BATCH" | jq -r '.data.createMany[1].id')
[ -n "$G12_ID0" ]
[ "$G12_ID0" = "$G12_ID1" ]
G12_TITLE1=$(echo "$G12_BATCH" | jq -r '.data.createMany[1].title')
[ "$G12_TITLE1" = "A" ]
# Exactly one row in the type table; the rejected ID is nowhere.
G12_COUNT=$(gql '{"query":"mutation { executeSql(sql: \"SELECT COUNT(*) FROM g12item\") { rows } }"}' | jq -r '.data.executeSql.rows[0]' | jq -r '.[0]')
[ "$G12_COUNT" = "1" ]
pass "issue-12: createMany IGNORE returns surviving ID for intra-batch duplicate"

gql '{"query":"mutation { executeSql(sql: \"DROP TABLE g12item CASCADE\") { message } }"}' >/dev/null

# 45.G11 — issue #11: TagsFilter operators are nullable; a contains-only
# filter parses (was rejected pre-PRD-00130 because containsAll/containsAny
# were schema-required); empty filter and empty arrays are rejected at
# resolve time.
ddl '{"query":"mutation { executeSql(sql: \"CREATE TABLE g11link (title TEXT, url VARCHAR(255))\") { message } }"}'
G11_CREATE=$(gql '{"query":"mutation { createDoogat(input: {title: \"Tagged\", type: \"g11link\", tags: [\"rust\", \"sql\"], fields: \"{\\\"url\\\":\\\"https://example.com\\\"}\"}) { id } }"}')
assert_gql_ok "$G11_CREATE"
# contains-only filter must succeed without supplying containsAll/containsAny
G11_CONTAINS=$(gql '{"query":"{ g11links(where: {tags: {contains: \"rust\"}}) { totalCount items { id } } }"}')
assert_gql_ok "$G11_CONTAINS"
G11_TC=$(echo "$G11_CONTAINS" | jq -r '.data.g11links.totalCount')
[ "$G11_TC" = "1" ]
pass "issue-11: TagsFilter contains-only filter parses and matches"

# containsAll-only must succeed
G11_ALL=$(gql '{"query":"{ g11links(where: {tags: {containsAll: [\"rust\", \"sql\"]}}) { totalCount } }"}')
assert_gql_ok "$G11_ALL"
[ "$(echo "$G11_ALL" | jq -r '.data.g11links.totalCount')" = "1" ]
pass "issue-11: TagsFilter containsAll-only filter parses"

# containsAny-only must succeed
G11_ANY=$(gql '{"query":"{ g11links(where: {tags: {containsAny: [\"rust\", \"go\"]}}) { totalCount } }"}')
assert_gql_ok "$G11_ANY"
[ "$(echo "$G11_ANY" | jq -r '.data.g11links.totalCount')" = "1" ]
pass "issue-11: TagsFilter containsAny-only filter parses"

# Empty filter: rejected at resolve time
G11_EMPTY=$(gql '{"query":"{ g11links(where: {tags: {}}) { totalCount } }"}')
assert_gql_errors "$G11_EMPTY"
printf '%s' "$G11_EMPTY" | grep -q "tags filter requires at least one of"
pass "issue-11: empty TagsFilter rejected with clear error"

# Empty containsAll: rejected
G11_EMPTY_ALL=$(gql '{"query":"{ g11links(where: {tags: {containsAll: []}}) { totalCount } }"}')
assert_gql_errors "$G11_EMPTY_ALL"
printf '%s' "$G11_EMPTY_ALL" | grep -q "containsAll cannot be empty"
pass "issue-11: empty containsAll rejected with clear error"

# Empty containsAny: rejected
G11_EMPTY_ANY=$(gql '{"query":"{ g11links(where: {tags: {containsAny: []}}) { totalCount } }"}')
assert_gql_errors "$G11_EMPTY_ANY"
printf '%s' "$G11_EMPTY_ANY" | grep -q "containsAny cannot be empty"
pass "issue-11: empty containsAny rejected with clear error"

# Schema introspection: containsAll / containsAny must be nullable list
G11_SDL=$(gql '{"query":"{ __type(name: \"TagsFilter\") { inputFields { name type { kind ofType { kind name ofType { kind name } } } } } }"}')
echo "$G11_SDL" | jq -e '.data.__type.inputFields[] | select(.name == "containsAll") | .type.kind == "LIST"' >/dev/null
echo "$G11_SDL" | jq -e '.data.__type.inputFields[] | select(.name == "containsAny") | .type.kind == "LIST"' >/dev/null
pass "issue-11: TagsFilter introspection confirms containsAll/containsAny are nullable lists"

gql '{"query":"mutation { executeSql(sql: \"DROP TABLE g11link CASCADE\") { message } }"}' >/dev/null

# 45.A1 — Cross-mutation parity after a failed UNIQUE INSERT (issue #4 group A1).
# duplicate_insert_does_not_leave_ghost_doogats_row pins the index invariant at
# the unit level; this sub-block proves all THREE GraphQL write paths
# (updateDoogat / createDoogat / deleteDoogat) still work after a UNIQUE
# rollback. Issue #4 explicitly named all three as broken on the regression.
ddl '{"query":"mutation { executeSql(sql: \"CREATE TABLE a1item (title VARCHAR(255) NOT NULL, name VARCHAR(255) NOT NULL, UNIQUE(name))\") { message } }"}'
A1_VALID=$(gql '{"query":"mutation { executeSql(sql: \"INSERT INTO a1item (title, name) VALUES (\\\"a\\\", \\\"unique1\\\")\") { message } }"}')
A1_VALID_ID=$(printf '%s' "$A1_VALID" | extract_id)
[ -n "$A1_VALID_ID" ]
A1_DUP=$(gql '{"query":"mutation { executeSql(sql: \"INSERT INTO a1item (title, name) VALUES (\\\"b\\\", \\\"unique1\\\")\") { message } }"}')
assert_gql_errors "$A1_DUP"
printf '%s' "$A1_DUP" | grep -q 'UNIQUE'
# updateDoogat after the failure
A1_UPD=$(gql "{\"query\":\"mutation { updateDoogat(input: { id: \\\"$A1_VALID_ID\\\", tags: [\\\"a1-recovered\\\"] }) { id tags } }\"}")
assert_gql_ok "$A1_UPD"
printf '%s' "$A1_UPD" | grep -q 'a1-recovered'
# createDoogat after the failure (different unique key)
A1_CREATE=$(gql '{"query":"mutation { createDoogat(input: { type: \"a1item\", title: \"created-after-rollback\", fields: \"{\\\"name\\\":\\\"unique2\\\"}\" }) { id title } }"}')
assert_gql_ok "$A1_CREATE"
A1_CREATE_ID=$(printf '%s' "$A1_CREATE" | sed -n 's/.*"id":"\([^"]*\)".*/\1/p')
[ -n "$A1_CREATE_ID" ]
# deleteDoogat after the failure (delete the row created above so we don't
# disturb the surviving baseline row)
A1_DEL=$(gql "{\"query\":\"mutation { deleteDoogat(id: \\\"$A1_CREATE_ID\\\") }\"}")
assert_gql_ok "$A1_DEL"
printf '%s' "$A1_DEL" | grep -q 'true'
gql '{"query":"mutation { executeSql(sql: \"DROP TABLE a1item CASCADE\") { message } }"}' >/dev/null
pass "issue-4-A1: failed UNIQUE INSERT does not break update/create/delete mutations"

# 45.A3 — Cross-table isolation (issue #4 group A3). A failed UNIQUE INSERT on
# one table must not leak into a sibling table. Proves the savepoint rollback
# is scoped correctly.
VER=$(gql '{"query":"{ schemaVersion }"}' | sed -n 's/.*"schemaVersion":\([0-9]*\).*/\1/p')
VER=${VER:-0}
gql '{"query":"mutation { executeSql(sql: \"CREATE TABLE a3thing (title VARCHAR(255) NOT NULL)\") { message } }"}' >/dev/null
gql '{"query":"mutation { executeSql(sql: \"CREATE TABLE a3item (title VARCHAR(255) NOT NULL, name VARCHAR(255) NOT NULL, UNIQUE(name))\") { message } }"}' >/dev/null
wait_schema_reload "$VER"
A3_THING=$(gql '{"query":"mutation { executeSql(sql: \"INSERT INTO a3thing (title) VALUES (\\\"t1\\\")\") { message } }"}')
A3_THING_ID=$(printf '%s' "$A3_THING" | extract_id)
[ -n "$A3_THING_ID" ]
gql '{"query":"mutation { executeSql(sql: \"INSERT INTO a3item (title, name) VALUES (\\\"a\\\", \\\"u1\\\")\") { message } }"}' >/dev/null
A3_DUP=$(gql '{"query":"mutation { executeSql(sql: \"INSERT INTO a3item (title, name) VALUES (\\\"b\\\", \\\"u1\\\")\") { message } }"}')
assert_gql_errors "$A3_DUP"
printf '%s' "$A3_DUP" | grep -q 'UNIQUE'
# Table a3thing must still be writable via updateDoogat after the A3 failure.
A3_UPD=$(gql "{\"query\":\"mutation { updateDoogat(input: { id: \\\"$A3_THING_ID\\\", tags: [\\\"a3-isolated\\\"] }) { id tags } }\"}")
assert_gql_ok "$A3_UPD"
printf '%s' "$A3_UPD" | grep -q 'a3-isolated'
gql '{"query":"mutation { executeSql(sql: \"DROP TABLE a3thing CASCADE\") { message } }"}' >/dev/null
gql '{"query":"mutation { executeSql(sql: \"DROP TABLE a3item CASCADE\") { message } }"}' >/dev/null
pass "issue-4-A3: failed INSERT on a3item does not corrupt a3thing"

# 45.R10 — RESTRICT on NOT NULL REFERENCES blocks the parent delete through
# both the SQL and deleteDoogat GraphQL surfaces (#10). The Rust unit tests
# delete_rejected_by_not_null_references_issue_10 pin the engine invariant.
# This scenario confirms the GraphQL layer propagates the error and leaves
# both the parent and the child row intact.
ddl '{"query":"mutation { executeSql(sql: \"CREATE TABLE r10link (url VARCHAR(255) NOT NULL)\") { message } }"}'
ddl '{"query":"mutation { executeSql(sql: \"CREATE TABLE r10cat (name VARCHAR(255) NOT NULL)\") { message } }"}'
ddl '{"query":"mutation { executeSql(sql: \"CREATE TABLE \\\"r10-mem\\\" (link_id VARCHAR(255) NOT NULL REFERENCES r10link(id), cat_id VARCHAR(255) NOT NULL REFERENCES r10cat(id), UNIQUE(link_id, cat_id))\") { message } }"}'
# Use raw curl (not the `-sf` gql helper) so a transient non-2xx surfaces as
# a diagnostic instead of killing the script via set -e.
gql_noexit() {
  curl -s "$GQL_URL" \
    -H "Authorization: Bearer $TOKEN" \
    -H "Content-Type: application/json" \
    -d "$1"
}
# Extract id from a JSON string; sed -n won't trip pipefail on no-match.
extract_id_str() {
  local s="$1"
  printf '%s' "$s" | sed -n 's/.*"message":"\([^"]*\)".*/\1/p'
}
R10_L_RESP=$(gql_noexit '{"query":"mutation { executeSql(sql: \"INSERT INTO r10link (title, url) VALUES (\\\"L\\\", \\\"https://r10.example\\\")\") { message } }"}')
R10_L_ID=$(extract_id_str "$R10_L_RESP")
[ -n "$R10_L_ID" ] || { printf '  ✗ 45.R10: INSERT r10link returned no id: %s\n' "$R10_L_RESP" >&2; exit 1; }
R10_C_RESP=$(gql_noexit '{"query":"mutation { executeSql(sql: \"INSERT INTO r10cat (title, name) VALUES (\\\"C\\\", \\\"c\\\")\") { message } }"}')
R10_C_ID=$(extract_id_str "$R10_C_RESP")
[ -n "$R10_C_ID" ] || { printf '  ✗ 45.R10: INSERT r10cat returned no id: %s\n' "$R10_C_RESP" >&2; exit 1; }
gql_noexit "{\"query\":\"mutation { executeSql(sql: \\\"INSERT INTO \\\\\\\"r10-mem\\\\\\\" (title, link_id, cat_id) VALUES (\\\\\\\"M\\\\\\\", \\\\\\\"$R10_L_ID\\\\\\\", \\\\\\\"$R10_C_ID\\\\\\\")\\\") { message } }\"}" >/dev/null
# SQL DELETE via executeSql must surface the RESTRICT error via "errors"
R10_SQL_ERR=$(gql_noexit "{\"query\":\"mutation { executeSql(sql: \\\"DELETE FROM r10link WHERE id = \\\\\\\"$R10_L_ID\\\\\\\"\\\") { message } }\"}")
assert_gql_errors "$R10_SQL_ERR"
printf '%s' "$R10_SQL_ERR" | grep -q "NOT NULL REFERENCES"
printf '%s' "$R10_SQL_ERR" | grep -q "r10-mem"
# deleteDoogat GraphQL mutation must also fail
R10_GQL_ERR=$(gql_noexit "{\"query\":\"mutation { deleteDoogat(id: \\\"$R10_L_ID\\\") }\"}")
assert_gql_errors "$R10_GQL_ERR"
printf '%s' "$R10_GQL_ERR" | grep -q "NOT NULL REFERENCES"
# Parent + child rows must both still exist (executeSql is a Mutation field).
R10_PARENT=$(gql_noexit "{\"query\":\"mutation { executeSql(sql: \\\"SELECT COUNT(*) FROM r10link WHERE id = \\\\\\\"$R10_L_ID\\\\\\\"\\\") { rows } }\"}")
printf '%s' "$R10_PARENT" | grep -q '"rows":\["\[\\"1\\"\]"\]'
R10_CHILD=$(gql_noexit "{\"query\":\"mutation { executeSql(sql: \\\"SELECT COUNT(*) FROM \\\\\\\"r10-mem\\\\\\\" WHERE link_id = \\\\\\\"$R10_L_ID\\\\\\\"\\\") { rows } }\"}")
printf '%s' "$R10_CHILD" | grep -q '"rows":\["\[\\"1\\"\]"\]'
# After deleting the child, the parent delete succeeds
gql_noexit "{\"query\":\"mutation { executeSql(sql: \\\"DELETE FROM \\\\\\\"r10-mem\\\\\\\" WHERE link_id = \\\\\\\"$R10_L_ID\\\\\\\"\\\") { message } }\"}" >/dev/null
R10_OK=$(gql_noexit "{\"query\":\"mutation { executeSql(sql: \\\"DELETE FROM r10link WHERE id = \\\\\\\"$R10_L_ID\\\\\\\"\\\") { affected } }\"}")
assert_gql_ok "$R10_OK"
printf '%s' "$R10_OK" | grep -q '"affected":1'
gql_noexit '{"query":"mutation { executeSql(sql: \"DROP TABLE \\\"r10-mem\\\" CASCADE\") { message } }"}' >/dev/null
gql_noexit '{"query":"mutation { executeSql(sql: \"DROP TABLE r10link CASCADE\") { message } }"}' >/dev/null
gql_noexit '{"query":"mutation { executeSql(sql: \"DROP TABLE r10cat CASCADE\") { message } }"}' >/dev/null
pass "issue-10: RESTRICT blocks delete via SQL and deleteDoogat"

# 45.A2 — Ghost-row fix persists across server restart (issue #4 group A2).
# Seed a UNIQUE failure on the running server, kill it, restart on the same
# $TMPDIR, then verify a fresh GraphQL write path still succeeds against the
# restarted process. This catches a regression where the fix lives in memory
# only and doesn't actually persist to the SQLite index file.
ddl '{"query":"mutation { executeSql(sql: \"CREATE TABLE a2persist (title VARCHAR(255) NOT NULL, name VARCHAR(255) NOT NULL, UNIQUE(name))\") { message } }"}'
A2_VALID=$(gql '{"query":"mutation { executeSql(sql: \"INSERT INTO a2persist (title, name) VALUES (\\\"seed\\\", \\\"uniq_a2\\\")\") { message } }"}')
A2_VALID_ID=$(printf '%s' "$A2_VALID" | extract_id)
[ -n "$A2_VALID_ID" ]
A2_DUP=$(gql '{"query":"mutation { executeSql(sql: \"INSERT INTO a2persist (title, name) VALUES (\\\"dup\\\", \\\"uniq_a2\\\")\") { message } }"}')
assert_gql_errors "$A2_DUP"
# Kill the server, confirm it's gone, then restart on the same $TMPDIR.
kill "$SERVER_PID" 2>/dev/null || true
wait "$SERVER_PID" 2>/dev/null || true
$DDB serve --port "$SERVER_PORT" --pg-port "$PG_PORT" &
SERVER_PID=$!
for i in $(seq 1 20); do
  if curl -sf "$GQL_URL" \
    -H "Authorization: Bearer $TOKEN" \
    -H "Content-Type: application/json" \
    -d '{"query":"{ typeDefs { name } }"}' >/dev/null 2>&1; then
    break
  fi
  sleep 0.2
done
# Restarted server must be able to updateDoogat on the pre-restart row.
A2_UPD=$(gql "{\"query\":\"mutation { updateDoogat(input: { id: \\\"$A2_VALID_ID\\\", tags: [\\\"restart-survived\\\"] }) { id tags } }\"}")
assert_gql_ok "$A2_UPD"
printf '%s' "$A2_UPD" | grep -q 'restart-survived'
# And still accept fresh INSERTs on the same table.
A2_FRESH=$(gql '{"query":"mutation { executeSql(sql: \"INSERT INTO a2persist (title, name) VALUES (\\\"fresh\\\", \\\"uniq_a2_post\\\")\") { message } }"}')
assert_gql_ok "$A2_FRESH"
printf '%s' "$A2_FRESH" | extract_id >/dev/null
gql '{"query":"mutation { executeSql(sql: \"DROP TABLE a2persist CASCADE\") { message } }"}' >/dev/null
pass "issue-4-A2: ghost-row fix persists across server restart"

# 49. PRD 00131: structured-error code propagation through typed mutations.
# Restores `extensions.code = "UNIQUE_VIOLATION"` (and NOT_NULL_VIOLATION)
# on duplicate-key and missing-required-column violations through
# `createDoogat` and `createMany`. The codes regressed in 0.2.5 when the
# service-layer batch_create path flattened structured errors to plain
# `Validation` strings; PRD 00131 swaps both flatten sites
# (validation.rs:234, crud.rs:499) back to `DoogatError::unique_violation`.
# Placed here while the GraphQL server is still running (it shuts down
# below before sections 46-48, which use the CLI).
echo "=== PRD 00131: structured-error code propagation ==="

# 49.1 — GraphQL UNIQUE violation on createDoogat carries extensions.code.
ddl '{"query":"mutation { executeSql(sql: \"CREATE TABLE puv_link (title VARCHAR(255), slug VARCHAR(255) NOT NULL, space VARCHAR(255) NOT NULL, UNIQUE(slug, space))\") { message } }"}'

SE_FIRST=$(gql '{"query":"mutation { createDoogat(input: {type: \"puv_link\", title: \"first\", fields: \"{\\\"slug\\\":\\\"hn\\\",\\\"space\\\":\\\"news\\\"}\"}) { id } }"}')
assert_gql_ok "$SE_FIRST"

SE_DUP=$(gql '{"query":"mutation { createDoogat(input: {type: \"puv_link\", title: \"dup\", fields: \"{\\\"slug\\\":\\\"hn\\\",\\\"space\\\":\\\"news\\\"}\"}) { id } }"}')
assert_gql_errors "$SE_DUP"
echo "$SE_DUP" | jq -e '.errors[0].extensions.code == "UNIQUE_VIOLATION"' >/dev/null
echo "$SE_DUP" | jq -e '.errors[0].extensions.columns == ["slug", "space"]' >/dev/null
echo "$SE_DUP" | jq -e '.errors[0].extensions.values | type == "array" and length == 2' >/dev/null
pass "49.1: createDoogat UNIQUE violation carries extensions.code = UNIQUE_VIOLATION"

# 49.2 — GraphQL NOT NULL violation on createDoogat carries extensions.code.
# Sanity-locks the structured-shape audit so a future flatten can't slip
# in for NOT_NULL while still passing 49.1.
SE_NN=$(gql '{"query":"mutation { createDoogat(input: {type: \"puv_link\", title: \"missing-slug\", fields: \"{\\\"space\\\":\\\"news\\\"}\"}) { id } }"}')
assert_gql_errors "$SE_NN"
echo "$SE_NN" | jq -e '.errors[0].extensions.code == "NOT_NULL_VIOLATION"' >/dev/null
echo "$SE_NN" | jq -e '.errors[0].extensions.column == "slug"' >/dev/null
pass "49.2: createDoogat NOT NULL violation carries extensions.code = NOT_NULL_VIOLATION"

# 49.3 — createMany single-input duplicate under ERROR carries extensions.code.
SE_CM=$(gql '{"query":"mutation { createMany(inputs: [{type: \"puv_link\", title: \"cm-dup\", fields: \"{\\\"slug\\\":\\\"hn\\\",\\\"space\\\":\\\"news\\\"}\"}], onConflict: ERROR) { id } }"}')
assert_gql_errors "$SE_CM"
echo "$SE_CM" | jq -e '.errors[0].extensions.code == "UNIQUE_VIOLATION"' >/dev/null
pass "49.3: createMany ERROR UNIQUE violation carries extensions.code = UNIQUE_VIOLATION"

# 49.4 — createMany multi-input intra-batch duplicate under ERROR carries
# extensions.code. Exercises the batch_create intra-batch flatten site
# fixed by PRD 00131 (crud.rs:499).
SE_CM_INTRA=$(gql '{"query":"mutation { createMany(inputs: [{type: \"puv_link\", title: \"cm-a\", fields: \"{\\\"slug\\\":\\\"twin\\\",\\\"space\\\":\\\"news\\\"}\"}, {type: \"puv_link\", title: \"cm-b\", fields: \"{\\\"slug\\\":\\\"twin\\\",\\\"space\\\":\\\"news\\\"}\"}], onConflict: ERROR) { id } }"}')
assert_gql_errors "$SE_CM_INTRA"
echo "$SE_CM_INTRA" | jq -e '.errors[0].extensions.code == "UNIQUE_VIOLATION"' >/dev/null
pass "49.4: createMany intra-batch ERROR carries extensions.code = UNIQUE_VIOLATION"

# Cleanup: drop the typedef so subsequent runs start clean.
ddl '{"query":"mutation { executeSql(sql: \"DROP TABLE puv_link\") { message } }"}'

# 50. PRD 00132: ALTER TABLE foo RENAME TO bar across protocols.
# 50a. via GraphQL executeSql
ddl '{"query":"mutation { executeSql(sql: \"CREATE TABLE rngql_src (title VARCHAR(64))\") { message } }"}'
ddl '{"query":"mutation { executeSql(sql: \"ALTER TABLE rngql_src RENAME TO rngql_dst\") { message } }"}'
RNGQL_COUNT=$(gql '{"query":"mutation { executeSql(sql: \"SELECT count(*) FROM rngql_dst\") { message } }"}')
echo "$RNGQL_COUNT" | grep -q '"data"'
RNGQL_OLD=$(gql '{"query":"mutation { executeSql(sql: \"SELECT count(*) FROM rngql_src\") { message } }"}')
echo "$RNGQL_OLD" | grep -q '"errors"'
pass "50a: ALTER TABLE RENAME TO via GraphQL executeSql succeeds; old name no longer resolves"

# 50b. MySQL alias rejected with explicit message (no internal error leak).
RNGQL_ALIAS=$(gql '{"query":"mutation { executeSql(sql: \"RENAME TABLE rngql_dst TO rngql_dst2\") { message } }"}')
echo "$RNGQL_ALIAS" | jq -e '.errors[0].message | contains("RENAME TABLE not supported")' >/dev/null
pass "50b: MySQL RENAME TABLE alias rejected with explicit ALTER TABLE hint"

# 50c. via PgWire when psql is available.
if command -v psql >/dev/null 2>&1; then
  PGPASSWORD="$TOKEN" psql -h 127.0.0.1 -p "$PG_PORT" -U ddb -d ddb -c "CREATE TABLE rnpg_src (title VARCHAR(64))" >/dev/null
  PGPASSWORD="$TOKEN" psql -h 127.0.0.1 -p "$PG_PORT" -U ddb -d ddb -c "ALTER TABLE rnpg_src RENAME TO rnpg_dst" >/dev/null
  PGPASSWORD="$TOKEN" psql -h 127.0.0.1 -p "$PG_PORT" -U ddb -d ddb -tAc "SELECT count(*) FROM rnpg_dst" | grep -q "0"
  pass "50c: ALTER TABLE RENAME TO via PgWire succeeds"
fi

# Cleanup the rename-test typedefs so subsequent runs start clean.
ddl '{"query":"mutation { executeSql(sql: \"DROP TABLE rngql_dst\") { message } }"}'
if command -v psql >/dev/null 2>&1; then
  ddl '{"query":"mutation { executeSql(sql: \"DROP TABLE rnpg_dst\") { message } }"}'
fi

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

# UPDATE/DELETE WHERE id no-match semantics (continuation of section 30, #5)
cd "$TMPDIR"
$DDB query "CREATE TABLE smokenomatch (name TEXT, score INTEGER)" | grep -q "table smokenomatch created"
NOMATCH_ID=$($DDB query "INSERT INTO smokenomatch (name, score) VALUES ('alpha', 1)")
# B1: UPDATE with nonexistent id returns 0 rows affected (not an error)
$DDB query "UPDATE smokenomatch SET score = 1 WHERE id = 'nonexistent_id_00000000000000'" | grep -q "0 row(s) affected"
# B2: DELETE with nonexistent id returns 0 rows affected (not an error)
$DDB query "DELETE FROM smokenomatch WHERE id = 'nonexistent_id_00000000000000'" | grep -q "0 row(s) affected"
# B3: IN clause mixing missing and valid ids still affects 1 row
$DDB query "UPDATE smokenomatch SET score = 7 WHERE id IN ('nope', '$NOMATCH_ID')" | grep -q "1 row(s) affected"
# B4: compound predicate with valid id + non-matching column returns 0 rows affected
$DDB query "UPDATE smokenomatch SET score = 99 WHERE id = '$NOMATCH_ID' AND name = 'wrongname'" | grep -q "0 row(s) affected"
# B5: valid id on the fast path still affects 1 row
$DDB query "UPDATE smokenomatch SET score = 42 WHERE id = '$NOMATCH_ID'" | grep -q "1 row(s) affected"
$DDB query "SELECT score FROM smokenomatch WHERE id = '$NOMATCH_ID'" | grep -q "42"
$DDB query "DROP TABLE smokenomatch CASCADE" | grep -q "dropped"
pass "update/delete WHERE id no-match semantics (#5)"

# 30.F1 — composite UNIQUE duplicate rejection surfaces a clear error on the
# CLI (#9 group F1). The Rust unit test
# composite_unique_duplicate_rejected_with_clear_error_issue_9_f1 covers the
# error message at the engine level; this check confirms the CLI path
# propagates the same error with the table context intact.
cd "$TMPDIR"
$DDB query 'CREATE TABLE f1mship (title VARCHAR(255), link_id VARCHAR(255), category VARCHAR(255), UNIQUE(link_id, category))' | grep -q "table f1mship created"
$DDB query "INSERT INTO f1mship (title, link_id, category) VALUES ('a', 'link1', 'cat1')" >/dev/null
F1_DUP=$($DDB query "INSERT INTO f1mship (title, link_id, category) VALUES ('b', 'link1', 'cat1')" 2>&1 || true)
echo "$F1_DUP" | grep -q "UNIQUE"
echo "$F1_DUP" | grep -qE "f1mship|link_id|category"
$DDB query "DROP TABLE f1mship CASCADE" | grep -q "dropped"
pass "issue-9-F1: composite UNIQUE duplicate rejected with clear error"

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

# 46. title_template REFERENCES resolution (PRD 00127).
# Server has been down since section 28, so use the CLI directly.
echo "=== title_template REFERENCES resolution ==="
TT_DIR="$(mktemp -d)"
cd "$TT_DIR"
$DDB init >/dev/null
$DDB query "CREATE TABLE tt_link (url TEXT)" >/dev/null
$DDB query "CREATE TABLE tt_category (fqn TEXT)" >/dev/null
$DDB query "CREATE TABLE tt_membership (link TEXT REFERENCES tt_link, category TEXT REFERENCES tt_category)" >/dev/null
$DDB query "ALTER TABLE tt_membership SET TITLE TEMPLATE '{link.title} in {category.fqn}'" >/dev/null
pass "46: declared dotted title_template"

TT_LINK_ID=$($DDB query "INSERT INTO tt_link (title, url) VALUES ('My Link', 'https://x')" | tr -d '[:space:]')
sleep 1
TT_CAT_ID=$($DDB query "INSERT INTO tt_category (title, fqn) VALUES ('Cat', 'A/B')" | tr -d '[:space:]')
sleep 1
TT_MEM_ID=$($DDB query "INSERT INTO tt_membership (link, category) VALUES ('$TT_LINK_ID', '$TT_CAT_ID')" | tr -d '[:space:]')
$DDB query "SELECT title FROM tt_membership WHERE id = '$TT_MEM_ID'" | grep -q "My Link in A/B"
pass "46: composed title 'My Link in A/B' from REFERENCES"

# Bad dotted path is rejected at ALTER TABLE.
TT_BAD_OUT="$($DDB query "ALTER TABLE tt_membership SET TITLE TEMPLATE '{link.does_not_exist}'" 2>&1 || true)"
printf '%s' "$TT_BAD_OUT" | grep -q "does not exist on tt_link"
pass "46: ALTER TABLE rejects bad dotted path"

cd "$TMPDIR"
rm -rf "$TT_DIR"

# 47. ALTER TABLE ALTER COLUMN TYPE (PRD 00128).
# Server is not running at this point; drive via CLI.
echo "=== ALTER TABLE ALTER COLUMN TYPE ==="
AC_DIR="$(mktemp -d)"
cd "$AC_DIR"
$DDB init >/dev/null
$DDB query "CREATE TABLE ac_link (url VARCHAR(32))" >/dev/null

# Insert a row at the boundary.
AC_SHORT=$(printf 'a%.0s' $(seq 1 32))
AC_ID1=$($DDB query "INSERT INTO ac_link (title, url) VALUES ('boundary', '$AC_SHORT')" | tr -d '[:space:]')
[ -n "$AC_ID1" ]
pass "47: baseline VARCHAR(32) insert at boundary"

# Pre-ALTER: long insert must fail.
AC_LONG=$(printf 'b%.0s' $(seq 1 80))
AC_FAIL_OUT="$($DDB query "INSERT INTO ac_link (title, url) VALUES ('toolong', '$AC_LONG')" 2>&1 || true)"
printf '%s' "$AC_FAIL_OUT" | grep -q "exceeds limit"
pass "47: pre-ALTER INSERT rejects 80-char value for VARCHAR(32)"

# Widen to VARCHAR(100).
$DDB query "ALTER TABLE ac_link ALTER COLUMN url TYPE VARCHAR(100)" >/dev/null
pass "47: widen VARCHAR(32) -> VARCHAR(100) succeeds"

# Post-ALTER: the same long value succeeds.
sleep 1
AC_ID2=$($DDB query "INSERT INTO ac_link (title, url) VALUES ('now-ok', '$AC_LONG')" | tr -d '[:space:]')
[ -n "$AC_ID2" ]
pass "47: post-ALTER INSERT accepts 80-char value"

# Narrowing with over-limit rows is rejected with a row-count message.
AC_NARROW_OUT="$($DDB query "ALTER TABLE ac_link ALTER COLUMN url TYPE VARCHAR(5)" 2>&1 || true)"
printf '%s' "$AC_NARROW_OUT" | grep -q "cannot narrow"
printf '%s' "$AC_NARROW_OUT" | grep -q "existing rows exceed limit"
pass "47: narrowing rejects with cannot-narrow row-count message"

# Widen to TEXT and insert a 2000-char value.
$DDB query "ALTER TABLE ac_link ALTER COLUMN url TYPE TEXT" >/dev/null
sleep 1
AC_HUGE=$(printf 'c%.0s' $(seq 1 2000))
AC_ID3=$($DDB query "INSERT INTO ac_link (title, url) VALUES ('text-row', '$AC_HUGE')" | tr -d '[:space:]')
[ -n "$AC_ID3" ]
pass "47: VARCHAR -> TEXT widening persists and accepts long values"

cd "$TMPDIR"
rm -rf "$AC_DIR"

# 48. PRD 00129: typed write blocker & simplification SQL surface.
# Server is not running at this point; drive via CLI.
echo "=== PRD 00129: typed write blockers + ON DELETE CASCADE + INDEX no-op ==="
P9_DIR="$(mktemp -d)"
cd "$P9_DIR"
$DDB init >/dev/null

# §3b: CREATE INDEX IF NOT EXISTS is accepted as a no-op so legacy
# startup migrations keep working.
$DDB query "CREATE TABLE p9_link (title TEXT, url VARCHAR(255))" >/dev/null
P9_INDEX_OUT="$($DDB query "CREATE INDEX IF NOT EXISTS idx_p9_url ON p9_link(url)" 2>&1)"
printf '%s' "$P9_INDEX_OUT" | grep -q "ignored"
pass "48: CREATE INDEX IF NOT EXISTS accepted as no-op"

# Plain CREATE INDEX still rejects.
P9_PLAIN_OUT="$($DDB query "CREATE INDEX idx_plain ON p9_link(url)" 2>&1 || true)"
printf '%s' "$P9_PLAIN_OUT" | grep -q "CREATE INDEX not supported"
pass "48: plain CREATE INDEX still rejects"

# §2: ON DELETE CASCADE walks one level.
$DDB query "CREATE TABLE p9_membership (title TEXT, link VARCHAR(255) REFERENCES p9_link(id) ON DELETE CASCADE)" >/dev/null
P9_LINK_ID=$($DDB query "INSERT INTO p9_link (title, url) VALUES ('Parent', 'https://x')" | tr -d '[:space:]')
sleep 1
P9_MEM_ID=$($DDB query "INSERT INTO p9_membership (title, link) VALUES ('Child', '$P9_LINK_ID')" | tr -d '[:space:]')
[ -n "$P9_MEM_ID" ]
pass "48: typed insert into cascade-bound child succeeds"

$DDB delete "$P9_LINK_ID" >/dev/null
P9_AFTER_LINK="$($DDB query "SELECT id FROM p9_link WHERE id = '$P9_LINK_ID'" 2>&1 || true)"
P9_AFTER_MEM="$($DDB query "SELECT id FROM p9_membership WHERE id = '$P9_MEM_ID'" 2>&1 || true)"
# `grep -qv` returned 0 on empty stdin under ugrep on developer hosts but 1
# under GNU grep on CI, masking the assertion. Use bash's [[ != *X* ]] which
# does abort under set -e and behaves the same regardless of grep flavor.
[[ "$P9_AFTER_LINK" != *"$P9_LINK_ID"* ]]
[[ "$P9_AFTER_MEM" != *"$P9_MEM_ID"* ]]
pass "48: ON DELETE CASCADE removes parent and child in one delete"

# §2: ON DELETE RESTRICT (default) blocks parent delete.
$DDB query "CREATE TABLE p9_blocker (title TEXT, link VARCHAR(255) NOT NULL REFERENCES p9_link(id))" >/dev/null
P9_LINK_ID2=$($DDB query "INSERT INTO p9_link (title, url) VALUES ('R Parent', 'https://r')" | tr -d '[:space:]')
sleep 1
$DDB query "INSERT INTO p9_blocker (title, link) VALUES ('Block', '$P9_LINK_ID2')" >/dev/null
P9_RESTRICT_OUT="$($DDB delete "$P9_LINK_ID2" 2>&1 || true)"
printf '%s' "$P9_RESTRICT_OUT" | grep -q "NOT NULL REFERENCES from p9_blocker.link"
pass "48: ON DELETE RESTRICT (default) rejects parent delete"

# §2: cascade cycle detection.
$DDB query "CREATE TABLE p9_a (title TEXT)" >/dev/null
$DDB query "CREATE TABLE p9_b (title TEXT)" >/dev/null
$DDB query "ALTER TABLE p9_a ADD COLUMN b VARCHAR(255) REFERENCES p9_b(id) ON DELETE CASCADE" >/dev/null
$DDB query "ALTER TABLE p9_b ADD COLUMN a VARCHAR(255) REFERENCES p9_a(id) ON DELETE CASCADE" >/dev/null
P9_A_ID=$($DDB query "INSERT INTO p9_a (title) VALUES ('A')" | tr -d '[:space:]')
sleep 1
P9_B_ID=$($DDB query "INSERT INTO p9_b (title) VALUES ('B')" | tr -d '[:space:]')
$DDB query "UPDATE p9_a SET b = '$P9_B_ID' WHERE id = '$P9_A_ID'" >/dev/null
$DDB query "UPDATE p9_b SET a = '$P9_A_ID' WHERE id = '$P9_B_ID'" >/dev/null
P9_CYCLE_OUT="$($DDB delete "$P9_A_ID" 2>&1 || true)"
printf '%s' "$P9_CYCLE_OUT" | grep -q "cascade delete would form a cycle"
pass "48: ON DELETE CASCADE cycle detection rejects"

cd "$TMPDIR"
rm -rf "$P9_DIR"

echo "=== all integration tests passed ==="
