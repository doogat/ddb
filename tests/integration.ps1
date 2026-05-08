#!/usr/bin/env pwsh

# Integration tests: server, sync, CRDT conflicts, bundles, advanced SQL.
# Runs smoke tests first, then continues with full integration suite.

$ErrorActionPreference = "Stop"

# --- Build + lint when DDB_BIN is not injected ---
$prepLabel = "prebuilt binary"
if (-not $env:DDB_BIN) {
    cargo build --quiet
    cargo clippy --workspace --quiet
    cargo bench --no-run --quiet 2>$null
    $prepLabel = "clippy + bench compile"
}

if ($env:DDB_BIN) {
    $DDB = $env:DDB_BIN
} else {
    $meta = cargo metadata --format-version=1 --no-deps | ConvertFrom-Json
    $DDB = Join-Path $meta.target_directory "debug" "ddb.exe"
}
$env:DDB_BIN = $DDB

# --- Integration tests ---
function New-TempDir {
    $p = Join-Path ([System.IO.Path]::GetTempPath()) ([System.IO.Path]::GetRandomFileName())
    New-Item -ItemType Directory -Path $p | Out-Null
    return $p
}

$TMPDIR = New-TempDir
$REMOTE_DIR = New-TempDir
$NODE1_DIR = New-TempDir
$NODE2_DIR = New-TempDir
$NODE3_DIR = New-TempDir

function Cleanup {
    if ($env:CI) { return }
    foreach ($d in @($TMPDIR, $REMOTE_DIR, $NODE1_DIR, $NODE2_DIR, $NODE3_DIR)) {
        if (Test-Path $d) { Remove-Item -Recurse -Force $d -ErrorAction SilentlyContinue }
    }
    if ($script:STALE_REMOTE -and (Test-Path $script:STALE_REMOTE)) { Remove-Item -Recurse -Force $script:STALE_REMOTE -ErrorAction SilentlyContinue }
    if ($script:STALE_N1 -and (Test-Path $script:STALE_N1)) { Remove-Item -Recurse -Force $script:STALE_N1 -ErrorAction SilentlyContinue }
    if ($script:STALE_N2 -and (Test-Path $script:STALE_N2)) { Remove-Item -Recurse -Force $script:STALE_N2 -ErrorAction SilentlyContinue }
}

trap { Cleanup }

# --- Run smoke tests first (after Cleanup is defined so trap can call it) ---
$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
& "$scriptDir/smoke.ps1"

Push-Location $TMPDIR

function pass($msg) { Write-Host "  ✓ $msg" }

function ddb {
    $raw = & $DDB @args 2>&1
    $lines = @($raw) | ForEach-Object { "$_" -replace '\x1b\[[0-9;]*m', '' } |
        Where-Object { $_ -notmatch '^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}' -and $_ -ne '' }
    $text = [string]::Join("`n", @($lines))
    if ($LASTEXITCODE -ne 0) { throw "ddb $($args -join ' ') failed: $text" }
    return $text
}

function ddb-fails {
    & $DDB @args 2>&1 | Out-Null
    return ($LASTEXITCODE -ne 0)
}

Write-Host "=== integration tests ==="

# Init a repo for server and CLI integration tests
ddb init . | Out-Null
$ID1 = ddb create --title "First note" --tags "test,smoke" --body "Hello world"
$ID2 = ddb create --title "Links to first" --body "See [[$ID1]]"
ddb update $ID1 --title "First note (edited)" --tags "test,smoke,updated"
ddb update $ID1 --body "- [ ] open task`n- [x] done task`n- [i] 2026-01-01 10:00 - info note"
ddb reindex | Out-Null
ddb query "CREATE TABLE foo (bar TEXT, baz INTEGER)" | Out-Null
ddb query "INSERT INTO foo (title, bar, baz) VALUES ('for suggest', 'val', 1)" | Out-Null
ddb register-node "integ-node" | Out-Null

# 17. GraphQL server
$SERVER_PORT = 19200 + (Get-Random -Maximum 800)
$PG_PORT = $SERVER_PORT + 1
$serverProc = Start-Process -FilePath $DDB -ArgumentList "serve","--port","$SERVER_PORT","--pg-port","$PG_PORT" -PassThru -NoNewWindow

# Wait for server to start (re-read token each iteration since the server writes it on startup)
$tokenPath = Join-Path $env:USERPROFILE ".config" "ddb" "token"

for ($i = 0; $i -lt 20; $i++) {
    try {
        $TOKEN = if (Test-Path $tokenPath) { (Get-Content $tokenPath -Raw).Trim() } else { "" }
        $null = Invoke-WebRequest -Uri "http://127.0.0.1:$SERVER_PORT/graphql" `
            -Method POST -ContentType "application/json" `
            -Headers @{ Authorization = "Bearer $TOKEN" } `
            -Body '{"query":"{ typeDefs { name } }"}' -ErrorAction Stop
        break
    } catch {
        Start-Sleep -Milliseconds 200
    }
}
$TOKEN = if (Test-Path $tokenPath) { (Get-Content $tokenPath -Raw).Trim() } else { "" }

$GQL_URL = "http://127.0.0.1:$SERVER_PORT/graphql"
$REST_URL = "http://127.0.0.1:$SERVER_PORT/rest"

function gql($body) {
    $resp = Invoke-WebRequest -Uri $GQL_URL -Method POST -ContentType "application/json" `
        -Headers @{ Authorization = "Bearer $TOKEN" } -Body $body -ErrorAction Stop
    if ($resp.Content -is [byte[]]) { return [System.Text.Encoding]::UTF8.GetString($resp.Content) }
    return $resp.Content
}

# Accepts a plain GraphQL query string; wraps it in a JSON envelope via
# ConvertTo-Json so callers never hand-escape JSON inside PowerShell strings.
function gqlq([string]$query) {
    return gql (@{ query = $query } | ConvertTo-Json -Compress)
}
# Wait for the GraphQL schema to reload after a DDL statement. Polls
# schemaVersion until it exceeds $before. Times out after 4 seconds (40 x 100ms).
function waitSchemaReload([int]$before) {
    for ($i = 0; $i -lt 40; $i++) {
        $r = gqlq '{ schemaVersion }'
        if ($r -match '"schemaVersion":(\d+)' -and [int]$Matches[1] -gt $before) { return }
        Start-Sleep -Milliseconds 100
    }
    throw "waitSchemaReload: version did not advance past $before within 4s"
}
function ddl([string]$query) {
    $r = gqlq '{ schemaVersion }'
    $ver = if ($r -match '"schemaVersion":(\d+)') { [int]$Matches[1] } else { 0 }
    gqlq $query | Out-Null
    waitSchemaReload $ver
}

# Extract the "message" field of a successful executeSql response (which is the
# new doogat id). Mirrors the bash extract_id helper for jink-port checks.
function extractId([string]$response) {
    $m = [regex]::Match($response, '"message":"([^"]+)"')
    if ($m.Success) { return $m.Groups[1].Value }
    return ""
}

# Assert that the given response contains "errors" (i.e. GraphQL rejected the
# call). Throws on missing "errors" so set-style error handling kicks in.
function assertGqlErrors([string]$response, [string]$context = "") {
    if ($response -notmatch '"errors"') {
        throw "assertGqlErrors$(if ($context) { " ($context)" }): response had no errors key`n  response: $response"
    }
}

# Assert that the given response is a successful GraphQL response (has "data"
# and no "errors"). Throws on either failure.
function assertGqlOk([string]$response, [string]$context = "") {
    if ($response -match '"errors"') {
        throw "assertGqlOk$(if ($context) { " ($context)" }): response had errors`n  response: $response"
    }
    if ($response -notmatch '"data"') {
        throw "assertGqlOk$(if ($context) { " ($context)" }): response had no data key`n  response: $response"
    }
}

function rest {
    param([string]$path, [string]$method = "GET", [string]$body = $null)
    $params = @{
        Uri = "$REST_URL$path"
        Method = $method
        ContentType = "application/json"
        Headers = @{ Authorization = "Bearer $TOKEN" }
        ErrorAction = "Stop"
    }
    if ($body) { $params.Body = $body }
    $resp = Invoke-WebRequest @params
    return $resp
}

function content($resp) {
    if ($resp.Content -is [byte[]]) { return [System.Text.Encoding]::UTF8.GetString($resp.Content) }
    return $resp.Content
}


# Test auth
try {
    Invoke-WebRequest -Uri $GQL_URL -Method POST -ContentType "application/json" `
        -Body '{"query":"{ typeDefs { name } }"}' -ErrorAction Stop
    throw "should have been 401"
} catch {
    if ($_.Exception.Response.StatusCode.value__ -ne 401) { throw "expected 401, got $($_.Exception.Response.StatusCode.value__)" }
}
pass "serve: auth rejects missing token"

# Health endpoint (no auth required)
$health = Invoke-RestMethod -Uri "http://127.0.0.1:$SERVER_PORT/health" -Method Get
if ($health.status -ne "ok") { throw "health endpoint returned unexpected status: $($health.status)" }
pass "serve: health endpoint"

# Test query
$result = gqlq '{ typeDefs { name } }'
if ($result -notmatch '"typeDefs"') { throw "graphql query failed" }
pass "serve: graphql query"

# Test mutation -- create
$result = gqlq 'mutation { createDoogat(input: { title: "Smoke Server" }) { id title } }'
if ($result -notmatch '"Smoke Server"') { throw "graphql create failed" }
$GQL_ID = if ($result -match '"id":"([^"]+)"') { $Matches[1] } else { throw "no id in response" }
pass "serve: graphql create"

# 17.J1 - Jink schema CREATE TABLE definitions (#9 jink full-sweep section 1).
# Tables persist across sub-blocks 17.J2, 18z8, 18z9, 18z10 below. Do NOT
# drop these in any sub-block other than the final cleanup in 18z10.
$j1Link = gqlq 'mutation { executeSql(sql: "CREATE TABLE link (title VARCHAR(255) NOT NULL, url VARCHAR(255) NOT NULL, subtitle VARCHAR(255), favicon_path VARCHAR(255), favicon_origin VARCHAR(255), bookmark_source VARCHAR(255), last_opened_at VARCHAR(255), description TEXT)") { message } }'
assertGqlOk $j1Link "j1 link create"
if ($j1Link -notmatch '"message":"table link') { throw "j1: link create did not return 'table link'" }
pass "j1: created link table"

$j1Cat = gqlq 'mutation { executeSql(sql: "CREATE TABLE category (title VARCHAR(255) NOT NULL, fqn VARCHAR(255) NOT NULL, space VARCHAR(255) NOT NULL, sort_order INTEGER DEFAULT 0)") { message } }'
assertGqlOk $j1Cat "j1 category create"
if ($j1Cat -notmatch '"message":"table category') { throw "j1: category create did not return 'table category'" }
pass "j1: created category table"

$j1Cm = gqlq 'mutation { executeSql(sql: "CREATE TABLE \"category-membership\" (title VARCHAR(255) NOT NULL, link_id VARCHAR(255) NOT NULL, category_fqn VARCHAR(255) NOT NULL, pinned BOOLEAN DEFAULT FALSE, sort_order INTEGER DEFAULT 0, UNIQUE(link_id, category_fqn))") { message } }'
assertGqlOk $j1Cm "j1 category-membership create"
if ($j1Cm -notmatch '"message":"table category-membership') { throw "j1: category-membership create did not return expected message" }
pass "j1: created category-membership table with composite UNIQUE"

$j1Q = gqlq 'mutation { executeSql(sql: "CREATE TABLE quote (title VARCHAR(255) NOT NULL, author VARCHAR(255), source VARCHAR(255), favorited BOOLEAN DEFAULT FALSE, text TEXT)") { message } }'
assertGqlOk $j1Q "j1 quote create"
if ($j1Q -notmatch '"message":"table quote') { throw "j1: quote create did not return expected message" }
pass "j1: created quote table"

$j1Ss = gqlq 'mutation { executeSql(sql: "CREATE TABLE \"saved-search\" (title VARCHAR(255) NOT NULL, query_raw VARCHAR(255) NOT NULL, query_normalized VARCHAR(255) NOT NULL)") { message } }'
assertGqlOk $j1Ss "j1 saved-search create"
if ($j1Ss -notmatch '"message":"table saved-search') { throw "j1: saved-search create did not return expected message" }
pass "j1: created saved-search table"

$j1Pr = gqlq 'mutation { executeSql(sql: "CREATE TABLE \"pinned-result\" (title VARCHAR(255) NOT NULL, query_normalized VARCHAR(255) NOT NULL, link_id VARCHAR(255) NOT NULL, sort_order INTEGER DEFAULT 0)") { message } }'
assertGqlOk $j1Pr "j1 pinned-result create"
if ($j1Pr -notmatch '"message":"table pinned-result') { throw "j1: pinned-result create did not return expected message" }
pass "j1: created pinned-result table"

$ver = if ((gqlq '{ schemaVersion }') -match '"schemaVersion":(\d+)') { [int]$Matches[1] } else { 0 }
$j1Jc = gqlq 'mutation { executeSql(sql: "CREATE TABLE \"jink-config\" (dashboard_title VARCHAR(255) DEFAULT ''Bobs Battlestation'', quote_rotation_minutes INTEGER DEFAULT 30, links_per_category INTEGER DEFAULT 8, frontend_version VARCHAR(255))") { message } }'
assertGqlOk $j1Jc "j1 jink-config create"
if ($j1Jc -notmatch '"message":"table jink-config') { throw "j1: jink-config create did not return expected message" }
pass "j1: created jink-config table"
waitSchemaReload $ver

# 17.J2 - jink-config singleton + Link CRUD (#9 jink full-sweep sections 3-4).
$j2JcEmpty = gqlq '{ sql(query: "SELECT id FROM \"jink-config\" LIMIT 1") { rows } }'
assertGqlOk $j2JcEmpty "j2 jink-config empty select"
if ($j2JcEmpty -notmatch '"rows"') { throw "j2: empty select missing rows" }
pass "j2: SELECT from empty jink-config"

$j2JcIns = gqlq 'mutation { executeSql(sql: "INSERT INTO \"jink-config\" (title, dashboard_title, quote_rotation_minutes, links_per_category) VALUES (''jink-config'', ''Bobs Battlestation'', 30, 8)") { message } }'
assertGqlOk $j2JcIns "j2 jink-config insert"
if ($j2JcIns -notmatch '"message":"\d+"') { throw "j2: jink-config insert did not return id" }
pass "j2: INSERT jink-config singleton"

$j2JcSel = gqlq '{ sql(query: "SELECT quote_rotation_minutes FROM \"jink-config\" LIMIT 1") { rows } }'
assertGqlOk $j2JcSel "j2 jink-config select"
if ($j2JcSel -notmatch '\\"30\\"') { throw "j2: SELECT did not return 30, got: $j2JcSel" }
pass "j2: SELECT quote_rotation_minutes returns 30"

$j2LinkIns = gqlq 'mutation { executeSql(sql: "INSERT INTO link (title, url, description) VALUES (''Test Link'', ''https://example.com'', ''a test link'')") { message } }'
assertGqlOk $j2LinkIns "j2 link insert"
$JINK_LINK_ID = extractId $j2LinkIns
if (-not $JINK_LINK_ID) { throw "j2: could not extract JINK_LINK_ID from: $j2LinkIns" }
pass "j2: INSERT link returns id"

$j2LinkGql = gqlq "{ links(where: {id: {eq: `"$JINK_LINK_ID`"}}) { items { id title url description tags } } }"
assertGqlOk $j2LinkGql "j2 link graphql query"
if ($j2LinkGql -notmatch '"Test Link"') { throw "j2: GraphQL links query missing 'Test Link'" }
pass "j2: query links via GraphQL"

$j2LinkUpd = gqlq "mutation { executeSql(sql: `"UPDATE link SET favicon_path = 'favicon/x.png', favicon_origin = 'fetched' WHERE id = '$JINK_LINK_ID' AND url = 'https://example.com'`") { message affected } }"
assertGqlOk $j2LinkUpd "j2 link update"
pass "j2: UPDATE link favicon via compound-predicate SQL"

# 18. expanded GraphQL operations
$result = gqlq "mutation { updateDoogat(input: { id: `"$GQL_ID`", title: `"Smoke Updated`" }) { id title } }"
if ($result -notmatch '"Smoke Updated"') { throw "graphql update failed" }
pass "serve: graphql update"

$result = gqlq '{ search(query: "Smoke") { totalCount hits { id title tags type fields created_at } } }'
if ($result -notmatch '"search"') { throw "graphql search failed" }
if ($result -notmatch '"tags"') { throw "graphql search missing tags" }
if ($result -notmatch '"created_at"') { throw "graphql search missing created_at" }
pass "serve: graphql search with enriched fields"

$result = gqlq '{ doogats { id title } }'
if ($result -notmatch '"doogats"') { throw "graphql doogats failed" }
pass "serve: graphql doogats"

$result = gqlq "mutation { deleteDoogat(id: `"$GQL_ID`") }"
if ($result -notmatch "true") { throw "graphql delete failed" }
pass "serve: graphql delete"

# 18b. GraphQL checkbox queries
$result = gqlq '{ openActions { state content } }'
if ($result -notmatch '"openActions"') { throw "graphql openActions failed" }
pass "serve: graphql openActions"

# 18c. GraphQL tag queries
$result = gqlq 'mutation { createDoogat(input: { title: "Tag Test", tags: ["alpha", "beta"] }) { id title tags } }'
if ($result -notmatch '"alpha"') { throw "graphql create with tags failed" }
$TAG_ID = if ($result -match '"id":"([^"]+)"') { $Matches[1] } else { throw "no tag id in response" }
pass "serve: graphql create with tags"

$result = gqlq '{ tags { name count } }'
if ($result -notmatch '"alpha"') { throw "tags query missing alpha" }
if ($result -notmatch '"beta"') { throw "tags query missing beta" }
pass "serve: graphql tags query"

$result = gqlq '{ doogats(tag: "alpha") { id title tags } }'
if ($result -notmatch $TAG_ID) { throw "tag filter missing expected doogat" }
pass "serve: graphql doogats tag filter"

gqlq "mutation { deleteDoogat(id: `"$TAG_ID`") }" | Out-Null

# 18c2. GraphQL updated_at and created_at fields
$ts_result = gqlq 'mutation { createDoogat(input: { title: "Timestamp Test" }) { id } }'
$TS_ID = if ($ts_result -match '"id":"([^"]+)"') { $Matches[1] }
$ts_query = gqlq "{ doogat(id: `"$TS_ID`") { updated_at created_at date } }"
if ($ts_query -notmatch '"updated_at"') { throw "missing updated_at" }
if ($ts_query -notmatch '"created_at"') { throw "missing created_at" }
pass "serve: graphql updated_at and created_at fields"

$ts_date = if ($ts_query -match '"date":"([^"]+)"') { $Matches[1] }
$ts_created = if ($ts_query -match '"created_at":"([^"]+)"') { $Matches[1] }
if ($ts_date -ne $ts_created) { throw "created_at does not equal date" }
pass "serve: created_at equals date"

$ts_search = gqlq '{ search(query: "Timestamp Test") { hits { id updated_at } } }'
if ($ts_search -notmatch '"updated_at"') { throw "search hit missing updated_at" }
pass "serve: search hits include updated_at"

gqlq "mutation { deleteDoogat(id: `"$TS_ID`") }" | Out-Null

# 18d. GraphQL search filters
# Section j1 above registered the `link` typedef with url NOT NULL, so typed
# creates must supply url. SF2 stays untyped since `note` is not a registered
# typedef and PRD 00129 rejects unregistered types from GraphQL createDoogat.
$sf1 = gqlq 'mutation { createDoogat(input: { title: "SearchFilter Alpha", type: "link", tags: ["sf-tag"], fields: "{\"url\":\"https://example.com/sf1\"}" }) { id } }'
$SF1_ID = if ($sf1 -match '"id":"([^"]+)"') { $Matches[1] }
$sf2 = gqlq 'mutation { createDoogat(input: { title: "SearchFilter Beta", tags: ["sf-tag"] }) { id } }'
$SF2_ID = if ($sf2 -match '"id":"([^"]+)"') { $Matches[1] }
$sf3 = gqlq 'mutation { createDoogat(input: { title: "SearchFilter Gamma", type: "link", fields: "{\"url\":\"https://example.com/sf3\"}" }) { id } }'
$SF3_ID = if ($sf3 -match '"id":"([^"]+)"') { $Matches[1] }

$result = gqlq '{ search(query: "SearchFilter", types: ["link"]) { totalCount hits { id } } }'
if ($result -notmatch '"totalCount":2') { throw "search type filter: expected 2, got $result" }
pass "serve: search filter by type"

$result = gqlq '{ search(query: "SearchFilter", tag: "sf-tag") { totalCount hits { id } } }'
if ($result -notmatch '"totalCount":2') { throw "search tag filter: expected 2, got $result" }
pass "serve: search filter by tag"

$result = gqlq '{ search(query: "SearchFilter", types: ["link"], tag: "sf-tag") { totalCount hits { id } } }'
if ($result -notmatch '"totalCount":1') { throw "search combined filter: expected 1, got $result" }
pass "serve: search filter combined type+tag"

gqlq "mutation { deleteDoogat(id: `"$SF1_ID`") }" | Out-Null
gqlq "mutation { deleteDoogat(id: `"$SF2_ID`") }" | Out-Null
gqlq "mutation { deleteDoogat(id: `"$SF3_ID`") }" | Out-Null

# 18d2. Search where field filters (materialized columns + tag)
gqlq 'mutation { executeSql(sql: "CREATE TABLE wflink (url TEXT NOT NULL)") { message } }' | Out-Null
$wf1 = gqlq 'mutation { executeSql(sql: "INSERT INTO wflink (title, url) VALUES (''WFLink Alpha'', ''https://example.com'')") { message } }'
$WF1_ID = if ($wf1 -match '"message":"([^"]+)"') { $Matches[1].Trim() }
$wf2 = gqlq 'mutation { executeSql(sql: "INSERT INTO wflink (title, url) VALUES (''WFLink Beta'', ''https://other.org'')") { message } }'
$WF2_ID = if ($wf2 -match '"message":"([^"]+)"') { $Matches[1].Trim() }
$wf3 = gqlq 'mutation { executeSql(sql: "INSERT INTO wflink (title, url) VALUES (''WFLink Gamma'', ''https://example.com/page'')") { message } }'
$WF3_ID = if ($wf3 -match '"message":"([^"]+)"') { $Matches[1].Trim() }

$result = gqlq '{ search(query: "WFLink", where: [{field: "url", eq: "https://example.com"}]) { totalCount } }'
if ($result -notmatch '"totalCount":1') { throw "search where eq: expected 1, got $result" }
pass "serve: search where filter materialized column eq"

$result = gqlq '{ search(query: "WFLink", where: [{field: "url", contains: "example"}]) { totalCount } }'
if ($result -notmatch '"totalCount":2') { throw "search where contains: expected 2, got $result" }
pass "serve: search where filter materialized column contains"

# Tag via where filter
$wft1 = gqlq 'mutation { createDoogat(input: { title: "WFTag Alpha", tags: ["wf-rust"] }) { id } }'
$WFT1_ID = if ($wft1 -match '"id":"([^"]+)"') { $Matches[1] }
$wft2 = gqlq 'mutation { createDoogat(input: { title: "WFTag Beta", tags: ["wf-python"] }) { id } }'
$WFT2_ID = if ($wft2 -match '"id":"([^"]+)"') { $Matches[1] }

$result = gqlq '{ search(query: "WFTag", where: [{field: "tag", eq: "wf-rust"}]) { totalCount } }'
if ($result -notmatch '"totalCount":1') { throw "search where tag: expected 1, got $result" }
pass "serve: search where filter tag eq"

# Combined type + where field filter
$result = gqlq '{ search(query: "WFLink", types: ["wflink"], where: [{field: "url", eq: "https://example.com"}]) { totalCount } }'
if ($result -notmatch '"totalCount":1') { throw "search where type+field: expected 1, got $result" }
pass "serve: search where filter combined type+field"

gqlq "mutation { deleteDoogat(id: `"$WF1_ID`") }" | Out-Null
gqlq "mutation { deleteDoogat(id: `"$WF2_ID`") }" | Out-Null
gqlq "mutation { deleteDoogat(id: `"$WF3_ID`") }" | Out-Null
gqlq "mutation { deleteDoogat(id: `"$WFT1_ID`") }" | Out-Null
gqlq "mutation { deleteDoogat(id: `"$WFT2_ID`") }" | Out-Null

# 18d3. Search where filter: in operator
$in1 = gqlq 'mutation { createDoogat(input: { title: "InOp Alpha", tags: ["in-rust", "in-systems"] }) { id } }'
$IN1_ID = if ($in1 -match '"id":"([^"]+)"') { $Matches[1] }
$in2 = gqlq 'mutation { createDoogat(input: { title: "InOp Beta", tags: ["in-python"] }) { id } }'
$IN2_ID = if ($in2 -match '"id":"([^"]+)"') { $Matches[1] }
$in3 = gqlq 'mutation { createDoogat(input: { title: "InOp Gamma", tags: ["in-go"] }) { id } }'
$IN3_ID = if ($in3 -match '"id":"([^"]+)"') { $Matches[1] }

# in with multiple values — should match Alpha (in-rust) and Beta (in-python)
$result = gqlq '{ search(query: "InOp", where: [{field: "tag", in: ["in-rust", "in-python"]}]) { totalCount hits { id } } }'
if ($result -notmatch '"totalCount":2') { throw "search where in multi: expected 2, got $result" }
if ($result -notmatch $IN1_ID) { throw "search where in multi: missing Alpha" }
if ($result -notmatch $IN2_ID) { throw "search where in multi: missing Beta" }
pass "serve: search where filter in operator (multiple values)"

# in with single value — should match Gamma only
$result = gqlq '{ search(query: "InOp", where: [{field: "tag", in: ["in-go"]}]) { totalCount } }'
if ($result -notmatch '"totalCount":1') { throw "search where in single: expected 1, got $result" }
pass "serve: search where filter in operator (single value)"

# in with empty array — should match nothing
$result = gqlq '{ search(query: "InOp", where: [{field: "tag", in: []}]) { totalCount } }'
if ($result -notmatch '"totalCount":0') { throw "search where in empty: expected 0, got $result" }
pass "serve: search where filter in operator (empty array)"

# in on materialized column
gqlq 'mutation { executeSql(sql: "CREATE TABLE inlink (url TEXT NOT NULL)") { message } }' | Out-Null
$inl1 = gqlq 'mutation { executeSql(sql: "INSERT INTO inlink (title, url) VALUES (''InLink A'', ''https://a.example.com'')") { message } }'
$INL1_ID = if ($inl1 -match '"message":"([^"]+)"') { $Matches[1].Trim() }
$inl2 = gqlq 'mutation { executeSql(sql: "INSERT INTO inlink (title, url) VALUES (''InLink B'', ''https://b.example.com'')") { message } }'
$INL2_ID = if ($inl2 -match '"message":"([^"]+)"') { $Matches[1].Trim() }
$inl3 = gqlq 'mutation { executeSql(sql: "INSERT INTO inlink (title, url) VALUES (''InLink C'', ''https://c.example.com'')") { message } }'
$INL3_ID = if ($inl3 -match '"message":"([^"]+)"') { $Matches[1].Trim() }

$result = gqlq '{ search(query: "InLink", where: [{field: "url", in: ["https://a.example.com", "https://c.example.com"]}]) { totalCount hits { id } } }'
if ($result -notmatch '"totalCount":2') { throw "search where in materialized: expected 2, got $result" }
if ($result -notmatch $INL1_ID) { throw "search where in materialized: missing A" }
if ($result -notmatch $INL3_ID) { throw "search where in materialized: missing C" }
pass "serve: search where filter in operator (materialized column)"

gqlq "mutation { deleteDoogat(id: `"$IN1_ID`") }" | Out-Null
gqlq "mutation { deleteDoogat(id: `"$IN2_ID`") }" | Out-Null
gqlq "mutation { deleteDoogat(id: `"$IN3_ID`") }" | Out-Null
gqlq "mutation { deleteDoogat(id: `"$INL1_ID`") }" | Out-Null
gqlq "mutation { deleteDoogat(id: `"$INL2_ID`") }" | Out-Null
gqlq "mutation { deleteDoogat(id: `"$INL3_ID`") }" | Out-Null
gqlq 'mutation { executeSql(sql: "DROP TABLE inlink CASCADE") { message } }' | Out-Null

# 18e. Boolean and phrase search queries
$bq1 = gqlq 'mutation { createDoogat(input: { title: "BoolSearch Rust CRDT", content: "rust crdt patterns" }) { id } }'
$BQ1_ID = if ($bq1 -match '"id":"([^"]+)"') { $Matches[1] }
$bq2 = gqlq 'mutation { createDoogat(input: { title: "BoolSearch Rust Only", content: "rust programming" }) { id } }'
$BQ2_ID = if ($bq2 -match '"id":"([^"]+)"') { $Matches[1] }
$bq3 = gqlq 'mutation { createDoogat(input: { title: "BoolSearch Golang", content: "golang programming" }) { id } }'
$BQ3_ID = if ($bq3 -match '"id":"([^"]+)"') { $Matches[1] }

$result = gqlq '{ search(query: "rust AND crdt") { totalCount } }'
if ($result -notmatch '"totalCount":1') { throw "search AND: expected 1, got $result" }
pass "serve: search boolean AND"

$result = gqlq '{ search(query: "rust OR golang") { totalCount } }'
if ($result -notmatch '"totalCount":3') { throw "search OR: expected 3, got $result" }
pass "serve: search boolean OR"

$result = gqlq '{ search(query: "rust NOT crdt") { totalCount } }'
if ($result -notmatch '"totalCount":1') { throw "search NOT: expected 1, got $result" }
pass "serve: search boolean NOT"

$result = gqlq '{ search(query: "\"rust crdt\"") { totalCount } }'
if ($result -notmatch '"totalCount":1') { throw "search phrase: expected 1, got $result" }
pass "serve: search quoted phrase"

$result = try { gqlq '{ search(query: "AND AND") { totalCount } }' } catch { $_.Exception.Message }
if ($result -notmatch 'invalid search query') { throw "search malformed: expected error message, got $result" }
pass "serve: search malformed query returns error"

gqlq "mutation { deleteDoogat(id: `"$BQ1_ID`") }" | Out-Null
gqlq "mutation { deleteDoogat(id: `"$BQ2_ID`") }" | Out-Null
gqlq "mutation { deleteDoogat(id: `"$BQ3_ID`") }" | Out-Null

# 18g. Search query normalization
$result = gqlq '{ normalizeSearchQuery(query: "B AND A") }'
if ($result -notmatch '"a and b"') { throw "normalizeSearchQuery sort: expected 'a and b', got $result" }
pass "serve: normalizeSearchQuery sorts AND operands"

$result = gqlq '{ normalizeSearchQuery(query: "Tag=svelte AND category=work.portals") }'
if ($result -notmatch '"category=work.portals and tag=svelte"') { throw "normalizeSearchQuery fields: unexpected $result" }
pass "serve: normalizeSearchQuery sorts field filters"

$result = gqlq '{ normalizeSearchQuery(query: "  MEETING   Minutes  ") }'
if ($result -notmatch '"meeting and minutes"') { throw "normalizeSearchQuery implicit AND: unexpected $result" }
pass "serve: normalizeSearchQuery implicit AND and lowercase"

$result = gqlq '{ search(query: "rust AND crdt") { queryNormalized } }'
if ($result -notmatch '"queryNormalized"') { throw "search queryNormalized missing: $result" }
if ($result -notmatch '"crdt and rust"') { throw "search queryNormalized value: unexpected $result" }
pass "serve: search returns queryNormalized"

# 18h. In-query field-filter alignment + error-class consistency (PRD 00121)
$prd121a = gqlq 'mutation { createDoogat(input: { title: "PRD121 Alpha", tags: ["prd121-rust"] }) { id } }'
$PRD121A_ID = if ($prd121a -match '"id":"([^"]+)"') { $Matches[1] }
$prd121b = gqlq 'mutation { createDoogat(input: { title: "PRD121 Beta", tags: ["prd121-python"] }) { id } }'
$PRD121B_ID = if ($prd121b -match '"id":"([^"]+)"') { $Matches[1] }
$prd121g = gqlq 'mutation { createDoogat(input: { title: "PRD121 Gamma", tags: ["prd121-rust", "prd121-cli"] }) { id } }'
$PRD121G_ID = if ($prd121g -match '"id":"([^"]+)"') { $Matches[1] }

$result = gqlq '{ search(query: "tag=prd121-rust") { totalCount hits { id } } }'
if ($result -notmatch '"totalCount":2') { throw "search in-query tag: expected 2, got $result" }
pass "serve: search in-query tag filter returns matching set"

$resultInq = gqlq '{ search(query: "tag=prd121-rust") { hits { id } } }'
$resultWhere = gqlq '{ search(query: "", where: [{field: "tag", eq: "prd121-rust"}]) { hits { id } } }'
$idsInq = [regex]::Matches($resultInq, '"id":"([^"]+)"') | ForEach-Object { $_.Groups[1].Value } | Sort-Object
$idsWhere = [regex]::Matches($resultWhere, '"id":"([^"]+)"') | ForEach-Object { $_.Groups[1].Value } | Sort-Object
if (($idsInq -join ',') -ne ($idsWhere -join ',')) {
    throw "in-query vs where mismatch: inq=$($idsInq -join ',') where=$($idsWhere -join ',')"
}
pass "serve: in-query tag filter matches where-arg tag filter"

$result = gqlq '{ search(query: "PRD121 tag=prd121-rust") { totalCount } }'
if ($result -notmatch '"totalCount":2') { throw "search text AND tag: expected 2, got $result" }
pass "serve: search text AND in-query tag filter"

$result = gqlq '{ search(query: "tag=prd121-rust", where: [{field: "tag", eq: "prd121-python"}]) { totalCount } }'
if ($result -notmatch '"totalCount":0') { throw "search intersect filters: expected 0, got $result" }
pass "serve: search in-query + where tag filters intersect (AND)"

$result = try { gqlq '{ search(query: "*") { totalCount } }' } catch { $_.Exception.Message }
if ($result -notmatch 'invalid search query') { throw "bare asterisk: expected invalid search query, got $result" }
if ($result -match 'internal error') { throw "bare asterisk: leaked internal error, got $result" }
pass "serve: search bare asterisk returns bad request (not internal)"

$result = try { gqlq '{ search(query: "**") { totalCount } }' } catch { $_.Exception.Message }
if ($result -notmatch 'invalid search query') { throw "double asterisk: expected invalid search query, got $result" }
if ($result -match 'internal error') { throw "double asterisk: leaked internal error, got $result" }
pass "serve: search double asterisk returns bad request (not internal)"

$result = try { gqlq '{ search(query: "(unbalanced") { totalCount } }' } catch { $_.Exception.Message }
if ($result -notmatch 'invalid search query') { throw "unbalanced paren: expected invalid search query, got $result" }
if ($result -match 'internal error') { throw "unbalanced paren: leaked internal error, got $result" }
pass "serve: search unbalanced paren returns bad request (not internal)"

$result = try { gqlq '{ search(query: "AND") { totalCount } }' } catch { $_.Exception.Message }
if ($result -notmatch 'invalid search query') { throw "bare AND: expected invalid search query, got $result" }
if ($result -match 'internal error') { throw "bare AND: leaked internal error, got $result" }
pass "serve: search bare AND returns bad request (not internal)"

$result = gqlq '{ normalizeSearchQuery(query: "tag=prd121-rust") }'
if ($result -notmatch '"tag=prd121-rust"') { throw "normalizeSearchQuery in-query tag: unexpected $result" }
pass "serve: normalizeSearchQuery preserves in-query tag filter"

$normResult = gqlq '{ normalizeSearchQuery(query: "tag=prd121-rust AND category=work.dev") }'
$normalized = if ($normResult -match '"normalizeSearchQuery":"([^"]+)"') { $Matches[1] }
if ($normalized -ne 'category=work.dev and tag=prd121-rust') {
    throw "normalizeSearchQuery round-trip: unexpected '$normalized'"
}
$result = gqlq "{ search(query: `"$normalized`") { totalCount } }"
if ($result -match 'invalid search query') { throw "round-trip: invalid search query, got $result" }
if ($result -match 'internal error') { throw "round-trip: leaked internal error, got $result" }
pass "serve: search accepts normalized query round-trip"

gqlq "mutation { deleteDoogat(id: `"$PRD121A_ID`") }" | Out-Null
gqlq "mutation { deleteDoogat(id: `"$PRD121B_ID`") }" | Out-Null
gqlq "mutation { deleteDoogat(id: `"$PRD121G_ID`") }" | Out-Null

# 18i. In-query field-filter substring + REFERENCES title resolution (PRD 00133)
ddl 'mutation { executeSql(sql: "CREATE TABLE int133cat (label VARCHAR(100))") { message } }'
ddl 'mutation { executeSql(sql: "CREATE TABLE int133link (url TEXT, int133cat VARCHAR(14) REFERENCES int133cat(id))") { message } }'
$p133Dev = gqlq 'mutation { executeSql(sql: "INSERT INTO int133cat (title, label) VALUES (''Development'', ''dev'')") { message } }'
$P133_DEV_ID = extractId $p133Dev
if (-not $P133_DEV_ID) { throw "18i: failed to seed category row" }
gqlq "mutation { executeSql(sql: `"INSERT INTO int133link (title, url, int133cat) VALUES ('Rust Async', 'https://example.com/rust-async', '$P133_DEV_ID')`") { message } }" | Out-Null
gqlq 'mutation { executeSql(sql: "INSERT INTO int133link (title, url) VALUES (''Meeting Notes Archive'', ''https://example.com/archive'')") { message } }' | Out-Null

$result = gqlq '{ search(query: "title=Archive") { totalCount hits { title } } }'
if ($result -notmatch 'Meeting Notes Archive') { throw "18i: title=Archive did not substring-match: $result" }
pass "serve: in-query title=X does substring match"

$result = gqlq '{ search(query: "int133cat=Development") { totalCount hits { title } } }'
if ($result -notmatch 'Rust Async') { throw "18i: int133cat=Development did not resolve via referenced title: $result" }
pass "serve: in-query <ref_col>=X resolves via referenced typedef title"

$result = gqlq '{ search(query: "", where: [{field: "int133cat", eq: "Development"}]) { totalCount } }'
$count = if ($result -match '"totalCount":(\d+)') { $Matches[1] }
if ($count -ne '0') { throw "18i: explicit where eq should not LIKE-match for REFERENCES, got count=$count" }
pass "serve: explicit where eq stays exact (not LIKE) for REFERENCES"

$result = gqlq "{ search(query: `"`", where: [{field: `"int133cat`", eq: `"$P133_DEV_ID`"}]) { totalCount } }"
$count = if ($result -match '"totalCount":(\d+)') { $Matches[1] }
if ($count -ne '1') { throw "18i: explicit where eq <id> should match exactly, got count=$count" }
pass "serve: explicit where eq matches REFERENCES doogat ID exactly"

ddl 'mutation { executeSql(sql: "DROP TABLE int133link CASCADE") { message } }'
ddl 'mutation { executeSql(sql: "DROP TABLE int133cat CASCADE") { message } }'

# 18z - UPDATE/DELETE WHERE id no-match GraphQL parity (#5 group B).
ddl 'mutation { executeSql(sql: "CREATE TABLE link_b1 (url VARCHAR(255))") { message } }'
$b1Seed = gqlq 'mutation { executeSql(sql: "INSERT INTO link_b1 (title, url) VALUES (''A'', ''https://a.com'')") { message } }'
$B1_SEED_ID = extractId $b1Seed
if (-not $B1_SEED_ID) { throw "18z: failed to seed B1 row" }

$b1 = gqlq 'mutation { executeSql(sql: "UPDATE link_b1 SET title = ''x'' WHERE id = ''does_not_exist_b1''") { affected message } }'
assertGqlOk $b1 "B1"
if ($b1 -notmatch '"affected":0') { throw "18z B1: expected affected=0, got: $b1" }

$b2 = gqlq 'mutation { executeSql(sql: "DELETE FROM link_b1 WHERE id = ''does_not_exist_b2''") { affected message } }'
assertGqlOk $b2 "B2"
if ($b2 -notmatch '"affected":0') { throw "18z B2: expected affected=0, got: $b2" }

$b3 = gqlq 'mutation { executeSql(sql: "UPDATE link_b1 SET title = ''x'' WHERE url = ''https://nope.com''") { affected message } }'
assertGqlOk $b3 "B3"
if ($b3 -notmatch '"affected":0') { throw "18z B3: expected affected=0, got: $b3" }

$b4 = gqlq "mutation { executeSql(sql: `"UPDATE link_b1 SET title = 'x' WHERE id = '$B1_SEED_ID' AND url = 'https://wrong.com'`") { affected message } }"
assertGqlOk $b4 "B4"
if ($b4 -notmatch '"affected":0') { throw "18z B4: expected affected=0, got: $b4" }

$b5 = gqlq "mutation { executeSql(sql: `"UPDATE link_b1 SET title = 'from_in_clause' WHERE id IN ('nope', '$B1_SEED_ID')`") { affected message } }"
assertGqlOk $b5 "B5"
if ($b5 -notmatch '"affected":1') { throw "18z B5: expected affected=1, got: $b5" }

$b5Fast = gqlq "mutation { executeSql(sql: `"UPDATE link_b1 SET title = 'final' WHERE id = '$B1_SEED_ID'`") { affected message } }"
assertGqlOk $b5Fast "B5 fast"
if ($b5Fast -notmatch '"affected":1') { throw "18z B5 fast: expected affected=1, got: $b5Fast" }

gqlq 'mutation { executeSql(sql: "DROP TABLE link_b1 CASCADE") { message } }' | Out-Null
pass "issue-5-B1..B5: UPDATE/DELETE no-match GraphQL parity"

# 18z2 - executeBatch atomicity (#9 F4).
$ver = if ((gqlq '{ schemaVersion }') -match '"schemaVersion":(\d+)') { [int]$Matches[1] } else { 0 }
gqlq 'mutation { executeSql(sql: "CREATE TABLE link_f4 (url VARCHAR(255))") { message } }' | Out-Null
gqlq 'mutation { executeSql(sql: "CREATE TABLE membership_f4 (link_id VARCHAR(255), category VARCHAR(255), UNIQUE(link_id, category))") { message } }' | Out-Null
waitSchemaReload $ver
$f4Link = gqlq 'mutation { executeSql(sql: "INSERT INTO link_f4 (title, url) VALUES (''initial'', ''https://f4.com'')") { message } }'
$F4_LINK_ID = extractId $f4Link
if (-not $F4_LINK_ID) { throw "18z2: could not extract F4_LINK_ID" }

gqlq "mutation { executeSql(sql: `"INSERT INTO membership_f4 (title, link_id, category) VALUES ('m', '$F4_LINK_ID', 'work')`") { message } }" | Out-Null

$f4Batch = gqlq "mutation { executeBatch(statements: [`"UPDATE link_f4 SET title = 'batched' WHERE id = '$F4_LINK_ID' AND url = 'https://f4.com'`", `"INSERT INTO membership_f4 (title, link_id, category) VALUES ('dup', '$F4_LINK_ID', 'work')`"]) { message } }"
assertGqlErrors $f4Batch "F4 batch"
if ($f4Batch -notmatch 'UNIQUE') { throw "18z2: batch error missing UNIQUE marker, got: $f4Batch" }

$f4After = gqlq "{ sql(query: `"SELECT title FROM link_f4 WHERE id = '$F4_LINK_ID'`") { rows } }"
assertGqlOk $f4After "F4 after"
if ($f4After -notmatch 'initial') { throw "18z2: batch was not rolled back, title changed, got: $f4After" }
if ($f4After -match 'batched') { throw "18z2: batch rollback failed, found 'batched' in: $f4After" }

gqlq 'mutation { executeSql(sql: "DROP TABLE link_f4 CASCADE") { message } }' | Out-Null
gqlq 'mutation { executeSql(sql: "DROP TABLE membership_f4 CASCADE") { message } }' | Out-Null
pass "issue-9-F4: executeBatch rolls back all statements when one fails"

# 18z3 - updateDoogat tag semantics (#9 F5/F6/F7).
$f5 = gqlq 'mutation { createDoogat(input: { title: "F5 tag clear", tags: ["a", "b", "c"] }) { id tags } }'
$F5_ID = if ($f5 -match '"id":"([^"]+)"') { $Matches[1] } else { throw "18z3 F5: no id" }
if ($f5 -notmatch '"a"') { throw "18z3 F5: initial tags missing 'a'" }
gqlq "mutation { updateDoogat(input: { id: `"$F5_ID`", tags: [] }) { id } }" | Out-Null
$f5Check = gqlq "{ doogat(id: `"$F5_ID`") { id tags } }"
if ($f5Check -notmatch '"tags":\[\]') { throw "18z3 F5: tags not cleared, got: $f5Check" }
gqlq "mutation { deleteDoogat(id: `"$F5_ID`") }" | Out-Null
pass "issue-9-F5: updateDoogat tags: [] clears all tags"

$f6 = gqlq 'mutation { createDoogat(input: { title: "F6 dedupe" }) { id } }'
$F6_ID = if ($f6 -match '"id":"([^"]+)"') { $Matches[1] } else { throw "18z3 F6: no id" }
gqlq "mutation { updateDoogat(input: { id: `"$F6_ID`", tags: [`"x`", `"y`", `"x`", `"y`", `"x`"] }) { id tags } }" | Out-Null
$f6Check = gqlq "{ doogat(id: `"$F6_ID`") { id tags } }"
$f6XCount = ([regex]::Matches($f6Check, '"x"')).Count
$f6YCount = ([regex]::Matches($f6Check, '"y"')).Count
if ($f6XCount -ne 1) { throw "18z3 F6: expected 1 'x', got $f6XCount in $f6Check" }
if ($f6YCount -ne 1) { throw "18z3 F6: expected 1 'y', got $f6YCount in $f6Check" }
gqlq "mutation { deleteDoogat(id: `"$F6_ID`") }" | Out-Null
pass "issue-9-F6: updateDoogat dedupes input tags"

$f7 = gqlq 'mutation { createDoogat(input: { title: "F7 unicode", tags: ["日本語", "café", "ñoño"] }) { id tags } }'
$F7_ID = if ($f7 -match '"id":"([^"]+)"') { $Matches[1] } else { throw "18z3 F7: no id" }
$f7Check = gqlq "{ doogat(id: `"$F7_ID`") { id tags } }"
if ($f7Check -notmatch '日本語|\\u65e5\\u672c\\u8a9e') { throw "18z3 F7: missing 日本語 in $f7Check" }
if ($f7Check -notmatch 'café|caf\\u00e9') { throw "18z3 F7: missing café in $f7Check" }
if ($f7Check -notmatch 'ñoño|\\u00f1o\\u00f1o') { throw "18z3 F7: missing ñoño in $f7Check" }
gqlq "mutation { deleteDoogat(id: `"$F7_ID`") }" | Out-Null
pass "issue-9-F7: updateDoogat preserves unicode tags"

# 18z4 - SQL feature coverage pins (#9 F9).
ddl 'mutation { executeSql(sql: "CREATE TABLE feat (val INTEGER, label VARCHAR(255), maybe_null VARCHAR(255))") { message } }'
gqlq 'mutation { executeSql(sql: "INSERT INTO feat (title, val, label, maybe_null) VALUES (''r1'', 1, ''a'', ''x'')") { message } }' | Out-Null
gqlq 'mutation { executeSql(sql: "INSERT INTO feat (title, val, label) VALUES (''r2'', 2, ''a'')") { message } }' | Out-Null
gqlq 'mutation { executeSql(sql: "INSERT INTO feat (title, val, label, maybe_null) VALUES (''r3'', 3, ''b'', ''y'')") { message } }' | Out-Null
gqlq 'mutation { executeSql(sql: "INSERT INTO feat (title, val, label) VALUES (''r4'', 4, ''b'')") { message } }' | Out-Null
gqlq 'mutation { executeSql(sql: "INSERT INTO feat (title, val, label, maybe_null) VALUES (''r5'', 5, ''c'', ''z'')") { message } }' | Out-Null

$f9Cnt = gqlq '{ sql(query: "SELECT COUNT(*) FROM feat") { rows } }'
if ($f9Cnt -notmatch '\[\\"5\\"\]') { throw "18z4 F9: COUNT(*) did not return 5: $f9Cnt" }
$f9Grp = gqlq '{ sql(query: "SELECT label, COUNT(*) FROM feat GROUP BY label ORDER BY label") { rows } }'
assertGqlOk $f9Grp "F9 GROUP BY"
foreach ($g in @('\\"a\\"', '\\"b\\"', '\\"c\\"')) {
    if ($f9Grp -notmatch $g) { throw "18z4 F9: GROUP BY missing $g in $f9Grp" }
}
$f9Ord = gqlq '{ sql(query: "SELECT label FROM feat ORDER BY val DESC LIMIT 2") { rows } }'
if ($f9Ord -notmatch '\[\\"c\\"\]') { throw "18z4 F9 ORDER BY: missing [c]: $f9Ord" }
if ($f9Ord -notmatch '\[\\"b\\"\]') { throw "18z4 F9 ORDER BY: missing [b]: $f9Ord" }
if ($f9Ord -match '\[\\"a\\"\]') { throw "18z4 F9 ORDER BY: unexpected [a]: $f9Ord" }
$f9Off = gqlq '{ sql(query: "SELECT val FROM feat ORDER BY val ASC LIMIT 10 OFFSET 3") { rows } }'
if ($f9Off -notmatch '\[\\"4\\"\]') { throw "18z4 F9 OFFSET: missing [4]: $f9Off" }
if ($f9Off -notmatch '\[\\"5\\"\]') { throw "18z4 F9 OFFSET: missing [5]: $f9Off" }
if ($f9Off -match '\[\\"1\\"\]') { throw "18z4 F9 OFFSET: unexpected [1]: $f9Off" }
$f9Nul = gqlq '{ sql(query: "SELECT COUNT(*) FROM feat WHERE maybe_null IS NULL") { rows } }'
if ($f9Nul -notmatch '\[\\"2\\"\]') { throw "18z4 F9 IS NULL: expected 2: $f9Nul" }
$f9Lik = gqlq '{ sql(query: "SELECT COUNT(*) FROM feat WHERE label LIKE \"a%\"") { rows } }'
if ($f9Lik -notmatch '\[\\"2\\"\]') { throw "18z4 F9 LIKE: expected 2: $f9Lik" }
gqlq 'mutation { executeSql(sql: "DROP TABLE feat CASCADE") { message } }' | Out-Null
pass "issue-9-F9: SQL feature coverage (COUNT, GROUP BY, ORDER BY, LIMIT, OFFSET, IS NULL, LIKE)"

# 18z5 - search() limit boundary pins (#9 F10).
$f10a = gqlq 'mutation { createDoogat(input: { title: "F10boundary alpha" }) { id } }'
$F10A_ID = if ($f10a -match '"id":"([^"]+)"') { $Matches[1] } else { throw "F10A no id" }
$f10b = gqlq 'mutation { createDoogat(input: { title: "F10boundary beta" }) { id } }'
$F10B_ID = if ($f10b -match '"id":"([^"]+)"') { $Matches[1] } else { throw "F10B no id" }
$f10c = gqlq 'mutation { createDoogat(input: { title: "F10boundary gamma" }) { id } }'
$F10C_ID = if ($f10c -match '"id":"([^"]+)"') { $Matches[1] } else { throw "F10C no id" }
Start-Sleep -Seconds 1
$f10Zero = gqlq '{ search(query: "F10boundary", limit: 0) { totalCount hits { id } } }'
if ($f10Zero -match 'internal error') { throw "18z5 F10: limit 0 should not surface internal error: $f10Zero" }
$f10Max = gqlq '{ search(query: "F10boundary", limit: 10000) { totalCount hits { id } } }'
assertGqlOk $f10Max "F10 max"
if ($f10Max -notmatch '"totalCount":3') { throw "18z5 F10: limit 10000 should return 3 rows: $f10Max" }
$f10Over = gqlq '{ search(query: "F10boundary", limit: 10001) { totalCount } }'
if ($f10Over -notmatch 'limit must not exceed') { throw "18z5 F10: limit 10001 should error with 'limit must not exceed': $f10Over" }
if ($f10Over -match 'internal error') { throw "18z5 F10: limit 10001 should not surface internal error: $f10Over" }
$f10Neg = gqlq '{ search(query: "F10boundary", limit: -1) { totalCount } }'
if ($f10Neg -match 'internal error') { throw "18z5 F10: limit -1 should not surface internal error: $f10Neg" }
gqlq "mutation { deleteDoogat(id: `"$F10A_ID`") }" | Out-Null
gqlq "mutation { deleteDoogat(id: `"$F10B_ID`") }" | Out-Null
gqlq "mutation { deleteDoogat(id: `"$F10C_ID`") }" | Out-Null
pass "issue-9-F10: search limit boundaries (0, 10000, 10001, -1)"

# 18z6 - ALTER TABLE ADD COLUMN appears in typeDefs introspection (#9 F11).
ddl 'mutation { executeSql(sql: "CREATE TABLE altschema_f11 (a VARCHAR(255))") { message } }'
$f11Before = gqlq '{ typeDefs { name columns { name dataType } } }'
assertGqlOk $f11Before "F11 before"
if ($f11Before -notmatch '"altschema_f11"') { throw "18z6 F11: altschema_f11 missing from typeDefs" }
if ($f11Before -notmatch '"a"') { throw "18z6 F11: column a missing before ALTER" }
ddl 'mutation { executeSql(sql: "ALTER TABLE altschema_f11 ADD COLUMN b INTEGER") { message } }'
$f11After = gqlq '{ typeDefs { name columns { name dataType } } }'
assertGqlOk $f11After "F11 after"
if ($f11After -notmatch '"b"') { throw "18z6 F11: column b missing after ALTER, got: $f11After" }
gqlq 'mutation { executeSql(sql: "DROP TABLE altschema_f11 CASCADE") { message } }' | Out-Null
pass "issue-9-F11: ALTER TABLE ADD COLUMN appears in typeDefs introspection"

# 18z7 - GraphQL schema introspection contract (#9 group G).
$ver = if ((gqlq '{ schemaVersion }') -match '"schemaVersion":(\d+)') { [int]$Matches[1] } else { 0 }
gqlq 'mutation { executeSql(sql: "CREATE TABLE gqtesta (label VARCHAR(255))") { message } }' | Out-Null
gqlq 'mutation { executeSql(sql: "CREATE TABLE gqtestb (label VARCHAR(255))") { message } }' | Out-Null
waitSchemaReload $ver
$gIntro = gqlq '{ __schema { queryType { fields { name } } types { name fields { name } } } }'
assertGqlOk $gIntro "G introspection"
foreach ($fld in @('"name":"gqtestas"', '"name":"gqtestasAggregate"', '"name":"gqtestbs"', '"name":"gqtestbsAggregate"')) {
    if ($gIntro -notmatch [regex]::Escape($fld)) { throw "18z7 G1: missing field $fld" }
}
foreach ($conn in @('GqtestaConnection', 'GqtestbConnection')) {
    if ($gIntro -notmatch $conn) { throw "18z7 G2: missing $conn connection type" }
}
# G2: verify items and totalCount fields on Connection types structurally.
$gIntroObj = $gIntro | ConvertFrom-Json
$gTypes = @{}
foreach ($t in $gIntroObj.data.__schema.types) { $gTypes[$t.name] = $t }
foreach ($conn in @('GqtestaConnection', 'GqtestbConnection')) {
    if (-not $gTypes.ContainsKey($conn)) { throw "18z7 G2: $conn missing from schema types" }
    $fields = @($gTypes[$conn].fields | ForEach-Object { $_.name })
    if ('items' -notin $fields) { throw "18z7 G2: $conn missing items field, got: $($fields -join ',')" }
    if ('totalCount' -notin $fields) { throw "18z7 G2: $conn missing totalCount field, got: $($fields -join ',')" }
}
gqlq 'mutation { executeSql(sql: "DROP TABLE gqtesta CASCADE") { message } }' | Out-Null
gqlq 'mutation { executeSql(sql: "DROP TABLE gqtestb CASCADE") { message } }' | Out-Null
pass "issue-9-G1: every typed table has plural and Aggregate query fields"
pass "issue-9-G2: every Connection type has items and totalCount fields"

# 18z8 - Category + membership jink port (#9 jink full-sweep section 5).
$j3Cat = gqlq 'mutation { executeSql(sql: "INSERT INTO category (title, fqn, space, sort_order) VALUES (''Dev'', ''work.dev'', ''work'', 0)") { message } }'
assertGqlOk $j3Cat "j3 category insert"
$JINK_CAT_ID = extractId $j3Cat
if (-not $JINK_CAT_ID) { throw "18z8: could not extract JINK_CAT_ID" }
pass "j3: INSERT category"

$j3CatSpace = gqlq '{ categories(where: {space: {eq: "work"}}) { items { id fqn title space sort_order } } }'
assertGqlOk $j3CatSpace "j3 category by space"
if ($j3CatSpace -notmatch '"work.dev"') { throw "18z8 j3: missing work.dev in space query" }
pass "j3: categories GraphQL query by space"

$j3CatIn = gqlq '{ categories(where: {fqn: {in: ["work.dev"]}}) { items { fqn title space } } }'
assertGqlOk $j3CatIn "j3 category by fqn IN"
if ($j3CatIn -notmatch '"work.dev"') { throw "18z8 j3: missing work.dev in IN query" }
pass "j3: categories GraphQL query by fqn IN list"

$j3Cm = gqlq "mutation { executeSql(sql: `"INSERT INTO \`"category-membership\`" (title, link_id, category_fqn, sort_order) VALUES ('Test in work.dev', '$JINK_LINK_ID', 'work.dev', COALESCE((SELECT MAX(sort_order) + 1 FROM \`"category-membership\`" WHERE category_fqn = 'work.dev'), 0))`") { message } }"
assertGqlOk $j3Cm "j3 category-membership insert"
if ($j3Cm -notmatch '"message":"\d+"') { throw "18z8 j3: category-membership insert did not return id: $j3Cm" }
pass "j3: INSERT category-membership with COALESCE+subquery sort_order"

$j3CmBoth = gqlq "{ categoryMemberships(where: {link_id: {eq: `"$JINK_LINK_ID`"}, category_fqn: {eq: `"work.dev`"}}) { items { id link_id category_fqn pinned sort_order } } }"
assertGqlOk $j3CmBoth "j3 categoryMemberships both"
if ($j3CmBoth -notmatch '"work.dev"') { throw "18z8 j3: categoryMemberships by both missing work.dev" }
pass "j3: categoryMemberships by link_id + category_fqn"

$j3CmLink = gqlq "{ categoryMemberships(where: {link_id: {eq: `"$JINK_LINK_ID`"}}) { items { category_fqn } } }"
assertGqlOk $j3CmLink "j3 categoryMemberships by link_id"
if ($j3CmLink -notmatch '"work.dev"') { throw "18z8 j3: categoryMemberships by link_id missing work.dev" }
pass "j3: categoryMemberships by link_id only"

$j3CmCat = gqlq '{ categoryMemberships(where: {category_fqn: {eq: "work.dev"}}) { items { link_id pinned sort_order } } }'
assertGqlOk $j3CmCat "j3 categoryMemberships by category"
if ($j3CmCat -notmatch $JINK_LINK_ID) { throw "18z8 j3: categoryMemberships by category missing JINK_LINK_ID" }
pass "j3: categoryMemberships by category_fqn only"

# 18z9 - Jink quotes + saved-searches + pinned-results + jinkConfigs port
# (#9 jink full-sweep sections 10-12).
$j4Q = gqlq 'mutation { executeSql(sql: "INSERT INTO quote (title, author, text) VALUES (''First'', ''Anon'', ''Hello world'')") { message } }'
assertGqlOk $j4Q "j4 quote insert"
$JINK_QUOTE_ID = extractId $j4Q
if (-not $JINK_QUOTE_ID) { throw "18z9: could not extract JINK_QUOTE_ID" }
pass "j4: INSERT quote"

$j4QId = gqlq "{ quotes(where: {id: {eq: `"$JINK_QUOTE_ID`"}}) { items { id title text author } } }"
assertGqlOk $j4QId "j4 quotes by id"
if ($j4QId -notmatch 'Hello world') { throw "18z9 j4: quote query missing 'Hello world'" }
pass "j4: quotes query by id"

$j4QAll = gqlq '{ quotes { items { id } } }'
assertGqlOk $j4QAll "j4 quotes all"
if ($j4QAll -notmatch '"quotes"') { throw "18z9 j4: quotes query missing quotes key" }
pass "j4: quotes query all"

$j4QUpd = gqlq "mutation { executeSql(sql: `"UPDATE quote SET favorited = 'true' WHERE id = '$JINK_QUOTE_ID' AND title = 'First'`") { affected } }"
assertGqlOk $j4QUpd "j4 quote update"
if ($j4QUpd -notmatch '"affected":1') { throw "18z9 j4: quote update did not affect 1 row: $j4QUpd" }
pass "j4: UPDATE quote SET favorited (compound predicate)"

$j4Ss = gqlq 'mutation { executeSql(sql: "INSERT INTO \"saved-search\" (title, query_raw, query_normalized) VALUES (''rust stuff'', ''Rust'', ''rust'')") { message } }'
assertGqlOk $j4Ss "j4 saved-search insert"
$JINK_SS_ID = extractId $j4Ss
if (-not $JINK_SS_ID) { throw "18z9: could not extract JINK_SS_ID" }
pass "j4: INSERT saved-search"

$j4SsQ = gqlq "{ savedSearches(where: {id: {eq: `"$JINK_SS_ID`"}}) { items { id title query_raw query_normalized } } }"
assertGqlOk $j4SsQ "j4 savedSearches query"
if ($j4SsQ -notmatch '"rust"') { throw "18z9 j4: savedSearches query missing rust" }
pass "j4: savedSearches query by id"

$j4Pr = gqlq "mutation { executeSql(sql: `"INSERT INTO \`"pinned-result\`" (title, query_normalized, link_id, sort_order) VALUES ('pinned test', 'rust', '$JINK_LINK_ID', 0)`") { message } }"
assertGqlOk $j4Pr "j4 pinned-result insert"
if ($j4Pr -notmatch '"message":"\d+"') { throw "18z9 j4: pinned-result insert did not return id: $j4Pr" }
pass "j4: INSERT pinned-result"

$j4PrQ = gqlq '{ pinnedResults(where: {query_normalized: {eq: "rust"}}) { items { id query_normalized link_id sort_order } } }'
assertGqlOk $j4PrQ "j4 pinnedResults query"
if ($j4PrQ -notmatch $JINK_LINK_ID) { throw "18z9 j4: pinnedResults query missing JINK_LINK_ID" }
pass "j4: pinnedResults query by query_normalized"

$j4Jc = gqlq '{ jinkConfigs { items { id dashboard_title quote_rotation_minutes links_per_category frontend_version } } }'
assertGqlOk $j4Jc "j4 jinkConfigs query"
if ($j4Jc -notmatch 'dashboard_title') { throw "18z9 j4: jinkConfigs missing dashboard_title" }
pass "j4: jinkConfigs query"

$j4JcUpd = gqlq 'mutation { executeSql(sql: "UPDATE \"jink-config\" SET frontend_version = ''1.0.0'' WHERE title = ''jink-config''") { affected } }'
assertGqlOk $j4JcUpd "j4 jink-config update"
if ($j4JcUpd -notmatch '"affected":1') { throw "18z9 j4: jink-config update did not affect 1: $j4JcUpd" }
pass "j4: UPDATE jink-config frontend_version (compound predicate)"

$j4JcSel = gqlq '{ sql(query: "SELECT frontend_version FROM \"jink-config\" LIMIT 1") { rows } }'
assertGqlOk $j4JcSel "j4 jink-config select"
if ($j4JcSel -notmatch '\\"1.0.0\\"') { throw "18z9 j4: jink-config SELECT did not return 1.0.0: $j4JcSel" }
pass "j4: SELECT frontend_version returns 1.0.0"

# 18z10 - composite UNIQUE duplicate + compound-predicate DELETE jink port
# (#9 jink full-sweep sections 6 + 13). Also drops all jink tables.
$j5Dup = gqlq "mutation { executeSql(sql: `"INSERT INTO \`"category-membership\`" (title, link_id, category_fqn) VALUES ('dup', '$JINK_LINK_ID', 'work.dev')`") { message } }"
assertGqlErrors $j5Dup "j5 duplicate"
if ($j5Dup -notmatch 'UNIQUE') { throw "18z10 j5: duplicate should mention UNIQUE: $j5Dup" }
pass "j5: duplicate category-membership rejected with UNIQUE error"

$j5Batch = gqlq "mutation { executeBatch(statements: [`"DELETE FROM \`"category-membership\`" WHERE link_id = '$JINK_LINK_ID' AND category_fqn = 'work.dev'`", `"DELETE FROM link WHERE id = '$JINK_LINK_ID' AND url = 'https://example.com'`"]) { message } }"
assertGqlOk $j5Batch "j5 batch delete"
if ($j5Batch -notmatch '"executeBatch"') { throw "18z10 j5: batch delete missing executeBatch key: $j5Batch" }
pass "j5: executeBatch DELETE category-membership + link (compound predicates)"

$j5LinkGone = gqlq "{ links(where: {id: {eq: `"$JINK_LINK_ID`"}}) { items { id } } }"
assertGqlOk $j5LinkGone "j5 link gone"
if ($j5LinkGone -notmatch '"items":\[\]') { throw "18z10 j5: link not gone after batch delete: $j5LinkGone" }
pass "j5: link is gone after batch delete"

$j5QDel = gqlq "mutation { executeSql(sql: `"DELETE FROM quote WHERE id = '$JINK_QUOTE_ID' AND title = 'First'`") { affected } }"
assertGqlOk $j5QDel "j5 quote delete"
if ($j5QDel -notmatch '"affected":1') { throw "18z10 j5: quote delete did not affect 1: $j5QDel" }
pass "j5: DELETE quote (compound predicate)"

# Final cleanup: drop all jink tables from sub-block J1.
gqlq 'mutation { executeSql(sql: "DROP TABLE link CASCADE") { message } }' | Out-Null
gqlq 'mutation { executeSql(sql: "DROP TABLE category CASCADE") { message } }' | Out-Null
gqlq 'mutation { executeSql(sql: "DROP TABLE \"category-membership\" CASCADE") { message } }' | Out-Null
gqlq 'mutation { executeSql(sql: "DROP TABLE quote CASCADE") { message } }' | Out-Null
gqlq 'mutation { executeSql(sql: "DROP TABLE \"saved-search\" CASCADE") { message } }' | Out-Null
gqlq 'mutation { executeSql(sql: "DROP TABLE \"pinned-result\" CASCADE") { message } }' | Out-Null
gqlq 'mutation { executeSql(sql: "DROP TABLE \"jink-config\" CASCADE") { message } }' | Out-Null
pass "j5: jink port cleanup (all tables dropped)"

# 19. REST API CRUD
try {
    Invoke-WebRequest -Uri "$REST_URL/doogats" -Method POST -ContentType "application/json" `
        -Body '{"title":"REST No Auth"}' -ErrorAction Stop
    throw "should have been 401"
} catch {
    if ($_.Exception.Response.StatusCode.value__ -ne 401) { throw "expected 401" }
}
pass "rest: auth rejects missing token"

$resp = rest "/doogats" "POST" '{"title":"REST Smoke","body":"rest body","tags":["rest"]}'
if ($resp.StatusCode -ne 201) { throw "rest create expected 201" }
$REST_ID = if ((content $resp) -match '"id":"([^"]+)"') { $Matches[1] } else { throw "no id" }
pass "rest: create"

$resp = rest "/doogats/$REST_ID"
if ((content $resp) -notmatch "REST Smoke") { throw "rest get failed" }
pass "rest: get"

$resp = rest "/doogats/$REST_ID" "PUT" '{"title":"REST Updated"}'
if ((content $resp) -notmatch "REST Updated") { throw "rest update failed" }
pass "rest: update"

$resp = rest "/doogats?tag=rest"
if ((content $resp) -notmatch $REST_ID) { throw "rest list failed" }
pass "rest: list with filter"

# Field filtering
gqlq 'mutation{executeSql(sql:"CREATE TABLE smokeitem (label TEXT NOT NULL, priority INTEGER)"){message}}'
gqlq 'mutation{executeSql(sql:"INSERT INTO smokeitem (label, priority) VALUES (''Smoke1'', 7)"){message}}'
$resp = rest "/doogats?field.priority=7"
if ((content $resp) -notmatch "Smoke1") { throw "field filter failed" }
pass "rest: field filter"

$resp = rest "/doogats?field.priority=999"
if ((content $resp) -notmatch '"data":\[\]') { throw "field filter no-match failed" }
pass "rest: field filter no match"

# REST sort: create doogats with distinct titles
$resp = rest "/doogats" "POST" '{"title":"Charlie Sort","tags":["sorttest"]}'
$SORT_A = if ((content $resp) -match '"id":"([^"]+)"') { $Matches[1] } else { throw "no id" }
Start-Sleep -Milliseconds 100
$resp = rest "/doogats" "POST" '{"title":"Alpha Sort","tags":["sorttest"]}'
$SORT_B = if ((content $resp) -match '"id":"([^"]+)"') { $Matches[1] } else { throw "no id" }
Start-Sleep -Milliseconds 100
$resp = rest "/doogats" "POST" '{"title":"Bravo Sort","tags":["sorttest"]}'
$SORT_C = if ((content $resp) -match '"id":"([^"]+)"') { $Matches[1] } else { throw "no id" }

$resp = rest "/doogats?tag=sorttest&sort=title"
$c = content $resp
if ($c -notmatch '"data":\[\{[^}]*"title":"([^"]*)"') { throw "sort title parse" }
if ($Matches[1] -ne "Alpha Sort") { throw "sort title asc: got $($Matches[1])" }
pass "rest: sort by title ascending"

$resp = rest "/doogats?tag=sorttest&sort=-title"
$c = content $resp
if ($c -notmatch '"data":\[\{[^}]*"title":"([^"]*)"') { throw "sort -title parse" }
if ($Matches[1] -ne "Charlie Sort") { throw "sort title desc: got $($Matches[1])" }
pass "rest: sort by title descending"

$resp = rest "/doogats?tag=sorttest&sort=date"
$c = content $resp
if ($c -notmatch '"data":\[\{[^}]*"id":"([^"]*)"') { throw "sort date parse" }
if ($Matches[1] -ne $SORT_C) { throw "sort date desc: expected $SORT_C, got $($Matches[1])" }
pass "rest: sort by date descending (default)"

try {
    Invoke-WebRequest -Uri "$REST_URL/doogats?sort=invalid" -Method GET `
        -Headers @{ Authorization = "Bearer $TOKEN" } -ErrorAction Stop
    throw "should have been 400"
} catch {
    if ($_.Exception.Response.StatusCode.value__ -ne 400) { throw "expected 400, got $($_.Exception.Response.StatusCode.value__)" }
}
pass "rest: sort invalid field returns 400"

# Clean up sort test doogats
Invoke-WebRequest -Uri "$REST_URL/doogats/$SORT_A" -Method DELETE -Headers @{ Authorization = "Bearer $TOKEN" } -ErrorAction SilentlyContinue | Out-Null
Invoke-WebRequest -Uri "$REST_URL/doogats/$SORT_B" -Method DELETE -Headers @{ Authorization = "Bearer $TOKEN" } -ErrorAction SilentlyContinue | Out-Null
Invoke-WebRequest -Uri "$REST_URL/doogats/$SORT_C" -Method DELETE -Headers @{ Authorization = "Bearer $TOKEN" } -ErrorAction SilentlyContinue | Out-Null

$resp = Invoke-WebRequest -Uri "$REST_URL/doogats/$REST_ID" -Method DELETE `
    -Headers @{ Authorization = "Bearer $TOKEN" } -ErrorAction Stop
if ($resp.StatusCode -ne 204) { throw "rest delete expected 204" }
pass "rest: delete"

try {
    Invoke-WebRequest -Uri "$REST_URL/doogats/$REST_ID" -Method GET `
        -Headers @{ Authorization = "Bearer $TOKEN" } -ErrorAction Stop
    throw "should have been 404"
} catch {
    if ($_.Exception.Response.StatusCode.value__ -ne 404) { throw "expected 404" }
}
pass "rest: get after delete returns 404"

# 20. PgWire — skip on Windows (psql rarely available)
pass "pgwire: skipped (windows)"

# NoSQL server endpoints
$NOSQL_URL = "http://127.0.0.1:$SERVER_PORT/nosql"
function nosql($path) {
    $resp = Invoke-WebRequest -Uri "$NOSQL_URL$path" `
        -Headers @{ Authorization = "Bearer $TOKEN" } -ErrorAction Stop
    if ($resp.Content -is [byte[]]) { return [System.Text.Encoding]::UTF8.GetString($resp.Content) }
    return $resp.Content
}

$result = nosql "/$ID1"
if ($result -notmatch "First note") { throw "nosql get failed" }
pass "nosql-api: get by id"

$result = nosql "?tag=smoke"
if ($result -notmatch $ID1) { throw "nosql scan failed" }
pass "nosql-api: scan by tag"

try {
    Invoke-WebRequest -Uri "$NOSQL_URL`?type=project&tag=test" `
        -Headers @{ Authorization = "Bearer $TOKEN" } -ErrorAction Stop
    throw "should have been 400"
} catch {
    if ($_.Exception.Response.StatusCode.value__ -ne 400) { throw "expected 400" }
}
pass "nosql-api: rejects both type and tag"

try {
    Invoke-WebRequest -Uri "$NOSQL_URL/$ID1" -Method GET -ContentType "application/json" -ErrorAction Stop
    throw "should have been 401"
} catch {
    if ($_.Exception.Response.StatusCode.value__ -ne 401) { throw "expected 401" }
}
pass "nosql-api: auth rejects missing token"

# error response — SQL engine errors are descriptive (user-actionable)
$result = gqlq 'mutation { executeSql(sql: "SELCT * FORM oops") { message } }'
if ($result -notmatch "errors") { throw "expected errors in response" }
if ($result -notmatch "(?i)parse:") { throw "expected descriptive parse error" }
pass "serve: sql engine error is descriptive"

# error sanitization — not-found returns 404
try {
    Invoke-WebRequest -Uri "$REST_URL/doogats/99990101000000" `
        -Headers @{ Authorization = "Bearer $TOKEN" } -ErrorAction Stop
    throw "should have been 404"
} catch {
    if ($_.Exception.Response.StatusCode.value__ -ne 404) { throw "expected 404" }
}
pass "serve: not-found returns 404"

# GraphQL introspection hides internal tables
$intro = gqlq '{ __schema { queryType { fields { name } } } }'
if ($intro -match "_ddb_") { throw "introspection leaked internal table: $intro" }
pass "serve: introspection hides internal tables"

# compact mutation
$result = gqlq 'mutation { compact { filesRemoved crdtDocsCompacted gcSuccess crdtTempBytesBefore crdtTempBytesAfter crdtTempFilesBefore crdtTempFilesAfter repoBytesBefore repoBytesAfter backupPath } }'
if ($result -notmatch "gcSuccess") { throw "compact mutation failed" }
pass "serve: compact mutation"

# compact(force: true)
$result = gqlq 'mutation { compact(force: true) { filesRemoved crdtDocsCompacted gcSuccess crdtTempBytesBefore crdtTempBytesAfter repoBytesBefore repoBytesAfter backupPath } }'
if ($result -notmatch "gcSuccess") { throw "compact(force:true) mutation failed" }
if ($result -notmatch "backupPath") { throw "compact(force:true) missing backupPath" }
pass "serve: compact(force: true) mutation"

# compact(noBackup: true)
$result = gqlq 'mutation { compact(force: true, noBackup: true) { gcSuccess backupPath } }'
if ($result -notmatch "gcSuccess") { throw "compact(noBackup:true) failed" }
if ($result -notmatch '"backupPath":null') { throw "compact(noBackup:true) should have null backupPath" }
pass "serve: compact(noBackup: true) mutation"

# compact(backupPath: custom)
$gqlBackup = Join-Path $env:TEMP "gql-backup.bundle.tar"
$gqlBackupEsc = $gqlBackup -replace '\\', '\\\\'
$result = gqlq "mutation { compact(force: true, backupPath: `"$gqlBackupEsc`") { gcSuccess backupPath } }"
if ($result -notmatch "gcSuccess") { throw "compact(backupPath) failed" }
if ($result -notmatch "backupPath") { throw "compact(backupPath) missing backupPath" }
if (-not (Test-Path $gqlBackup)) { throw "compact(backupPath) file not created" }
pass "serve: compact(backupPath) mutation"

# maintenance mutation
$result = gqlq 'mutation { maintenance { success durationMs fallbackUsed tasksRun } }'
if ($result -notmatch "success") { throw "maintenance mutation failed" }
pass "serve: maintenance mutation"

# sync mutation — no remote configured, expect error not panic
$result = gqlq 'mutation { sync { direction commitsTransferred conflictsResolved resurrected } }'
if ($result -notmatch "errors") { throw "sync should have errored without remote" }
pass "serve: sync mutation (no remote)"

# 37. WebSocket payload auth (browser-style, no Authorization header)
# Skipped in PowerShell — requires websocat or native WebSocket client.
# Full coverage provided by e2e tests (ws_payload_auth_subscribe_receive etc.)
pass "ws: payload auth (skipped in ps1 — see e2e tests)"

# 38. read-under-write: concurrent read + write
$writeJob = Start-Job -ScriptBlock {
    param($url, $token)
    $headers = @{ "Authorization" = "Bearer $token"; "Content-Type" = "application/json" }
    Invoke-RestMethod -Uri $url -Method Post -Headers $headers -Body '{"query":"mutation { createDoogat(input: { title: \"ReadPoolWrite\" }) { id } }"}'
} -ArgumentList $GQL_URL, $TOKEN
$readResult = gqlq '{ doogats { id title } }'
if ($readResult -notmatch "doogats") { throw "read-under-write: read failed" }
$writeResult = Receive-Job -Job $writeJob -Wait | ConvertTo-Json
if ($writeResult -notmatch "id") { throw "read-under-write: write failed" }
Remove-Job $writeJob
pass "serve: read-under-write (concurrent read + write)"

# 38b. multi-value references via GraphQL + REST
$ver = if ((gqlq '{ schemaVersion }') -match '"schemaVersion":(\d+)') { [int]$Matches[1] } else { 0 }
gqlq 'mutation{executeSql(sql:"CREATE TABLE mvcategory (name VARCHAR(100))"){message}}' | Out-Null
gqlq 'mutation{executeSql(sql:"CREATE TABLE mvbookmark (mvcategory TEXT REFERENCES mvcategory)"){message}}' | Out-Null
waitSchemaReload $ver
$mvCat1 = (gqlq 'mutation{executeSql(sql:"INSERT INTO mvcategory (name) VALUES (''Science'')"){message}}') -replace '.*"message":"(\d+)".*','$1'
$mvCat2 = (gqlq 'mutation{executeSql(sql:"INSERT INTO mvcategory (name) VALUES (''Math'')"){message}}') -replace '.*"message":"(\d+)".*','$1'
$mvBm = (gqlq "mutation{executeSql(sql:`"INSERT INTO mvbookmark (mvcategory) VALUES ('$mvCat1')`"){message}}") -replace '.*"message":"(\d+)".*','$1'
gqlq "mutation{executeSql(sql:`"INSERT INTO mvbookmark_mvcategory (mvbookmark_id, mvcategory_id) VALUES ('$mvBm', '$mvCat2')`"){message}}" | Out-Null
$mvResult = gqlq '{ mvbookmarks { items { id mvcategories { id } } } }'
if ($mvResult -notmatch $mvCat1) { throw "multi-value ref: cat1 not in graphql list field" }
if ($mvResult -notmatch $mvCat2) { throw "multi-value ref: cat2 not in graphql list field" }
pass "serve: graphql multi-value ref list field"
$mvRest = rest "/doogats/$mvBm"
if ($mvRest -notmatch '"references"') { throw "multi-value ref: no references in rest json" }
if ($mvRest -notmatch '"mvcategory"') { throw "multi-value ref: no category key in references" }
pass "serve: rest multi-value ref structured json"

# 38b2. REFERENCES relation resolution
$ver = if ((gqlq '{ schemaVersion }') -match '"schemaVersion":(\d+)') { [int]$Matches[1] } else { 0 }
gqlq 'mutation { executeSql(sql: "CREATE TABLE smokecat (label TEXT)") { message } }' | Out-Null
gqlq 'mutation { executeSql(sql: "CREATE TABLE smokebm (url TEXT, smokecat TEXT REFERENCES smokecat)") { message } }' | Out-Null
waitSchemaReload $ver
$scat = gqlq "mutation { executeSql(sql: `"INSERT INTO smokecat (title, label) VALUES ('Tech', 'tech')`") { message } }"
$SCAT_ID = if ($scat -match '"message":"([^"]+)"') { $Matches[1] }
$sbm = gqlq "mutation { executeSql(sql: `"INSERT INTO smokebm (title, url) VALUES ('Example', 'https://example.com')`") { message } }"
$SBM_ID = if ($sbm -match '"message":"([^"]+)"') { $Matches[1] }
gqlq "mutation { executeSql(sql: `"INSERT INTO smokebm_smokecat (smokebm_id, smokecat_id) VALUES ('$SBM_ID', '$SCAT_ID')`") { message } }" | Out-Null
$result = gqlq '{ smokebms { items { smokecat { id label } } } }'
if ($result -notmatch '"label":"tech"') { throw "singular relation resolution failed: $result" }
pass "serve: relation singular resolves object"
$result = gqlq '{ smokebms { items { smokecats { id label } } } }'
if ($result -notmatch '"label":"tech"') { throw "plural relation resolution failed: $result" }
pass "serve: relation plural resolves object list"
Start-Sleep -Seconds 1
gqlq "mutation { executeSql(sql: `"INSERT INTO smokebm (title, url) VALUES ('No Cat', 'https://nocat.com')`") { message } }" | Out-Null
$result = gqlq '{ smokebms { items { id smokecat { id } smokecats { id } } } }'
if ($result -notmatch '"smokecat":null') { throw "null relation failed: $result" }
if ($result -notmatch '"smokecats":\[\]') { throw "empty plural relation failed: $result" }
pass "serve: relation null returns null and empty list"
gqlq 'mutation { executeSql(sql: "DROP TABLE smokebm CASCADE") { message } }' | Out-Null
gqlq 'mutation { executeSql(sql: "DROP TABLE smokecat CASCADE") { message } }' | Out-Null

# 38b2b. raw ID scalar + orderBy/limit on plural references
$ver = if ((gqlq '{ schemaVersion }') -match '"schemaVersion":(\d+)') { [int]$Matches[1] } else { 0 }
gqlq 'mutation { executeSql(sql: "CREATE TABLE rlcat (label TEXT)") { message } }' | Out-Null
gqlq 'mutation { executeSql(sql: "CREATE TABLE rlbm (url TEXT, rlcat TEXT REFERENCES rlcat)") { message } }' | Out-Null
waitSchemaReload $ver
$rlC1 = (gqlq 'mutation { executeSql(sql: "INSERT INTO rlcat (label) VALUES (\"cherry\")") { message } }') -replace '.*"message":"([^"]+)".*','$1'
$rlC2 = (gqlq 'mutation { executeSql(sql: "INSERT INTO rlcat (label) VALUES (\"apple\")") { message } }') -replace '.*"message":"([^"]+)".*','$1'
Start-Sleep -Seconds 1
$rlC3 = (gqlq 'mutation { executeSql(sql: "INSERT INTO rlcat (label) VALUES (\"banana\")") { message } }') -replace '.*"message":"([^"]+)".*','$1'
Start-Sleep -Seconds 1
$rlBm = (gqlq 'mutation { executeSql(sql: "INSERT INTO rlbm (url) VALUES (\"https://example.com\")") { message } }') -replace '.*"message":"([^"]+)".*','$1'
foreach ($cid in @($rlC1, $rlC2, $rlC3)) {
    gqlq "mutation { executeSql(sql: `"INSERT INTO rlbm_rlcat (rlbm_id, rlcat_id) VALUES ('$rlBm', '$cid')`") { message } }" | Out-Null
}
# raw ID scalar
$result = gqlq '{ rlbms { items { rlcat_id rlcat { id label } } } }'
if ($result -notmatch "rlcat_id`":`"$rlC1") { throw "raw ID scalar missing: $result" }
if ($result -notmatch '"label":"cherry"') { throw "object resolver missing label: $result" }
pass "serve: relation raw ID scalar coexists with object resolver"
# orderBy ASC
$result = gqlq '{ rlbms { items { rlcats(orderBy: "label") { label } } } }'
if ($result -notmatch '"apple".*"banana".*"cherry"') { throw "orderBy ASC wrong: $result" }
pass "serve: relation plural orderBy ASC"
# orderBy DESC + limit
$result = gqlq '{ rlbms { items { rlcats(orderBy: "label", orderDir: "DESC", limit: 2) { label } } } }'
if ($result -notmatch '"cherry".*"banana"') { throw "orderBy DESC wrong: $result" }
if ($result -match '"apple"') { throw "limit not applied: $result" }
pass "serve: relation plural orderBy DESC + limit"
gqlq 'mutation { executeSql(sql: "DROP TABLE rlbm CASCADE") { message } }' | Out-Null
gqlq 'mutation { executeSql(sql: "DROP TABLE rlcat CASCADE") { message } }' | Out-Null

# 38b3. typed connection includes tags
gqlq 'mutation { executeSql(sql: "CREATE TABLE tagarticle (topic TEXT)") { message } }' | Out-Null
$taId = (gqlq 'mutation { executeSql(sql: "INSERT INTO tagarticle (topic) VALUES (\"rust\")") { message } }') -replace '.*"message":"(\d+)".*','$1'
gqlq "mutation { updateDoogat(input: { id: `"$taId`", tags: [`"coding`", `"systems`"] }) { id } }" | Out-Null
$result = gqlq '{ tagarticles { items { id tags topic } } }'
if ($result -notmatch '"coding"') { throw "typed connection tags: missing coding tag: $result" }
if ($result -notmatch '"systems"') { throw "typed connection tags: missing systems tag: $result" }
pass "serve: typed connection includes tags"
gqlq 'mutation { executeSql(sql: "DROP TABLE tagarticle CASCADE") { message } }' | Out-Null

# 38b4. tagEntries query with filters
$te1 = (gqlq 'mutation { createDoogat(input: { title: "TagEntry A", tags: ["te-rust", "te-cli"] }) { id } }') -replace '.*"id":"(\d+)".*','$1'
Start-Sleep -Seconds 1
$te2 = (gqlq 'mutation { createDoogat(input: { title: "TagEntry B", tags: ["te-rust"] }) { id } }') -replace '.*"id":"(\d+)".*','$1'
$result = gqlq "{ tagEntries(where: { doogatId: { eq: `"$te1`" } }) { items { doogatId tag } totalCount } }"
if ($result -notmatch '"totalCount":2') { throw "tagEntries doogatId eq: expected 2, got: $result" }
pass "serve: tagEntries filter by doogatId eq"

$result = gqlq '{ tagEntries(where: { tag: { eq: "te-rust" } }) { items { tag } totalCount } }'
if ($result -notmatch '"te-rust"') { throw "tagEntries tag eq: missing te-rust: $result" }
$teCount = if ($result -match '"totalCount":(\d+)') { [int]$Matches[1] } else { 0 }
if ($teCount -lt 2) { throw "tagEntries tag eq: expected >=2, got $teCount" }
pass "serve: tagEntries filter by tag eq"

$result = gqlq '{ tagEntries(where: { tag: { contains: "te-" } }) { totalCount } }'
$teCount = if ($result -match '"totalCount":(\d+)') { [int]$Matches[1] } else { 0 }
if ($teCount -lt 3) { throw "tagEntries tag contains: expected >=3, got $teCount" }
pass "serve: tagEntries filter by tag contains"

gqlq "mutation { deleteDoogat(id: `"$te1`") { id } }" | Out-Null
gqlq "mutation { deleteDoogat(id: `"$te2`") { id } }" | Out-Null

# 38c. sql-materialization (columns, boolean normalization, core fields)
$result = gqlq '{ sql(query: "SELECT id, title FROM doogats") { columns rows } }'
if ($result -notmatch '"columns"') { throw "sql columns: missing columns field" }
if ($result -notmatch '"id"') { throw "sql columns: missing id column" }
if ($result -notmatch '"title"') { throw "sql columns: missing title column" }
pass "serve: sql columns in response"

# 38c2. sql format:objects returns keyed rows
$result = gqlq '{ sql(query: "SELECT id, title FROM doogats", format: "objects") { columns rows } }'
if ($result -notmatch '\\"id\\":') { throw "sql format objects: missing id key in row object" }
if ($result -notmatch '\\"title\\":') { throw "sql format objects: missing title key in row object" }
pass "serve: sql format objects returns keyed rows"

ddl 'mutation{executeSql(sql:"CREATE TABLE smokepin (pinned BOOLEAN)"){message}}'
$smokepinId = (gqlq "mutation{executeSql(sql:`"INSERT INTO smokepin (title, pinned) VALUES ('PinTest', true)`"){message}}") -replace '.*"message":"(\d+)".*','$1'
if (-not $smokepinId) { throw "smokepin insert failed" }
$result = gqlq "{ sql(query: `"SELECT pinned FROM smokepin WHERE pinned = 1`") { rows } }"
if ($result -notmatch '[\\"]true[\\"]') { throw "boolean not coerced to true" }
pass "serve: boolean coerced to true/false"

# Boolean false
Start-Sleep -Seconds 1
gqlq "mutation{executeSql(sql:`"INSERT INTO smokepin (title, pinned) VALUES ('FalseTest', false)`"){message}}" | Out-Null
$result = gqlq "{ sql(query: `"SELECT pinned FROM smokepin WHERE pinned = 0`") { rows } }"
if ($result -notmatch '[\\"]false[\\"]') { throw "boolean false not coerced" }
pass "serve: boolean false coerced"

$result = gqlq '{ sql(query: "SELECT title FROM smokepin") { rows } }'
if ($result -notmatch 'PinTest') { throw "core fields: title missing from type table" }
pass "serve: core fields in type table"

# 38d. DISTINCT on typed connection queries
gqlq "mutation{executeSql(sql:`"INSERT INTO foo (title, bar, baz) VALUES ('dup1', 'val', 2)`"){message}}" | Out-Null
gqlq "mutation{executeSql(sql:`"INSERT INTO foo (title, bar, baz) VALUES ('uniq', 'other', 3)`"){message}}" | Out-Null
Start-Sleep -Seconds 1
$result = gqlq '{ foos(distinct: "bar") { items { bar } totalCount } }'
if ($result -notmatch '"totalCount":2') { throw "distinct totalCount: $result" }
pass "serve: distinct deduplicates and totalCount reflects unique count"

$result = gqlq '{ foos(distinct: "bar", where: { baz: { gte: 2 } }) { totalCount } }'
if ($result -notmatch '"totalCount":2') { throw "distinct with where: $result" }
pass "serve: distinct with where filter"

# 38e. GROUP BY on typed aggregate queries
$result = gqlq '{ foosAggregate(groupBy: "bar") { groups { key count } } }'
if ($result -notmatch '"key":"val"') { throw "groupBy missing val: $result" }
if ($result -notmatch '"key":"other"') { throw "groupBy missing other: $result" }
pass "serve: groupBy returns per-group counts"

$result = gqlq '{ foosAggregate(groupBy: "bar") { groups { key count minBaz maxBaz } } }'
if ($result -notmatch '"minBaz"') { throw "groupBy missing minBaz: $result" }
if ($result -notmatch '"maxBaz"') { throw "groupBy missing maxBaz: $result" }
pass "serve: groupBy with numeric aggregates"

$result = gqlq '{ foosAggregate(groupBy: "bar", where: { baz: { gte: 2 } }) { groups { key count } } }'
if ($result -notmatch '"key"') { throw "groupBy with where: $result" }
pass "serve: groupBy with where filter"

$result = gqlq '{ foosAggregate { count } }'
if ($result -notmatch '"count":3') { throw "aggregate without groupBy: $result" }
pass "serve: aggregate without groupBy still works"

# 38f. executeBatch mutation
$result = gqlq 'mutation { executeBatch(statements: ["INSERT INTO foo (title, bar, baz) VALUES (''batch1'', ''b1'', 10)", "INSERT INTO foo (title, bar, baz) VALUES (''batch2'', ''b2'', 20)"]) { message affected } }'
if ($result -match '"errors"') { throw "executeBatch errors: $result" }
pass "serve: executeBatch multiple INSERTs"

$ver = if ((gqlq '{ schemaVersion }') -match '"schemaVersion":(\d+)') { [int]$Matches[1] } else { 0 }
$result = gqlq 'mutation { executeBatch(statements: ["CREATE TABLE batchtest (col1 TEXT)"]) { message } }'
if ($result -notmatch '"message"') { throw "executeBatch DDL: $result" }
waitSchemaReload $ver
$result = gqlq '{ batchtests { totalCount } }'
if ($result -notmatch '"totalCount":0') { throw "executeBatch schema reload: $result" }
pass "serve: executeBatch DDL triggers schema reload"

# executeBatch failure rolls back
$preCount = (gqlq '{ foosAggregate { count } }') -replace '.*"count":(\d+).*','$1'
try { gqlq 'mutation { executeBatch(statements: ["INSERT INTO foo (title, bar, baz) VALUES (''rollback_test'', ''rb'', 99)", "INSERT INTO no_such_table (title) VALUES (''bad'')"]) { message } }' } catch {}
Start-Sleep -Seconds 1
$postCount = (gqlq '{ foosAggregate { count } }') -replace '.*"count":(\d+).*','$1'
if ($preCount -ne $postCount) { throw "executeBatch rollback failed: pre=$preCount post=$postCount" }
pass "serve: executeBatch failure rolls back all statements"

gqlq 'mutation { executeSql(sql: "DROP TABLE batchtest CASCADE") { message } }' | Out-Null

# 38g. batchUpdate mutation
$bu1 = gqlq 'mutation { createDoogat(input: { title: "BatchUp Alpha" }) { id } }'
$BU1_ID = if ($bu1 -match '"id":"([^"]+)"') { $Matches[1] } else { throw "no bu1 id" }
$bu2 = gqlq 'mutation { createDoogat(input: { title: "BatchUp Beta" }) { id } }'
$BU2_ID = if ($bu2 -match '"id":"([^"]+)"') { $Matches[1] } else { throw "no bu2 id" }
$bu3 = gqlq 'mutation { createDoogat(input: { title: "BatchUp Gamma" }) { id } }'
$BU3_ID = if ($bu3 -match '"id":"([^"]+)"') { $Matches[1] } else { throw "no bu3 id" }

$result = gqlq "mutation { batchUpdate(updates: [{id: `"$BU1_ID`", title: `"Updated Alpha`"}, {id: `"$BU2_ID`", title: `"Updated Beta`"}, {id: `"$BU3_ID`", title: `"Updated Gamma`"}]) { id title } }"
$parsed = $result | ConvertFrom-Json
if ($parsed.data.batchUpdate.Count -ne 3) { throw "batchUpdate expected 3 items, got $($parsed.data.batchUpdate.Count)" }
pass "serve: batchUpdate returns 3 items"

$item1 = $parsed.data.batchUpdate | Where-Object { $_.id -eq $BU1_ID }
$item2 = $parsed.data.batchUpdate | Where-Object { $_.id -eq $BU2_ID }
$item3 = $parsed.data.batchUpdate | Where-Object { $_.id -eq $BU3_ID }
if ($item1.title -ne "Updated Alpha") { throw "batchUpdate item1 title: $($item1.title)" }
if ($item2.title -ne "Updated Beta") { throw "batchUpdate item2 title: $($item2.title)" }
if ($item3.title -ne "Updated Gamma") { throw "batchUpdate item3 title: $($item3.title)" }
pass "serve: batchUpdate correct titles"

gqlq "mutation { deleteDoogat(id: `"$BU1_ID`") }" | Out-Null
gqlq "mutation { deleteDoogat(id: `"$BU2_ID`") }" | Out-Null
gqlq "mutation { deleteDoogat(id: `"$BU3_ID`") }" | Out-Null

# 38h. createMany mutation
$result = gqlq 'mutation { createMany(inputs: [{title: "Bulk A"}, {title: "Bulk B"}, {title: "Bulk C"}]) { id title } }'
$parsed = $result | ConvertFrom-Json
if ($parsed.data.createMany.Count -ne 3) { throw "createMany expected 3, got $($parsed.data.createMany.Count)" }
if ($parsed.data.createMany[0].title -ne "Bulk A") { throw "createMany[0] title: $($parsed.data.createMany[0].title)" }
if ($parsed.data.createMany[1].title -ne "Bulk B") { throw "createMany[1] title: $($parsed.data.createMany[1].title)" }
if ($parsed.data.createMany[2].title -ne "Bulk C") { throw "createMany[2] title: $($parsed.data.createMany[2].title)" }
pass "serve: createMany returns 3 items in order"

$CM_ID0 = $parsed.data.createMany[0].id
$verify = gqlq "{ doogat(id: `"$CM_ID0`") { title } }"
$vp = $verify | ConvertFrom-Json
if ($vp.data.doogat.title -ne "Bulk A") { throw "createMany persistence check failed" }
pass "serve: createMany persists records"

$emptyResult = gqlq 'mutation { createMany(inputs: []) { id } }'
$ep = $emptyResult | ConvertFrom-Json
if ($ep.data.createMany.Count -ne 0) { throw "createMany empty expected 0, got $($ep.data.createMany.Count)" }
pass "serve: createMany empty input"

# cleanup
$CM_ID1 = $parsed.data.createMany[1].id
$CM_ID2 = $parsed.data.createMany[2].id
gqlq "mutation { deleteDoogat(id: `"$CM_ID0`") }" | Out-Null
gqlq "mutation { deleteDoogat(id: `"$CM_ID1`") }" | Out-Null
gqlq "mutation { deleteDoogat(id: `"$CM_ID2`") }" | Out-Null

# 38i. typed field updates via GraphQL (updateDoogat fields/unsetFields, deleteDoogat cleanup)
$ver = if ((gqlq '{ schemaVersion }') -match '"schemaVersion":(\d+)') { [int]$Matches[1] } else { 0 }
$output = gqlq 'mutation { executeSql(sql: "CREATE TABLE tfubookmark (url VARCHAR(200))") { message } }'
if ($output -notmatch "table tfubookmark created") { throw "create tfubookmark failed: $output" }
waitSchemaReload $ver
$output = gqlq 'mutation { executeSql(sql: "INSERT INTO tfubookmark (title, url) VALUES (\"TFU Test\", \"https://old.com\")") { message } }'
$TFU_ID = if ($output -match '"message":"(\d{14})"') { $Matches[1] } else { throw "insert tfubookmark bad id: $output" }
# Verify initial materialized row
$output = gqlq "mutation { executeSql(sql: `"SELECT url FROM tfubookmark WHERE id = '$TFU_ID'`", format: `"objects`") { rows } }"
if ($output -notmatch "https://old.com") { throw "initial url not found: $output" }
# updateDoogat with fields to change url
$output = gqlq "mutation { updateDoogat(input: { id: `"$TFU_ID`", fields: `"{\`"url\`":\`"https://updated.com\`"}`" }) { id } }"
if ($output -notmatch $TFU_ID) { throw "updateDoogat fields failed: $output" }
# Verify via SQL SELECT that materialized row has updated url
$output = gqlq "mutation { executeSql(sql: `"SELECT url FROM tfubookmark WHERE id = '$TFU_ID'`", format: `"objects`") { rows } }"
if ($output -notmatch "https://updated.com") { throw "url not updated: $output" }
pass "serve: typed field update via GraphQL updateDoogat"
# updateDoogat with unsetFields to remove url
$output = gqlq "mutation { updateDoogat(input: { id: `"$TFU_ID`", unsetFields: [`"url`"] }) { id } }"
if ($output -notmatch $TFU_ID) { throw "updateDoogat unsetFields failed: $output" }
# Verify url is gone (NULL)
$output = gqlq "mutation { executeSql(sql: `"SELECT url FROM tfubookmark WHERE id = '$TFU_ID'`", format: `"objects`") { rows } }"
if ($output -match "https://") { throw "url should be unset after unsetFields: $output" }
pass "serve: typed field unset via GraphQL updateDoogat"
# Delete the doogat and verify materialized row is gone
gqlq "mutation { deleteDoogat(id: `"$TFU_ID`") }" | Out-Null
$output = gqlq "mutation { executeSql(sql: `"SELECT COUNT(*) FROM tfubookmark WHERE id = '$TFU_ID'`") { rows } }"
if ($output -notmatch '\[\\"0\\"\]') { throw "materialized row not cleaned after delete: $output" }
pass "serve: deleteDoogat cleans materialized type table row"
# Clean up typedef
gqlq 'mutation { executeSql(sql: "DROP TABLE tfubookmark CASCADE") { message } }' | Out-Null

# Hyphenated type names in GraphQL
ddl "mutation { executeSql(sql: `"CREATE TABLE \`"test-widget\`" (status TEXT, priority INTEGER)`") { message } }"
gqlq "mutation { executeSql(sql: `"INSERT INTO \`"test-widget\`" (status, priority) VALUES ('active', 1)`") { message } }" | Out-Null
$result = gqlq "{ testWidgets { items { id status priority } totalCount } }"
$parsed = $result | ConvertFrom-Json
if ($parsed.data.testWidgets.totalCount -ne 1) { throw "testWidgets expected 1 item, got $($parsed.data.testWidgets.totalCount)" }
if ($parsed.data.testWidgets.items[0].status -ne "active") { throw "status expected active, got $($parsed.data.testWidgets.items[0].status)" }
if ($parsed.data.testWidgets.items[0].priority -ne 1) { throw "priority expected 1, got $($parsed.data.testWidgets.items[0].priority)" }
pass "serve: hyphenated type typed query"

# 42. base field filters on typed queries (id, title)
gqlq "mutation { executeSql(sql: `"INSERT INTO \`"test-widget\`" (title, status, priority) VALUES ('FilterTarget', 'pending', 5)`") { message } }" | Out-Null
Start-Sleep -Seconds 1

$result = gqlq "{ testWidgets(where: { title: { eq: `"FilterTarget`" } }) { items { id title } totalCount } }"
$parsed = $result | ConvertFrom-Json
if ($parsed.data.testWidgets.totalCount -ne 1) { throw "title eq filter expected 1, got $($parsed.data.testWidgets.totalCount)" }
$BF_ID = $parsed.data.testWidgets.items[0].id
pass "serve: base field title eq filter"

$result = gqlq "{ testWidgets(where: { id: { eq: `"$BF_ID`" } }) { items { id title } totalCount } }"
$parsed = $result | ConvertFrom-Json
if ($parsed.data.testWidgets.totalCount -ne 1) { throw "id eq filter expected 1, got $($parsed.data.testWidgets.totalCount)" }
if ($parsed.data.testWidgets.items[0].id -ne $BF_ID) { throw "id mismatch" }
pass "serve: base field id eq filter"

$result = gqlq "{ testWidgets(where: { title: { contains: `"Target`" } }) { items { id } totalCount } }"
$parsed = $result | ConvertFrom-Json
if ($parsed.data.testWidgets.totalCount -ne 1) { throw "title contains filter expected 1, got $($parsed.data.testWidgets.totalCount)" }
pass "serve: base field title contains filter"

$result = gqlq "{ testWidgets(where: { id: { eq: `"99999999999999`" } }) { items { id } totalCount } }"
$parsed = $result | ConvertFrom-Json
if ($parsed.data.testWidgets.totalCount -ne 0) { throw "nonexistent id expected 0, got $($parsed.data.testWidgets.totalCount)" }
pass "serve: base field id nonexistent returns empty"

gqlq "mutation { deleteDoogat(id: `"$BF_ID`") }" | Out-Null
gqlq "mutation { executeSql(sql: `"DROP TABLE \`"test-widget\`"`") { message } }" | Out-Null

# 43. SQL INSERT via executeSql defaults date, created_at non-null
gqlq "mutation{executeSql(sql:`"CREATE TABLE datecheck (name TEXT)`"){message}}" | Out-Null
$dcResult = gqlq "mutation{executeSql(sql:`"INSERT INTO datecheck (name) VALUES (\`"DateTest\`")`"){message}}"
$dcId = ($dcResult | ConvertFrom-Json).data.executeSql.message
$dcExpected = "$($dcId.Substring(0,4))-$($dcId.Substring(4,2))-$($dcId.Substring(6,2))"
$dcQuery = gqlq "{ datechecks { items { id created_at } } }"
$dcCreated = ($dcQuery | ConvertFrom-Json).data.datechecks.items[0].created_at
if ($dcCreated -ne $dcExpected) { throw "created_at '$dcCreated' != expected '$dcExpected'" }
pass "serve: SQL INSERT defaults date, created_at matches ID"

# executeBatch also defaults date
$ebResult = gqlq "mutation{executeBatch(statements:[`"INSERT INTO datecheck (name) VALUES (\`"BatchTest\`")`"]){message}}"
$ebId = ($ebResult | ConvertFrom-Json).data.executeBatch[0].message
$ebExpected = "$($ebId.Substring(0,4))-$($ebId.Substring(4,2))-$($ebId.Substring(6,2))"
$ebQuery = gqlq "{ doogat(id: `"$ebId`") { created_at } }"
$ebCreated = ($ebQuery | ConvertFrom-Json).data.doogat.created_at
if ($ebCreated -ne $ebExpected) { throw "executeBatch created_at '$ebCreated' != expected '$ebExpected'" }
pass "serve: executeBatch INSERT defaults date, created_at matches ID"

# 43.D SQL constraint enforcement on executeSql write path (PRD 00122 / issue #7)
# Six checks (D1-D6) extending the existing INSERT-validation neighborhood.

# Setup: NOT NULL link table for D1-D5 and a numeric table for D3.
$ver = if ((gqlq '{ schemaVersion }') -match '"schemaVersion":(\d+)') { [int]$Matches[1] } else { 0 }
gqlq "mutation{executeSql(sql:`"CREATE TABLE link_d1 (title VARCHAR(255) NOT NULL, url VARCHAR(255) NOT NULL)`"){message}}" | Out-Null
gqlq "mutation{executeSql(sql:`"CREATE TABLE numeric_d3 (title VARCHAR(255) NOT NULL, count INTEGER)`"){message}}" | Out-Null
waitSchemaReload $ver

# D1. NOT NULL: INSERT with NULL title is rejected and no row is created.
$d1Result = gqlq "mutation{executeSql(sql:`"INSERT INTO link_d1 (title, url) VALUES (NULL, \`"https://n.com\`")`"){message}}"
if ($d1Result -notmatch 'NOT NULL constraint violated: link_d1.title') {
    throw "D1: expected NOT NULL error, got: $d1Result"
}
$d1Count = gqlq "mutation{executeSql(sql:`"SELECT COUNT(*) FROM link_d1`"){rows}}"
if ($d1Count -notmatch '\[\\"0\\"\]') { throw "D1: expected zero rows, got: $d1Count" }
pass "serve: D1 INSERT NULL on NOT NULL is rejected, no row created"

# D2. VARCHAR(N) overflow: 300-char title against VARCHAR(255) is rejected.
$long = 'x' * 300
$d2Result = gqlq "mutation{executeSql(sql:`"INSERT INTO link_d1 (title, url) VALUES (\`"$long\`", \`"https://v.com\`")`"){message}}"
if ($d2Result -notmatch 'value too long for link_d1.title') {
    throw "D2: expected length error, got: $d2Result"
}
$d2Count = gqlq "mutation{executeSql(sql:`"SELECT COUNT(*) FROM link_d1`"){rows}}"
if ($d2Count -notmatch '\[\\"0\\"\]') { throw "D2: expected zero rows, got: $d2Count" }
pass "serve: D2 VARCHAR(N) overflow is rejected, no row created"

# D3. INTEGER type mismatch: non-numeric value into INTEGER column is rejected.
$d3Result = gqlq "mutation{executeSql(sql:`"INSERT INTO numeric_d3 (title, count) VALUES (\`"a\`", \`"not_a_number\`")`"){message}}"
if ($d3Result -notmatch 'type mismatch for numeric_d3.count: expected INTEGER') {
    throw "D3: expected type-mismatch error, got: $d3Result"
}
$d3Count = gqlq "mutation{executeSql(sql:`"SELECT COUNT(*) FROM numeric_d3`"){rows}}"
if ($d3Count -notmatch '\[\\"0\\"\]') { throw "D3: expected zero rows, got: $d3Count" }
pass "serve: D3 INTEGER type mismatch is rejected, no row created"

# D4. Unknown column on INSERT: column not in schema is rejected.
$d4Result = gqlq "mutation{executeSql(sql:`"INSERT INTO link_d1 (title, url, unknown_col) VALUES (\`"t\`", \`"https://u.com\`", \`"dropped\`")`"){message}}"
if ($d4Result -notmatch 'unknown column: link_d1.unknown_col') {
    throw "D4: expected unknown-column error, got: $d4Result"
}
$d4Count = gqlq "mutation{executeSql(sql:`"SELECT COUNT(*) FROM link_d1`"){rows}}"
if ($d4Count -notmatch '\[\\"0\\"\]') { throw "D4: expected zero rows, got: $d4Count" }
pass "serve: D4 unknown column on INSERT is rejected, no row created"

# D5. Unknown column on UPDATE: insert one valid row, then UPDATE with bogus
# column. The original row's title must be unchanged after the rejection.
$d5Valid = gqlq "mutation{executeSql(sql:`"INSERT INTO link_d1 (title, url) VALUES (\`"keep\`", \`"https://k.com\`")`"){message}}"
$d5Id = ($d5Valid | ConvertFrom-Json).data.executeSql.message
$d5Result = gqlq "mutation{executeSql(sql:`"UPDATE link_d1 SET unknown_col = 'x' WHERE id = '$d5Id'`"){message}}"
if ($d5Result -notmatch 'unknown column: link_d1.unknown_col') {
    throw "D5: expected unknown-column error, got: $d5Result"
}
$d5Title = gqlq "mutation{executeSql(sql:`"SELECT title FROM link_d1 WHERE id = '$d5Id'`"){rows}}"
if ($d5Title -notmatch 'keep') { throw "D5: row mutated unexpectedly, got: $d5Title" }
pass "serve: D5 unknown column on UPDATE is rejected, row unchanged"

# D6. Silent title fallback removed: title NOT NULL with no template, INSERT
# omitting title now fails instead of coercing url/description into title.
ddl "mutation{executeSql(sql:`"CREATE TABLE link_d6 (title VARCHAR(255) NOT NULL, url VARCHAR(255), description TEXT)`"){message}}"
$d6Result = gqlq "mutation{executeSql(sql:`"INSERT INTO link_d6 (url) VALUES (\`"https://notitle.com\`")`"){message}}"
if ($d6Result -notmatch 'NOT NULL constraint violated: link_d6.title') {
    throw "D6: expected NOT NULL error, got: $d6Result"
}
$d6Count = gqlq "mutation{executeSql(sql:`"SELECT COUNT(*) FROM link_d6`"){rows}}"
if ($d6Count -notmatch '\[\\"0\\"\]') { throw "D6: expected zero rows, got: $d6Count" }
pass "serve: D6 silent title fallback removed, missing title rejected"

# Cleanup D-tables
gqlq "mutation{executeSql(sql:`"DROP TABLE link_d1 CASCADE`"){message}}" | Out-Null
gqlq "mutation{executeSql(sql:`"DROP TABLE numeric_d3 CASCADE`"){message}}" | Out-Null
gqlq "mutation{executeSql(sql:`"DROP TABLE link_d6 CASCADE`"){message}}" | Out-Null

# 44.E1 - Pin JOIN as working (#8 group E1, PRD 00123 archived as obsolete).
$ver = if ((gqlq '{ schemaVersion }') -match '"schemaVersion":(\d+)') { [int]$Matches[1] } else { 0 }
gqlq 'mutation { executeSql(sql: "CREATE TABLE e1_link (url VARCHAR(255))") { message } }' | Out-Null
gqlq 'mutation { executeSql(sql: "CREATE TABLE e1_num (count INTEGER)") { message } }' | Out-Null
waitSchemaReload $ver
gqlq 'mutation { executeSql(sql: "INSERT INTO e1_link (title, url) VALUES (''a'', ''https://a.com'')") { message } }' | Out-Null
gqlq 'mutation { executeSql(sql: "INSERT INTO e1_num (title, count) VALUES (''a'', 1)") { message } }' | Out-Null
$e1Join = gqlq '{ sql(query: "SELECT l.title, n.count FROM e1_link l JOIN e1_num n ON l.title = n.title") { rows } }'
assertGqlOk $e1Join "E1 JOIN"
if ($e1Join -notmatch '\\"a\\"') { throw "44.E1: JOIN response missing a: $e1Join" }
if ($e1Join -notmatch '\\"1\\"') { throw "44.E1: JOIN response missing 1: $e1Join" }
gqlq 'mutation { executeSql(sql: "DROP TABLE e1_link CASCADE") { message } }' | Out-Null
gqlq 'mutation { executeSql(sql: "DROP TABLE e1_num CASCADE") { message } }' | Out-Null
pass "issue-8-E1: SELECT ... JOIN returns joined rows (PRD 00123 archived as obsolete)"

# 44.J - Auto-junction tables populated atomically on SQL INSERT and UPDATE
# (PRD 00134). Mirrors the bash section 44.J in tests/integration.sh.
$ver = if ((gqlq '{ schemaVersion }') -match '"schemaVersion":(\d+)') { [int]$Matches[1] } else { 0 }
gqlq 'mutation { executeSql(sql: "CREATE TABLE j134_cat (label VARCHAR(100))") { message } }' | Out-Null
gqlq 'mutation { executeSql(sql: "CREATE TABLE j134_bm (url TEXT, category TEXT REFERENCES j134_cat)") { message } }' | Out-Null
waitSchemaReload $ver

$j134CatA = extractId (gqlq 'mutation { executeSql(sql: "INSERT INTO j134_cat (label) VALUES (''alpha'')") { message } }')
if (-not $j134CatA) { throw "44.J: failed to capture cat_a id" }
$j134CatB = extractId (gqlq 'mutation { executeSql(sql: "INSERT INTO j134_cat (label) VALUES (''beta'')") { message } }')
if (-not $j134CatB) { throw "44.J: failed to capture cat_b id" }
if ($j134CatA -eq $j134CatB) { throw "44.J: categories must have distinct ids" }

# T1's INSERT-side fix: bookmark INSERT must populate j134_bm_category.
$j134BmResp = gqlq "mutation { executeSql(sql: `"INSERT INTO j134_bm (url, category) VALUES ('https://j134.example', '$j134CatA')`") { message } }"
$j134Bm = extractId $j134BmResp
if (-not $j134Bm) { throw "44.J: failed to capture bookmark id: $j134BmResp" }

$j134J1 = gqlq "mutation { executeSql(sql: `"SELECT category_id FROM j134_bm_category WHERE j134_bm_id = '$j134Bm'`") { rows } }"
if ($j134J1 -notmatch [regex]::Escape($j134CatA)) { throw "44.J: junction missing cat_a after INSERT: $j134J1" }
pass "PRD 00134: SQL INSERT populates auto-junction atomically (no rebuild)"

# T2's UPDATE-side fix: re-pointing the REFERENCES column must drop the old
# junction row and insert the new one.
gqlq "mutation { executeSql(sql: `"UPDATE j134_bm SET category = '$j134CatB' WHERE id = '$j134Bm'`") { message } }" | Out-Null

$j134JOld = gqlq "mutation { executeSql(sql: `"SELECT COUNT(*) FROM j134_bm_category WHERE j134_bm_id = '$j134Bm' AND category_id = '$j134CatA'`") { rows } }"
if ($j134JOld -notmatch '\\"0\\"') { throw "44.J: stale junction row to cat_a not removed after UPDATE: $j134JOld" }

$j134JNew = gqlq "mutation { executeSql(sql: `"SELECT COUNT(*) FROM j134_bm_category WHERE j134_bm_id = '$j134Bm' AND category_id = '$j134CatB'`") { rows } }"
if ($j134JNew -notmatch '\\"1\\"') { throw "44.J: new junction row to cat_b not inserted after UPDATE: $j134JNew" }
pass "PRD 00134: SQL UPDATE syncs auto-junction (old removed, new inserted)"

# 44.K - GraphQL createDoogat typed-create populates auto-junction atomically
# (PRD 00134 cycle-1 review C1 task #4). Mirrors bash section 44.K. Drives
# the full GraphQL -> service -> indexer pipeline; unit tests cover the
# service layer in detail (see service::tests::create_doogat_with_extra_*).
$j134kCat = extractId (gqlq "mutation { executeSql(sql: `"INSERT INTO j134_cat (label) VALUES ('gamma')`") { message } }")
if (-not $j134kCat) { throw "44.K: failed to capture cat id" }
$j134kFields = "{\`"url\`":\`"https://k.example\`",\`"category\`":\`"$j134kCat\`"}"
$j134kResp = gqlq "mutation { createDoogat(input: { type: `"j134_bm`", title: `"K`", fields: `"$j134kFields`" }) { id } }"
# `createDoogat` returns the id under `data.createDoogat.id`, not `message`,
# so extractId (which scans for "message") would yield empty. Match the id
# field directly.
$j134kBm = if ($j134kResp -match '"id":"([^"]+)"') { $Matches[1] } else { "" }
if (-not $j134kBm) { throw "44.K: failed to capture bookmark id from createDoogat: $j134kResp" }
$j134kJ = gqlq "mutation { executeSql(sql: `"SELECT COUNT(*) FROM j134_bm_category WHERE j134_bm_id = '$j134kBm' AND category_id = '$j134kCat'`") { rows } }"
if ($j134kJ -notmatch '\\"1\\"') { throw "44.K: createDoogat did not populate auto-junction: $j134kJ" }
pass "PRD 00134: GraphQL createDoogat populates auto-junction for REFERENCES column (44.K)"

# Cleanup
gqlq 'mutation { executeSql(sql: "DROP TABLE j134_bm CASCADE") { message } }' | Out-Null
gqlq 'mutation { executeSql(sql: "DROP TABLE j134_cat CASCADE") { message } }' | Out-Null

# 44. DDL response consistency (no spurious errors)
$ver = if ((gqlq '{ schemaVersion }') -match '"schemaVersion":(\d+)') { [int]$Matches[1] } else { 0 }
$result = gqlq 'mutation { executeSql(sql: "CREATE TABLE ddltest (name VARCHAR(100))") { columns rows message } }'
if ($result -match '"errors"') { throw "CREATE TABLE has errors: $result" }
if ($result -notmatch '"columns":\[\]') { throw "CREATE TABLE columns not empty: $result" }
if ($result -notmatch '"rows":\[\]') { throw "CREATE TABLE rows not empty: $result" }
if ($result -notmatch '"message"') { throw "CREATE TABLE missing message: $result" }
pass "serve: CREATE TABLE response has no errors"
waitSchemaReload $ver

$ver = if ((gqlq '{ schemaVersion }') -match '"schemaVersion":(\d+)') { [int]$Matches[1] } else { 0 }
$result = gqlq 'mutation { executeSql(sql: "ALTER TABLE ddltest ADD COLUMN age INTEGER") { columns rows message } }'
if ($result -match '"errors"') { throw "ALTER TABLE has errors: $result" }
if ($result -notmatch '"columns":\[\]') { throw "ALTER TABLE columns not empty: $result" }
if ($result -notmatch '"rows":\[\]') { throw "ALTER TABLE rows not empty: $result" }
pass "serve: ALTER TABLE response has no errors"
waitSchemaReload $ver

$result = gqlq 'mutation { executeSql(sql: "DROP TABLE ddltest") { columns rows message } }'
if ($result -match '"errors"') { throw "DROP TABLE has errors: $result" }
if ($result -notmatch '"columns":\[\]') { throw "DROP TABLE columns not empty: $result" }
if ($result -notmatch '"rows":\[\]') { throw "DROP TABLE rows not empty: $result" }
pass "serve: DROP TABLE response has no errors"

$ver = if ((gqlq '{ schemaVersion }') -match '"schemaVersion":(\d+)') { [int]$Matches[1] } else { 0 }
$result = gqlq 'mutation { executeBatch(statements: ["CREATE TABLE ddlbatch1 (name VARCHAR)", "CREATE TABLE ddlbatch2 (val INTEGER)"]) { columns rows message } }'
if ($result -match '"errors"') { throw "executeBatch DDL has errors: $result" }
if ($result -notmatch '"columns":\[\]') { throw "executeBatch DDL columns not empty: $result" }
if ($result -notmatch '"rows":\[\]') { throw "executeBatch DDL rows not empty: $result" }
pass "serve: executeBatch DDL responses have no errors"
waitSchemaReload $ver

gqlq 'mutation { executeSql(sql: "DROP TABLE ddlbatch1 CASCADE") { message } }' | Out-Null
gqlq 'mutation { executeSql(sql: "DROP TABLE ddlbatch2 CASCADE") { message } }' | Out-Null

$dmlResult = gqlq 'mutation { executeSql(sql: "INSERT INTO datecheck (name) VALUES (\"DmlRegression\")") { affected message } }'
if ($dmlResult -match '"errors"') { throw "DML INSERT has errors: $dmlResult" }
if ($dmlResult -notmatch '"message"') { throw "DML INSERT missing message: $dmlResult" }
pass "serve: DML INSERT response unchanged"

# 45. createMany onConflict: IGNORE (upsert via GraphQL)
ddl 'mutation { executeSql(sql: "CREATE TABLE upsertgql (code TEXT, label TEXT)") { message } }'
Push-Location $TMPDIR
$utFile = Get-ChildItem -Path ddb/_typedef -Filter *.md | Where-Object {
    (Get-Content $_.FullName -Raw) -match "title: upsertgql"
} | Select-Object -First 1
$utContent = Get-Content $utFile.FullName -Raw
$utContent = $utContent -replace "type: _typedef", "type: _typedef`nunique_together:`n  - - code"
Set-Content -Path $utFile.FullName -Value $utContent -NoNewline
git add -A | Out-Null
git commit -m "add unique_together to upsertgql" --quiet | Out-Null
ddb reindex | Out-Null
$cm1 = gqlq 'mutation { createMany(inputs: [{title: "UpsertA", type: "upsertgql", fields: "{\"code\":\"X1\",\"label\":\"first\"}"}]) { id title } }'
$cm1Id = ($cm1 | ConvertFrom-Json).data.createMany[0].id
if (-not $cm1Id) { throw "upsert seed failed: $cm1" }
$cm2 = gqlq 'mutation { createMany(inputs: [{title: "UpsertA Dup", type: "upsertgql", fields: "{\"code\":\"X1\",\"label\":\"second\"}"}], onConflict: IGNORE) { id title } }'
$cm2Obj = ($cm2 | ConvertFrom-Json).data.createMany[0]
if ($cm2Obj.id -ne $cm1Id) { throw "upsert IGNORE should return existing ID: got $($cm2Obj.id) expected $cm1Id" }
if ($cm2Obj.title -ne "UpsertA") { throw "upsert IGNORE should return original title: got $($cm2Obj.title)" }
pass "serve: createMany onConflict IGNORE returns existing"
gqlq 'mutation { executeSql(sql: "DROP TABLE upsertgql CASCADE") { message } }' | Out-Null
Pop-Location

# === PRD 00130: GraphQL typed-write polish (issues #11/#12/#13) ===

# 45.G13 - issue #13: createDoogat must omit `title` when the typedef has a
# title_template, rendering it server-side. Pre-PRD-00130 the schema marked
# title NON_NULL so the template never fired through GraphQL.
ddl 'mutation { executeSql(sql: "CREATE TABLE g13link (title TEXT, url VARCHAR(255))") { message } }'
ddl "mutation { executeSql(sql: ""ALTER TABLE g13link SET TITLE TEMPLATE 'link-{url}'"") { message } }"
$g13Omit = gqlq 'mutation { createDoogat(input: {type: "g13link", fields: "{\"url\":\"https://example.com\"}"}) { id title } }'
assertGqlOk $g13Omit "G13 omit title"
$g13Title = ($g13Omit | ConvertFrom-Json).data.createDoogat.title
if ($g13Title -ne "link-https://example.com") { throw "issue-13: expected link-https://example.com, got $g13Title" }
pass "issue-13: createDoogat omits title when typedef has title_template"

ddl 'mutation { executeSql(sql: "CREATE TABLE g13plain (title TEXT, url VARCHAR(255))") { message } }'
$g13Null = gqlq 'mutation { createDoogat(input: {type: "g13plain", fields: "{\"url\":\"https://x\"}"}) { id } }'
assertGqlErrors $g13Null "G13 plain rejection"
if ($g13Null -notmatch 'NOT NULL constraint violated: g13plain\.title') { throw "issue-13: expected NOT_NULL_VIOLATION on g13plain.title, got $g13Null" }
pass "issue-13: createDoogat without title or template rejects with NOT_NULL_VIOLATION"

gqlq 'mutation { executeSql(sql: "DROP TABLE g13link CASCADE") { message } }' | Out-Null
gqlq 'mutation { executeSql(sql: "DROP TABLE g13plain CASCADE") { message } }' | Out-Null

# 45.G12 - issue #12: createMany(onConflict: IGNORE) returns the surviving
# row's ID for skipped rows when both duplicates appear in the same batch.
ddl 'mutation { executeSql(sql: "CREATE TABLE g12item (code TEXT, label TEXT)") { message } }'
Push-Location $TMPDIR
$g12File = Get-ChildItem -Path ddb/_typedef -Filter *.md | Where-Object {
    (Get-Content $_.FullName -Raw) -match "title: g12item"
} | Select-Object -First 1
$g12Content = Get-Content $g12File.FullName -Raw
$g12Content = $g12Content -replace "type: _typedef", "type: _typedef`nunique_together:`n  - - code"
Set-Content -Path $g12File.FullName -Value $g12Content -NoNewline
git add -A | Out-Null
git commit -m "add unique_together to g12item" --quiet | Out-Null
ddb reindex | Out-Null
$g12Batch = gqlq 'mutation { createMany(inputs: [{title: "A", type: "g12item", fields: "{\"code\":\"K1\",\"label\":\"first\"}"}, {title: "A Dup", type: "g12item", fields: "{\"code\":\"K1\",\"label\":\"second\"}"}], onConflict: IGNORE) { id title } }'
assertGqlOk $g12Batch "G12 batch"
$g12Items = ($g12Batch | ConvertFrom-Json).data.createMany
if (-not $g12Items[0].id) { throw "issue-12: empty surviving id" }
if ($g12Items[0].id -ne $g12Items[1].id) { throw "issue-12: intra-batch duplicate must return same surviving id, got $($g12Items[0].id) vs $($g12Items[1].id)" }
if ($g12Items[1].title -ne "A") { throw "issue-12: duplicate payload must carry surviving title 'A', got $($g12Items[1].title)" }
$g12CountResp = gqlq 'mutation { executeSql(sql: "SELECT COUNT(*) FROM g12item") { rows } }'
$g12Count = (($g12CountResp | ConvertFrom-Json).data.executeSql.rows[0] | ConvertFrom-Json)[0]
if ($g12Count -ne "1") { throw "issue-12: expected exactly 1 row in g12item, got $g12Count" }
pass "issue-12: createMany IGNORE returns surviving ID for intra-batch duplicate"
Pop-Location
gqlq 'mutation { executeSql(sql: "DROP TABLE g12item CASCADE") { message } }' | Out-Null

# 45.G11 - issue #11: TagsFilter operators are nullable; contains-only
# filter parses; empty filter and empty arrays are rejected at resolve time.
ddl 'mutation { executeSql(sql: "CREATE TABLE g11link (title TEXT, url VARCHAR(255))") { message } }'
$g11Create = gqlq 'mutation { createDoogat(input: {title: "Tagged", type: "g11link", tags: ["rust", "sql"], fields: "{\"url\":\"https://example.com\"}"}) { id } }'
assertGqlOk $g11Create "G11 create"

$g11Contains = gqlq '{ g11links(where: {tags: {contains: "rust"}}) { totalCount } }'
assertGqlOk $g11Contains "G11 contains-only"
$g11ContainsCount = ($g11Contains | ConvertFrom-Json).data.g11links.totalCount
if ($g11ContainsCount -ne 1) { throw "issue-11: contains-only expected 1 row, got $g11ContainsCount" }
pass "issue-11: TagsFilter contains-only filter parses and matches"

$g11All = gqlq '{ g11links(where: {tags: {containsAll: ["rust", "sql"]}}) { totalCount } }'
assertGqlOk $g11All "G11 containsAll-only"
if ((($g11All | ConvertFrom-Json).data.g11links.totalCount) -ne 1) { throw "issue-11: containsAll-only count mismatch" }
pass "issue-11: TagsFilter containsAll-only filter parses"

$g11Any = gqlq '{ g11links(where: {tags: {containsAny: ["rust", "go"]}}) { totalCount } }'
assertGqlOk $g11Any "G11 containsAny-only"
if ((($g11Any | ConvertFrom-Json).data.g11links.totalCount) -ne 1) { throw "issue-11: containsAny-only count mismatch" }
pass "issue-11: TagsFilter containsAny-only filter parses"

$g11Empty = gqlq '{ g11links(where: {tags: {}}) { totalCount } }'
assertGqlErrors $g11Empty "G11 empty filter"
if ($g11Empty -notmatch "tags filter requires at least one of") { throw "issue-11: empty filter wrong error: $g11Empty" }
pass "issue-11: empty TagsFilter rejected with clear error"

$g11EmptyAll = gqlq '{ g11links(where: {tags: {containsAll: []}}) { totalCount } }'
assertGqlErrors $g11EmptyAll "G11 empty containsAll"
if ($g11EmptyAll -notmatch "containsAll cannot be empty") { throw "issue-11: empty containsAll wrong error: $g11EmptyAll" }
pass "issue-11: empty containsAll rejected with clear error"

$g11EmptyAny = gqlq '{ g11links(where: {tags: {containsAny: []}}) { totalCount } }'
assertGqlErrors $g11EmptyAny "G11 empty containsAny"
if ($g11EmptyAny -notmatch "containsAny cannot be empty") { throw "issue-11: empty containsAny wrong error: $g11EmptyAny" }
pass "issue-11: empty containsAny rejected with clear error"

$g11Sdl = gqlq '{ __type(name: "TagsFilter") { inputFields { name type { kind ofType { kind name ofType { kind name } } } } } }'
$g11SdlFields = ($g11Sdl | ConvertFrom-Json).data.__type.inputFields
$g11AllField = $g11SdlFields | Where-Object { $_.name -eq "containsAll" }
$g11AnyField = $g11SdlFields | Where-Object { $_.name -eq "containsAny" }
if ($g11AllField.type.kind -ne "LIST") { throw "issue-11: containsAll must be nullable LIST at top level, got $($g11AllField.type.kind)" }
if ($g11AnyField.type.kind -ne "LIST") { throw "issue-11: containsAny must be nullable LIST at top level, got $($g11AnyField.type.kind)" }
pass "issue-11: TagsFilter introspection confirms containsAll/containsAny are nullable lists"

gqlq 'mutation { executeSql(sql: "DROP TABLE g11link CASCADE") { message } }' | Out-Null

# 45.A1 - Cross-mutation parity after a failed UNIQUE INSERT (#4 group A1).
ddl 'mutation { executeSql(sql: "CREATE TABLE a1item (title VARCHAR(255) NOT NULL, name VARCHAR(255) NOT NULL, UNIQUE(name))") { message } }'
$a1Valid = gqlq 'mutation { executeSql(sql: "INSERT INTO a1item (title, name) VALUES (''a'', ''unique1'')") { message } }'
$A1_VALID_ID = extractId $a1Valid
if (-not $A1_VALID_ID) { throw "45.A1: could not extract A1_VALID_ID" }
$a1Dup = gqlq 'mutation { executeSql(sql: "INSERT INTO a1item (title, name) VALUES (''b'', ''unique1'')") { message } }'
assertGqlErrors $a1Dup "A1 duplicate"
if ($a1Dup -notmatch 'UNIQUE') { throw "45.A1: duplicate did not report UNIQUE: $a1Dup" }
$a1Upd = gqlq "mutation { updateDoogat(input: { id: `"$A1_VALID_ID`", tags: [`"a1-recovered`"] }) { id tags } }"
assertGqlOk $a1Upd "A1 updateDoogat recovery"
if ($a1Upd -notmatch 'a1-recovered') { throw "45.A1: updateDoogat did not surface tag: $a1Upd" }
$a1Create = gqlq 'mutation { createDoogat(input: { type: "a1item", title: "created-after-rollback", fields: "{\"name\":\"unique2\"}" }) { id title } }'
assertGqlOk $a1Create "A1 createDoogat recovery"
$A1_CREATE_ID = if ($a1Create -match '"id":"([^"]+)"') { $Matches[1] } else { throw "45.A1: no id from createDoogat" }
$a1Del = gqlq "mutation { deleteDoogat(id: `"$A1_CREATE_ID`") }"
assertGqlOk $a1Del "A1 deleteDoogat recovery"
if ($a1Del -notmatch 'true') { throw "45.A1: deleteDoogat did not return true: $a1Del" }
gqlq 'mutation { executeSql(sql: "DROP TABLE a1item CASCADE") { message } }' | Out-Null
pass "issue-4-A1: failed UNIQUE INSERT does not break update/create/delete mutations"

# 45.A3 - Cross-table isolation (#4 group A3).
$ver = if ((gqlq '{ schemaVersion }') -match '"schemaVersion":(\d+)') { [int]$Matches[1] } else { 0 }
gqlq 'mutation { executeSql(sql: "CREATE TABLE a3thing (title VARCHAR(255) NOT NULL)") { message } }' | Out-Null
gqlq 'mutation { executeSql(sql: "CREATE TABLE a3item (title VARCHAR(255) NOT NULL, name VARCHAR(255) NOT NULL, UNIQUE(name))") { message } }' | Out-Null
waitSchemaReload $ver
$a3Thing = gqlq 'mutation { executeSql(sql: "INSERT INTO a3thing (title) VALUES (''t1'')") { message } }'
$A3_THING_ID = extractId $a3Thing
if (-not $A3_THING_ID) { throw "45.A3: could not extract A3_THING_ID" }
gqlq 'mutation { executeSql(sql: "INSERT INTO a3item (title, name) VALUES (''a'', ''u1'')") { message } }' | Out-Null
$a3Dup = gqlq 'mutation { executeSql(sql: "INSERT INTO a3item (title, name) VALUES (''b'', ''u1'')") { message } }'
assertGqlErrors $a3Dup "A3 duplicate"
if ($a3Dup -notmatch 'UNIQUE') { throw "45.A3: duplicate did not report UNIQUE: $a3Dup" }
$a3Upd = gqlq "mutation { updateDoogat(input: { id: `"$A3_THING_ID`", tags: [`"a3-isolated`"] }) { id tags } }"
assertGqlOk $a3Upd "A3 updateDoogat a3thing recovery"
if ($a3Upd -notmatch 'a3-isolated') { throw "45.A3: a3thing update tag missing: $a3Upd" }
gqlq 'mutation { executeSql(sql: "DROP TABLE a3thing CASCADE") { message } }' | Out-Null
gqlq 'mutation { executeSql(sql: "DROP TABLE a3item CASCADE") { message } }' | Out-Null
pass "issue-4-A3: failed INSERT on a3item does not corrupt a3thing"

# 45.R10 - RESTRICT on NOT NULL REFERENCES blocks delete via SQL and
# deleteDoogat (#10). Mirrors the Bash scenario.
$ver = if ((gqlq '{ schemaVersion }') -match '"schemaVersion":(\d+)') { [int]$Matches[1] } else { 0 }
gqlq 'mutation { executeSql(sql: "CREATE TABLE r10link (url VARCHAR(255) NOT NULL)") { message } }' | Out-Null
gqlq 'mutation { executeSql(sql: "CREATE TABLE r10cat (name VARCHAR(255) NOT NULL)") { message } }' | Out-Null
gqlq 'mutation { executeSql(sql: "CREATE TABLE \"r10-mem\" (link_id VARCHAR(255) NOT NULL REFERENCES r10link(id), cat_id VARCHAR(255) NOT NULL REFERENCES r10cat(id), UNIQUE(link_id, cat_id))") { message } }' | Out-Null
waitSchemaReload $ver
$r10L = gqlq 'mutation { executeSql(sql: "INSERT INTO r10link (title, url) VALUES (''L'', ''https://r10.example'')") { message } }'
$R10_L_ID = extractId $r10L
if (-not $R10_L_ID) { throw "45.R10: could not extract R10_L_ID" }
$r10C = gqlq 'mutation { executeSql(sql: "INSERT INTO r10cat (title, name) VALUES (''C'', ''c'')") { message } }'
$R10_C_ID = extractId $r10C
if (-not $R10_C_ID) { throw "45.R10: could not extract R10_C_ID" }
gqlq "mutation { executeSql(sql: ""INSERT INTO \""r10-mem\"" (title, link_id, cat_id) VALUES ('M', '$R10_L_ID', '$R10_C_ID')"") { message } }" | Out-Null
$r10SqlErr = gqlq "mutation { executeSql(sql: ""DELETE FROM r10link WHERE id = '$R10_L_ID'"") { message } }"
assertGqlErrors $r10SqlErr "R10 SQL DELETE"
if ($r10SqlErr -notmatch "NOT NULL REFERENCES") { throw "45.R10: SQL DELETE error missing 'NOT NULL REFERENCES': $r10SqlErr" }
if ($r10SqlErr -notmatch "r10-mem") { throw "45.R10: SQL DELETE error missing blocking table 'r10-mem': $r10SqlErr" }
$r10GqlErr = gqlq "mutation { deleteDoogat(id: `"$R10_L_ID`") }"
assertGqlErrors $r10GqlErr "R10 deleteDoogat"
if ($r10GqlErr -notmatch "NOT NULL REFERENCES") { throw "45.R10: deleteDoogat error missing 'NOT NULL REFERENCES': $r10GqlErr" }
$r10Parent = gqlq "mutation { executeSql(sql: ""SELECT COUNT(*) FROM r10link WHERE id = '$R10_L_ID'"") { rows } }"
if ($r10Parent -notmatch '"rows":\["\[\\"1\\"\]"\]') { throw "45.R10: parent row missing after blocked delete: $r10Parent" }
$r10Child = gqlq "mutation { executeSql(sql: ""SELECT COUNT(*) FROM \""r10-mem\"" WHERE link_id = '$R10_L_ID'"") { rows } }"
if ($r10Child -notmatch '"rows":\["\[\\"1\\"\]"\]') { throw "45.R10: child row missing after blocked delete: $r10Child" }
gqlq "mutation { executeSql(sql: ""DELETE FROM \""r10-mem\"" WHERE link_id = '$R10_L_ID'"") { message } }" | Out-Null
$r10Ok = gqlq "mutation { executeSql(sql: ""DELETE FROM r10link WHERE id = '$R10_L_ID'"") { affected } }"
assertGqlOk $r10Ok "R10 final DELETE"
if ($r10Ok -notmatch '"affected":1') { throw "45.R10: expected affected:1 after child removed, got: $r10Ok" }
gqlq 'mutation { executeSql(sql: "DROP TABLE \"r10-mem\" CASCADE") { message } }' | Out-Null
gqlq 'mutation { executeSql(sql: "DROP TABLE r10link CASCADE") { message } }' | Out-Null
gqlq 'mutation { executeSql(sql: "DROP TABLE r10cat CASCADE") { message } }' | Out-Null
pass "issue-10: RESTRICT blocks delete via SQL and deleteDoogat"

# 45.A2 - Ghost-row fix persists across server restart (#4 group A2).
ddl 'mutation { executeSql(sql: "CREATE TABLE a2persist (title VARCHAR(255) NOT NULL, name VARCHAR(255) NOT NULL, UNIQUE(name))") { message } }'
$a2Valid = gqlq 'mutation { executeSql(sql: "INSERT INTO a2persist (title, name) VALUES (''seed'', ''uniq_a2'')") { message } }'
$A2_VALID_ID = extractId $a2Valid
if (-not $A2_VALID_ID) { throw "45.A2: could not extract A2_VALID_ID" }
$a2Dup = gqlq 'mutation { executeSql(sql: "INSERT INTO a2persist (title, name) VALUES (''dup'', ''uniq_a2'')") { message } }'
assertGqlErrors $a2Dup "A2 duplicate"

# Kill + restart the server on the same $TMPDIR.
Stop-Process -Id $serverProc.Id -Force -ErrorAction SilentlyContinue
Start-Sleep -Milliseconds 500
$serverProc = Start-Process -FilePath $DDB -ArgumentList @("serve", "--port", "$SERVER_PORT", "--pg-port", "$PG_PORT") -NoNewWindow -PassThru
for ($i = 0; $i -lt 20; $i++) {
    try {
        $null = Invoke-WebRequest -Uri $GQL_URL -Method POST -ContentType "application/json" `
            -Headers @{ Authorization = "Bearer $TOKEN" } -Body '{"query":"{ typeDefs { name } }"}' -ErrorAction Stop
        break
    } catch {
        Start-Sleep -Milliseconds 200
    }
}

$a2Upd = gqlq "mutation { updateDoogat(input: { id: `"$A2_VALID_ID`", tags: [`"restart-survived`"] }) { id tags } }"
assertGqlOk $a2Upd "A2 updateDoogat after restart"
if ($a2Upd -notmatch 'restart-survived') { throw "45.A2: tags missing after restart: $a2Upd" }
$a2Fresh = gqlq 'mutation { executeSql(sql: "INSERT INTO a2persist (title, name) VALUES (''fresh'', ''uniq_a2_post'')") { message } }'
assertGqlOk $a2Fresh "A2 fresh insert after restart"
gqlq 'mutation { executeSql(sql: "DROP TABLE a2persist CASCADE") { message } }' | Out-Null
pass "issue-4-A2: ghost-row fix persists across server restart"

# 49. PRD 00131: structured-error code propagation through typed mutations.
# Mirrors tests/integration.sh §49. Asserts `extensions.code` returns
# UNIQUE_VIOLATION / NOT_NULL_VIOLATION on createDoogat and createMany.
# Placed here while the GraphQL server is still running (it shuts down
# below before sections 46-48, which use the CLI).
Write-Host "=== PRD 00131: structured-error code propagation ==="

# 49.1 - createDoogat UNIQUE violation carries extensions.code.
ddl 'mutation { executeSql(sql: "CREATE TABLE puv_link (title VARCHAR(255), slug VARCHAR(255) NOT NULL, space VARCHAR(255) NOT NULL, UNIQUE(slug, space))") { message } }'

$seFirst = gqlq 'mutation { createDoogat(input: {type: "puv_link", title: "first", fields: "{\"slug\":\"hn\",\"space\":\"news\"}"}) { id } }'
assertGqlOk $seFirst "49.1 first insert"

$seDup = gqlq 'mutation { createDoogat(input: {type: "puv_link", title: "dup", fields: "{\"slug\":\"hn\",\"space\":\"news\"}"}) { id } }'
assertGqlErrors $seDup "49.1 duplicate"
$seDupParsed = $seDup | ConvertFrom-Json
if ($seDupParsed.errors[0].extensions.code -ne "UNIQUE_VIOLATION") {
    throw "49.1: expected UNIQUE_VIOLATION, got: $($seDupParsed.errors[0].extensions.code)"
}
$seDupCols = $seDupParsed.errors[0].extensions.columns
if (-not ($seDupCols -is [array]) -or $seDupCols.Count -ne 2 -or $seDupCols[0] -ne "slug" -or $seDupCols[1] -ne "space") {
    throw "49.1: expected columns [slug, space], got: $($seDupCols -join ',')"
}
$seDupVals = $seDupParsed.errors[0].extensions.values
if (-not ($seDupVals -is [array]) -or $seDupVals.Count -ne 2) {
    throw "49.1: expected values list of length 2, got: $($seDupVals -join ',')"
}
pass "49.1: createDoogat UNIQUE violation carries extensions.code = UNIQUE_VIOLATION"

# 49.2 - createDoogat NOT NULL violation carries extensions.code.
$seNn = gqlq 'mutation { createDoogat(input: {type: "puv_link", title: "missing-slug", fields: "{\"space\":\"news\"}"}) { id } }'
assertGqlErrors $seNn "49.2 missing required column"
$seNnParsed = $seNn | ConvertFrom-Json
if ($seNnParsed.errors[0].extensions.code -ne "NOT_NULL_VIOLATION") {
    throw "49.2: expected NOT_NULL_VIOLATION, got: $($seNnParsed.errors[0].extensions.code)"
}
if ($seNnParsed.errors[0].extensions.column -ne "slug") {
    throw "49.2: expected column=slug, got: $($seNnParsed.errors[0].extensions.column)"
}
pass "49.2: createDoogat NOT NULL violation carries extensions.code = NOT_NULL_VIOLATION"

# 49.3 - createMany single-input ERROR carries extensions.code.
$seCm = gqlq 'mutation { createMany(inputs: [{type: "puv_link", title: "cm-dup", fields: "{\"slug\":\"hn\",\"space\":\"news\"}"}], onConflict: ERROR) { id } }'
assertGqlErrors $seCm "49.3 createMany ERROR"
$seCmParsed = $seCm | ConvertFrom-Json
if ($seCmParsed.errors[0].extensions.code -ne "UNIQUE_VIOLATION") {
    throw "49.3: expected UNIQUE_VIOLATION, got: $($seCmParsed.errors[0].extensions.code)"
}
pass "49.3: createMany ERROR UNIQUE violation carries extensions.code = UNIQUE_VIOLATION"

# 49.4 - createMany multi-input intra-batch duplicate under ERROR.
$seCmIntra = gqlq 'mutation { createMany(inputs: [{type: "puv_link", title: "cm-a", fields: "{\"slug\":\"twin\",\"space\":\"news\"}"}, {type: "puv_link", title: "cm-b", fields: "{\"slug\":\"twin\",\"space\":\"news\"}"}], onConflict: ERROR) { id } }'
assertGqlErrors $seCmIntra "49.4 intra-batch ERROR"
$seCmIntraParsed = $seCmIntra | ConvertFrom-Json
if ($seCmIntraParsed.errors[0].extensions.code -ne "UNIQUE_VIOLATION") {
    throw "49.4: expected UNIQUE_VIOLATION, got: $($seCmIntraParsed.errors[0].extensions.code)"
}
pass "49.4: createMany intra-batch ERROR carries extensions.code = UNIQUE_VIOLATION"

# Cleanup: drop the typedef so subsequent runs start clean.
ddl 'mutation { executeSql(sql: "DROP TABLE puv_link") { message } }'

# 50. PRD 00132: ALTER TABLE foo RENAME TO bar across protocols.
# 50a. via GraphQL executeSql.
ddl 'mutation { executeSql(sql: "CREATE TABLE rngql_src (title VARCHAR(64))") { message } }'
ddl 'mutation { executeSql(sql: "ALTER TABLE rngql_src RENAME TO rngql_dst") { message } }'
$rnGqlOk = gqlq 'mutation { executeSql(sql: "SELECT count(*) FROM rngql_dst") { message } }'
if ($rnGqlOk -notmatch '"data"') { throw "50a: expected data after rename, got $rnGqlOk" }
$rnGqlOld = gqlq 'mutation { executeSql(sql: "SELECT count(*) FROM rngql_src") { message } }'
assertGqlErrors $rnGqlOld "50a: old name should no longer resolve"
pass "50a: ALTER TABLE RENAME TO via GraphQL executeSql succeeds; old name no longer resolves"

# 50b. MySQL alias rejected with explicit message.
$rnAlias = gqlq 'mutation { executeSql(sql: "RENAME TABLE rngql_dst TO rngql_dst2") { message } }'
if ($rnAlias -notmatch 'RENAME TABLE not supported') {
    throw "50b: expected RENAME TABLE rejection hint, got $rnAlias"
}
pass "50b: MySQL RENAME TABLE alias rejected with explicit ALTER TABLE hint"

# Cleanup so subsequent runs start clean.
ddl 'mutation { executeSql(sql: "DROP TABLE rngql_dst") { message } }'

Stop-Process -Id $serverProc.Id -Force -ErrorAction SilentlyContinue
Start-Sleep -Milliseconds 500
pass "serve: clean shutdown"

Write-Host "=== sync conflict scenarios ==="

# --- Two-node setup ---
git init --bare $REMOTE_DIR 2>&1 | Out-Null

# node1: init + push
Push-Location $NODE1_DIR
ddb init . | Out-Null
git remote add origin $REMOTE_DIR
ddb register-node "Laptop" | Out-Null

# 21. fast-forward sync
$SYNC_ID = ddb create --title "Shared note" --tags "shared" --body "Original body"
git push -u origin master 2>&1 | Out-Null

# clone to node2
git clone $REMOTE_DIR $NODE2_DIR 2>&1 | Out-Null
Push-Location $NODE2_DIR
ddb reindex | Out-Null
ddb register-node "Desktop" | Out-Null

$output = ddb read $SYNC_ID
if ($output -notmatch "Shared note") { throw "fast-forward failed" }
pass "fast-forward sync"

# 22. non-overlapping edits
Pop-Location  # back to NODE1_DIR
ddb update $SYNC_ID --title "Updated Title" --tags "shared,laptop"

Push-Location $NODE2_DIR
ddb update $SYNC_ID --body "Modified body"

Pop-Location  # back to NODE1_DIR
ddb sync origin master | Out-Null

Push-Location $NODE2_DIR
$output = ddb sync origin master
if ($output -notmatch "conflicts resolved: 0") { throw "expected 0 conflicts" }

$output = ddb read $SYNC_ID
if ($output -notmatch "Updated Title") { throw "title not merged" }
if ($output -notmatch "Modified body") { throw "body not merged" }
pass "non-overlapping edits (clean merge)"

# 23. frontmatter scalar conflict (title)
Pop-Location  # NODE1_DIR
ddb sync origin master | Out-Null
ddb update $SYNC_ID --title "Laptop Title"

Push-Location $NODE2_DIR
ddb update $SYNC_ID --title "Desktop Title"

Pop-Location  # NODE1_DIR
ddb sync origin master | Out-Null

Push-Location $NODE2_DIR
$output = ddb sync origin master
if ($output -notmatch "conflicts resolved: 1") { throw "expected 1 conflict" }

$title = ((ddb read $SYNC_ID) -split "\r?\n") | Select-String "^title:"
if ($title -notmatch "(Laptop Title|Desktop Title)") { throw "title not resolved" }
pass "frontmatter scalar conflict (CRDT)"

# 24. frontmatter list conflict (tags)
Pop-Location  # NODE1_DIR
ddb sync origin master | Out-Null
ddb update $SYNC_ID --tags "base,alpha"

Push-Location $NODE2_DIR
ddb update $SYNC_ID --tags "base,beta"

Pop-Location  # NODE1_DIR
ddb sync origin master | Out-Null

Push-Location $NODE2_DIR
ddb sync origin master | Out-Null

$output = ddb read $SYNC_ID
if ($output -notmatch "alpha") { throw "alpha tag missing" }
if ($output -notmatch "beta") { throw "beta tag missing" }
pass "frontmatter list conflict (tag union)"

# 25. body conflict
Pop-Location  # NODE1_DIR
ddb sync origin master | Out-Null
ddb update $SYNC_ID --body "Line one LAPTOP.`nLine two.`nLine three."

Push-Location $NODE2_DIR
ddb update $SYNC_ID --body "Line one.`nLine two DESKTOP.`nLine three."

Pop-Location  # NODE1_DIR
ddb sync origin master | Out-Null

Push-Location $NODE2_DIR
$output = ddb sync origin master
if ($output -notmatch "conflicts resolved: 1") { throw "expected 1 conflict" }

$output = ddb read $SYNC_ID
if ($output -notmatch "LAPTOP") { throw "LAPTOP missing" }
if ($output -notmatch "DESKTOP") { throw "DESKTOP missing" }
pass "body conflict (CRDT text merge)"

# 26. reference section conflict
Pop-Location  # NODE1_DIR
ddb sync origin master | Out-Null

$DOOGAT_FILE = "ddb/${SYNC_ID}.md"

$content = Get-Content $DOOGAT_FILE -Raw
Set-Content $DOOGAT_FILE -Value "$content`n---`n- laptop note:: Added from laptop`n" -NoNewline
git add $DOOGAT_FILE
git commit -m "node1 add reference" 2>&1 | Out-Null
git push origin master 2>&1 | Out-Null

Push-Location $NODE2_DIR
$content = Get-Content $DOOGAT_FILE -Raw
Set-Content $DOOGAT_FILE -Value "$content`n---`n- desktop note:: Added from desktop`n" -NoNewline
git add $DOOGAT_FILE
git commit -m "node2 add reference" 2>&1 | Out-Null

$output = ddb sync origin master
if ($output -notmatch "conflicts resolved: 1") { throw "expected 1 conflict" }

$output = ddb read $SYNC_ID
if ($output -notmatch "laptop note") { throw "laptop note missing" }
if ($output -notmatch "desktop note") { throw "desktop note missing" }
pass "reference section conflict (CRDT union)"

# 27b. delete-vs-edit conflict
Pop-Location  # NODE1_DIR
ddb sync origin master | Out-Null
$DEL_ID = ddb create --title "To be deleted" --body "Original content"
ddb sync origin master | Out-Null

Push-Location $NODE2_DIR
ddb sync origin master | Out-Null
$output = ddb read $DEL_ID
if ($output -notmatch "To be deleted") { throw "pre-delete read failed" }

# node1 deletes, node2 edits
Pop-Location  # NODE1_DIR
ddb delete $DEL_ID

Push-Location $NODE2_DIR
ddb update $DEL_ID --body "Edited on desktop"

Pop-Location  # NODE1_DIR
ddb sync origin master | Out-Null

Push-Location $NODE2_DIR
ddb sync origin master | Out-Null

$output = ddb read $DEL_ID
if ($output -notmatch "Edited on desktop") { throw "edit-wins failed" }
$output = ddb status
if ($output -notmatch "resurrected") { throw "resurrected missing" }
pass "delete-vs-edit conflict (edit wins, resurrected)"

Write-Host "=== id collision detection ==="

# 27a. add-add collision: both doogats survive
$COLL_REMOTE = New-Item -ItemType Directory -Path "$env:TEMP\ddb-coll-remote-$(Get-Random)" -Force
git init --bare $COLL_REMOTE 2>&1 | Out-Null

$COLL_A = New-Item -ItemType Directory -Path "$env:TEMP\ddb-coll-a-$(Get-Random)" -Force
Push-Location $COLL_A
ddb init . | Out-Null
git remote add origin $COLL_REMOTE
ddb register-node "CollA" | Out-Null
git push -u origin master 2>&1 | Out-Null

$COLL_B_PARENT = "$env:TEMP\ddb-coll-b-$(Get-Random)"
git clone $COLL_REMOTE $COLL_B_PARENT 2>&1 | Out-Null
Push-Location $COLL_B_PARENT
ddb reindex | Out-Null
ddb register-node "CollB" | Out-Null
git push origin master 2>&1 | Out-Null

# Sync A to pick up B's node
Pop-Location  # back to COLL_A
ddb sync origin master | Out-Null

# Both create same-ID doogat independently
$COLL_ID = "20260101120000"
$collContent = "---`nid: $COLL_ID`ntitle: From A`ndate: 2026-01-01`n---`nBody A`n"
New-Item -ItemType Directory -Path "ddb" -Force | Out-Null
Set-Content -Path "ddb/$COLL_ID.md" -Value $collContent -NoNewline
git add "ddb/$COLL_ID.md" 2>&1 | Out-Null
git commit -m "A creates $COLL_ID" 2>&1 | Out-Null
git push origin master 2>&1 | Out-Null

Push-Location $COLL_B_PARENT
$collContentB = "---`nid: $COLL_ID`ntitle: From B`ndate: 2026-01-01`n---`nBody B`n"
New-Item -ItemType Directory -Path "ddb" -Force | Out-Null
Set-Content -Path "ddb/$COLL_ID.md" -Value $collContentB -NoNewline
git add "ddb/$COLL_ID.md" 2>&1 | Out-Null
git commit -m "B creates $COLL_ID" 2>&1 | Out-Null

$collOut = ddb sync origin master
if ($collOut -notmatch "collisions reassigned: 1") { throw "collision not detected: $collOut" }
$collFiles = @(Get-ChildItem "ddb/*.md" | Where-Object { $_.Name -notmatch "^_" })
if ($collFiles.Count -ne 2) { throw "expected 2 doogats after collision, got $($collFiles.Count)" }
pass "add-add collision: both doogats survive"
Pop-Location  # leave COLL_B

Pop-Location  # leave COLL_A

Write-Host "=== bundle sync ==="

# 27. bundle export --full + import
Pop-Location  # NODE1_DIR
ddb sync origin master | Out-Null
ddb bundle export --full --output (Join-Path $TMPDIR "full-bundle.tar") | Out-Null

Push-Location $NODE3_DIR
ddb init . | Out-Null
ddb register-node "Tablet" | Out-Null
$output = ddb bundle import (Join-Path $TMPDIR "full-bundle.tar")
if ($output -notmatch "imported") { throw "bundle import failed" }
$output = ddb read $SYNC_ID
if ($output -notmatch "laptop note") { throw "bundle content missing" }
pass "bundle export --full + import"

# 28. delta bundle export + import
Pop-Location  # NODE1_DIR
$DELTA_ID = ddb create --title "Delta note" --body "only in delta"

$NODE2_UUID = Get-Content (Join-Path $NODE2_DIR ".git" "ddb-node") -Raw
$NODE2_UUID = $NODE2_UUID.Trim()
ddb bundle export --target $NODE2_UUID --output (Join-Path $TMPDIR "delta-bundle.tar") | Out-Null

Push-Location $NODE2_DIR
$output = ddb bundle import (Join-Path $TMPDIR "delta-bundle.tar")
if ($output -notmatch "imported") { throw "delta import failed" }
$output = ddb read $DELTA_ID
if ($output -notmatch "Delta note") { throw "delta content missing" }
pass "delta bundle export + import"

# 29. update-bin help + rollback
$output = ddb update-bin --help
if ($output -notmatch "Update ddb") { throw "update-bin help failed" }
if ($output -notmatch "--rollback") { throw "update-bin help missing --rollback" }
pass "update-bin --help (includes --rollback)"

# rollback with no backup should fail gracefully (expected failure — bypass ddb wrapper)
$rollbackOutput = & $DDB update-bin --rollback 2>&1 | ForEach-Object { "$_" }
if (($rollbackOutput -join "`n") -notmatch "no backup") { throw "update-bin --rollback should report no backup" }
pass "update-bin --rollback (no backup error)"

# 30. ALTER TABLE + DROP TABLE + bulk UPDATE/DELETE
Pop-Location  # back to TMPDIR
Push-Location $TMPDIR

$output = ddb query "CREATE TABLE smokealt (name TEXT, score INTEGER)"
if ($output -notmatch "table smokealt created") { throw "create smokealt failed" }
ddb query "INSERT INTO smokealt (name, score) VALUES ('a', 1)" | Out-Null
Start-Sleep -Seconds 1
ddb query "INSERT INTO smokealt (name, score) VALUES ('b', 2)" | Out-Null
$output = ddb query "ALTER TABLE smokealt ADD COLUMN tag TEXT"
if ($output -notmatch "altered") { throw "alter add failed" }
$output = ddb query "SELECT name, tag FROM smokealt"
if ($output -notmatch "NULL") { throw "null check failed" }
$output = ddb query "ALTER TABLE smokealt RENAME COLUMN tag TO label"
if ($output -notmatch "renamed") { throw "alter rename failed" }
$output = ddb query "SELECT name, label FROM smokealt"
if ($output -notmatch "a") { throw "select after rename failed" }
$output = ddb query "UPDATE smokealt SET score = 99 WHERE name = 'a'"
if ($output -notmatch "1 row\(s\) affected") { throw "bulk update failed" }
$output = ddb query "DELETE FROM smokealt WHERE name = 'b'"
if ($output -notmatch "1 row\(s\) affected") { throw "bulk delete failed" }
$output = ddb query "DROP TABLE smokealt CASCADE"
if ($output -notmatch "dropped") { throw "drop table failed" }
pass "alter/drop table + bulk ops"

# UPDATE/DELETE WHERE id no-match semantics (continuation of section 30, #5)
$output = ddb query "CREATE TABLE smokenomatch (name TEXT, score INTEGER)"
if ($output -notmatch "table smokenomatch created") { throw "create smokenomatch failed" }
$nomatchId = ddb query "INSERT INTO smokenomatch (name, score) VALUES ('alpha', 1)"
# B1: UPDATE with nonexistent id returns 0 rows affected (not an error)
$output = ddb query "UPDATE smokenomatch SET score = 1 WHERE id = 'nonexistent_id_00000000000000'"
if ($output -notmatch "0 row\(s\) affected") { throw "B1 update missing id failed: $output" }
# B2: DELETE with nonexistent id returns 0 rows affected (not an error)
$output = ddb query "DELETE FROM smokenomatch WHERE id = 'nonexistent_id_00000000000000'"
if ($output -notmatch "0 row\(s\) affected") { throw "B2 delete missing id failed: $output" }
# B3: IN clause mixing missing and valid ids still affects 1 row
$output = ddb query "UPDATE smokenomatch SET score = 7 WHERE id IN ('nope', '$nomatchId')"
if ($output -notmatch "1 row\(s\) affected") { throw "B3 IN-clause mixed failed: $output" }
# B4: compound predicate with valid id + non-matching column returns 0 rows affected
$output = ddb query "UPDATE smokenomatch SET score = 99 WHERE id = '$nomatchId' AND name = 'wrongname'"
if ($output -notmatch "0 row\(s\) affected") { throw "B4 compound non-match failed: $output" }
# B5: valid id on the fast path still affects 1 row
$output = ddb query "UPDATE smokenomatch SET score = 42 WHERE id = '$nomatchId'"
if ($output -notmatch "1 row\(s\) affected") { throw "B5 valid fast-path failed: $output" }
$output = ddb query "SELECT score FROM smokenomatch WHERE id = '$nomatchId'"
if ($output -notmatch "42") { throw "B5 materialized score mismatch: $output" }
$output = ddb query "DROP TABLE smokenomatch CASCADE"
if ($output -notmatch "dropped") { throw "drop smokenomatch failed" }
pass "update/delete WHERE id no-match semantics (#5)"

# 30.F1 - composite UNIQUE duplicate rejection surfaces a clear error on the
# CLI (#9 group F1).
$output = ddb query 'CREATE TABLE f1mship (title VARCHAR(255), link_id VARCHAR(255), category VARCHAR(255), UNIQUE(link_id, category))'
if ($output -notmatch "table f1mship created") { throw "f1mship create failed" }
ddb query "INSERT INTO f1mship (title, link_id, category) VALUES ('a', 'link1', 'cat1')" | Out-Null
$f1Dup = & $DDB query "INSERT INTO f1mship (title, link_id, category) VALUES ('b', 'link1', 'cat1')" 2>&1 | Out-String
if ($LASTEXITCODE -eq 0) { throw "30.F1: duplicate INSERT should have failed, got: $f1Dup" }
if ($f1Dup -notmatch "UNIQUE") { throw "30.F1: duplicate INSERT should mention UNIQUE: $f1Dup" }
if ($f1Dup -notmatch "f1mship|link_id|category") { throw "30.F1: error should mention table/col: $f1Dup" }
$output = ddb query "DROP TABLE f1mship CASCADE"
if ($output -notmatch "dropped") { throw "f1mship drop failed" }
pass "issue-9-F1: composite UNIQUE duplicate rejected with clear error"

# 31. file attachments
$attachFile = Join-Path $TMPDIR "ddb-smoke-attach.txt"
Set-Content $attachFile -Value "hello attachment"
$output = ddb attach $ID1 $attachFile
if ($output -notmatch "attached") { throw "attach failed" }
$output = ddb attachments $ID1
if ($output -notmatch "ddb-smoke-attach.txt") { throw "attachments list failed" }
if ($output -notmatch "text/plain") { throw "mime type wrong" }
$output = ddb query "SELECT name, mime FROM _ddb_attachments WHERE doogat_id = '$ID1'"
if ($output -notmatch "ddb-smoke-attach.txt") { throw "attach query failed" }
$output = ddb detach $ID1 "ddb-smoke-attach.txt"
if ($output -notmatch "detached") { throw "detach failed" }
$output = ddb attachments $ID1
if ($output -notmatch "no attachments") { throw "post-detach failed" }
Remove-Item $attachFile -ErrorAction SilentlyContinue
pass "file attachments (attach/list/query/detach)"

# 32. NoSQL CLI commands
$output = ddb get $ID1
if ($output -notmatch "First note \(edited\)") { throw "nosql get failed" }
pass "nosql: get"

$output = ddb scan --tag test
if ($output -notmatch $ID1) { throw "nosql scan --tag failed" }
pass "nosql: scan --tag"

$output = ddb scan --type foo
if ($output -notmatch "(?m)^\d{14}$") { throw "nosql scan --type failed" }
pass "nosql: scan --type"

$output = ddb backlinks $ID1
if ($output -notmatch $ID2) { throw "nosql backlinks failed" }
pass "nosql: backlinks"

# 33. stale node resync after compaction
Write-Host "=== stale node resync ==="
$script:STALE_REMOTE = New-TempDir
$script:STALE_N1 = New-TempDir
$script:STALE_N2 = New-TempDir

git init --bare $script:STALE_REMOTE 2>&1 | Out-Null

Push-Location $script:STALE_N1
ddb init . | Out-Null
git remote add origin $script:STALE_REMOTE
ddb register-node "StaleNode1" | Out-Null
$STALE_ID = ddb create --title "Stale shared" --body "original content"
git push -u origin master 2>&1 | Out-Null

git clone $script:STALE_REMOTE $script:STALE_N2 2>&1 | Out-Null
Push-Location $script:STALE_N2
ddb reindex | Out-Null
ddb register-node "StaleNode2" | Out-Null

# Both nodes edit the same doogat
Pop-Location  # STALE_N1
ddb update $STALE_ID --body "body from node1"
git push origin master 2>&1 | Out-Null

Push-Location $script:STALE_N2
ddb update $STALE_ID --body "body from node2"
ddb sync origin master | Out-Null

# Compact to remove CRDT temp files — verify report includes byte stats
$compactOut = ddb compact --force
if ($compactOut -notmatch "crdt temp:") { throw "compact missing crdt temp stats in stale test" }
if ($compactOut -notmatch "repo \(\.git\):") { throw "compact missing repo stats in stale test" }

# Create another conflict without CRDT state
Pop-Location  # STALE_N1
ddb sync origin master | Out-Null
ddb update $STALE_ID --body "second edit node1"
git push origin master 2>&1 | Out-Null

Push-Location $script:STALE_N2
ddb update $STALE_ID --body "second edit node2"
ddb sync origin master | Out-Null

# Verify doogat is readable and valid
$output = ddb read $STALE_ID
if ($output -notmatch "title:") { throw "stale resync failed" }
pass "stale node resync after compaction"

# 34. multi-row INSERT
Push-Location $TMPDIR
ddb query "CREATE TABLE multirow (name TEXT, val INTEGER)" | Out-Null
$MULTI_IDS = ddb query "INSERT INTO multirow (name, val) VALUES ('a', 1), ('b', 2), ('c', 3)"
if ($MULTI_IDS -notmatch "(?m)^\d{14},\d{14},\d{14}$") { throw "multi-row insert did not return 3 IDs: $MULTI_IDS" }
$count = ddb query "SELECT COUNT(*) FROM multirow"
if ($count -notmatch "3") { throw "expected 3 rows, got: $count" }
pass "multi-row insert"

# 35. transaction commit + rollback
Push-Location $TMPDIR
ddb query "CREATE TABLE txntest (val TEXT)" | Out-Null
$txnOut = ddb query "BEGIN; INSERT INTO txntest (val) VALUES ('committed'); COMMIT"
if ($txnOut -notmatch "COMMIT") { throw "transaction commit failed: $txnOut" }
$txnSel = ddb query "SELECT val FROM txntest"
if ($txnSel -notmatch "committed") { throw "committed row missing" }
$rbOut = ddb query "BEGIN; INSERT INTO txntest (val) VALUES ('rolled-back'); ROLLBACK"
if ($rbOut -notmatch "ROLLBACK") { throw "rollback failed: $rbOut" }
$txnCount = ddb query "SELECT COUNT(*) FROM txntest"
if ($txnCount -notmatch "1") { throw "expected 1 row after rollback, got: $txnCount" }
pass "transaction commit + rollback"

# 36. hyphenated type SQL via quoted identifiers
Push-Location $TMPDIR
ddb query 'CREATE TABLE "my-type" (label TEXT)' | Out-Null
$MY_ID = ddb query "INSERT INTO `"my-type`" (label) VALUES ('test')"
$hSel = ddb query "SELECT label FROM `"my-type`""
if ($hSel -notmatch "test") { throw "hyphenated select failed" }
$hDel = ddb query "DELETE FROM `"my-type`" WHERE id = '$MY_ID'"
if ($hDel -notmatch "1 row") { throw "hyphenated delete failed: $hDel" }
pass "hyphenated type SQL"

# 37. junction table CRUD (multi-value references)
Push-Location $TMPDIR
ddb query "CREATE TABLE jtag (name VARCHAR(100))" | Out-Null
ddb query "CREATE TABLE jpost (url TEXT, jtag TEXT REFERENCES jtag)" | Out-Null
$JT_TAG_ID = ddb query "INSERT INTO jtag (name) VALUES ('rust')"
Start-Sleep -Seconds 1
$JT_POST_ID = ddb query "INSERT INTO jpost (url) VALUES ('https://example.com')"
$jtIns = ddb query "INSERT INTO jpost_jtag (jpost_id, jtag_id) VALUES ('$JT_POST_ID', '$JT_TAG_ID')"
if ($jtIns -notmatch "1 row") { throw "junction insert failed: $jtIns" }
$jtSel = ddb query "SELECT jtag_id FROM jpost_jtag WHERE jpost_id = '$JT_POST_ID'"
if ($jtSel -notmatch $JT_TAG_ID) { throw "junction select failed: $jtSel" }
$jtDel = ddb query "DELETE FROM jpost_jtag WHERE jpost_id = '$JT_POST_ID' AND jtag_id = '$JT_TAG_ID'"
if ($jtDel -notmatch "1 row") { throw "junction delete failed: $jtDel" }
$jtCount = ddb query "SELECT COUNT(*) FROM jpost_jtag"
if ($jtCount -notmatch "0") { throw "junction not empty after delete: $jtCount" }
ddb query "INSERT INTO jpost_jtag (jpost_id, jtag_id) VALUES ('$JT_POST_ID', '$JT_TAG_ID')" | Out-Null
$jtDrop = ddb query "DROP TABLE jpost CASCADE"
if ($jtDrop -notmatch "dropped") { throw "cascade drop failed: $jtDrop" }
if (-not (ddb-fails query "SELECT * FROM jpost_jtag")) { throw "junction table should not exist after cascade" }
pass "junction table CRUD"

# 38. title template compliance check
Push-Location $TMPDIR
ddb query "CREATE TABLE smwidget (name VARCHAR(100), description TEXT)" | Out-Null
ddb query "ALTER TABLE smwidget SET TITLE TEMPLATE '{name} Widget'" | Out-Null
ddb query "INSERT INTO smwidget (name, description) VALUES ('Foo', 'A foo widget')" | Out-Null
$fixVerbose = ddb fix --verbose --dry-run
if ($fixVerbose -notmatch "title does not match template") { throw "title compliance not reported" }
pass "title template compliance check"

# 39. zone migration
Push-Location $TMPDIR
ddb query "CREATE TABLE gadget (notes TEXT)" | Out-Null
ddb query "INSERT INTO gadget (notes) VALUES ('Some notes')" | Out-Null
ddb query "ALTER TABLE gadget SET ZONE frontmatter FOR notes" | Out-Null
$fixMigrate = ddb fix --migrate --verbose
if ($fixMigrate -notmatch "zone-migrated") { throw "zone migration not reported: $fixMigrate" }
pass "zone migration"

# 40. help guides
$helpOut = ddb help create-app
if ($helpOut -notmatch "CREATE TABLE") { throw "help create-app missing CREATE TABLE" }
pass "help create-app"
$helpList = ddb help
if ($helpList -notmatch "create-app") { throw "help list missing create-app" }
pass "help list"
$helpFail = $null
try { ddb help nonexistent 2>$null } catch { $helpFail = $true }
if (-not $helpFail) {
    # Also check exit code via $LASTEXITCODE for native commands
    if ($LASTEXITCODE -eq 0) { throw "help unknown should fail" }
}
pass "help unknown fails"

# 40. binary asset LWW conflict resolution
Write-Host "=== binary asset LWW ==="
$BIN_REMOTE = New-TempDir
$BIN_NODE1 = New-TempDir
$BIN_NODE2 = New-TempDir
git init --bare $BIN_REMOTE 2>$null | Out-Null

Push-Location $BIN_NODE1
ddb init . | Out-Null
git remote add origin $BIN_REMOTE
ddb register-node "BinNode1" | Out-Null
$binId = ddb create --title "Binary test"
New-Item -ItemType Directory -Force -Path "reference/test" | Out-Null
[System.IO.File]::WriteAllBytes("$BIN_NODE1/reference/test/photo.bin", [byte[]](0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A))
git add reference/test/photo.bin
git commit -m "add binary asset" 2>$null | Out-Null
git push -u origin master 2>$null | Out-Null

git clone $BIN_REMOTE $BIN_NODE2 2>$null | Out-Null
Push-Location $BIN_NODE2
ddb reindex | Out-Null
ddb register-node "BinNode2" | Out-Null
Pop-Location

# Node1: modify binary with higher HLC
[System.IO.File]::WriteAllText("$BIN_NODE1/reference/test/photo.bin", "NODE1_WINS_CONTENT")
git add reference/test/photo.bin
git commit -m "node1 update binary`n`nddb-hlc: 9999999999999.0.BinNode1" 2>$null | Out-Null
git push origin master 2>$null | Out-Null

# Node2: modify same binary with lower HLC
Push-Location $BIN_NODE2
[System.IO.File]::WriteAllText("$BIN_NODE2/reference/test/photo.bin", "NODE2_LOSES_CONTENT")
git add reference/test/photo.bin
git commit -m "node2 update binary`n`nddb-hlc: 1000000000000.0.BinNode2" 2>$null | Out-Null

$syncOut = ddb sync origin master
if ($syncOut -notmatch "conflicts resolved: 1") { throw "expected 1 conflict resolved: $syncOut" }
$resolved = [System.IO.File]::ReadAllText("$BIN_NODE2/reference/test/photo.bin")
if ($resolved -ne "NODE1_WINS_CONTENT") { throw "LWW winner wrong: $resolved" }
pass "binary asset LWW (higher HLC wins)"

$mergeLog = git log --merges --oneline -1
if ($mergeLog -notmatch "resolve merge") { throw "no merge commit found: $mergeLog" }
pass "binary asset LWW (loser preserved in history)"
Pop-Location
Pop-Location

Remove-Item -Recurse -Force $BIN_REMOTE, $BIN_NODE1, $BIN_NODE2

# 41. auto-register node on first sync
$AR_REMOTE = New-TempDir
$AR_NODE = New-TempDir
git init --bare $AR_REMOTE 2>$null | Out-Null
ddb init $AR_NODE | Out-Null
Push-Location $AR_NODE
git remote add origin $AR_REMOTE
git push -u origin master 2>$null | Out-Null

# No register-node — sync should auto-register
if (Test-Path ".git/ddb-node") { throw "node file should not exist before sync" }
ddb sync origin master | Out-Null
if (-not (Test-Path ".git/ddb-node")) { throw "auto-register should create .git/ddb-node" }
pass "auto-register node on first sync"

# Subsequent sync reuses registration
$uuidBefore = Get-Content ".git/ddb-node"
ddb sync origin master | Out-Null
$uuidAfter = Get-Content ".git/ddb-node"
if ($uuidBefore -ne $uuidAfter) { throw "existing registration should be reused" }
pass "auto-register reuses existing registration"

Pop-Location
Remove-Item -Recurse -Force $AR_REMOTE, $AR_NODE

# Return to original directory
Set-Location $TMPDIR

# 46. title_template REFERENCES resolution (PRD 00127).
# Server has been down since section 28, so use the CLI directly.
Write-Host "=== title_template REFERENCES resolution ==="
$TT_DIR = New-Item -ItemType Directory -Path (Join-Path $env:TEMP ([guid]::NewGuid().ToString())) | ForEach-Object { $_.FullName }
Push-Location $TT_DIR
ddb init | Out-Null
ddb query "CREATE TABLE tt_link (url TEXT)" | Out-Null
ddb query "CREATE TABLE tt_category (fqn TEXT)" | Out-Null
ddb query "CREATE TABLE tt_membership (link TEXT REFERENCES tt_link, category TEXT REFERENCES tt_category)" | Out-Null
ddb query "ALTER TABLE tt_membership SET TITLE TEMPLATE '{link.title} in {category.fqn}'" | Out-Null
pass "46: declared dotted title_template"

$ttLinkId = (ddb query "INSERT INTO tt_link (title, url) VALUES ('My Link', 'https://x')").Trim()
Start-Sleep -Seconds 1
$ttCatId = (ddb query "INSERT INTO tt_category (title, fqn) VALUES ('Cat', 'A/B')").Trim()
Start-Sleep -Seconds 1
$ttMemId = (ddb query "INSERT INTO tt_membership (link, category) VALUES ('$ttLinkId', '$ttCatId')").Trim()
$titleRow = ddb query "SELECT title FROM tt_membership WHERE id = '$ttMemId'"
if ($titleRow -notmatch "My Link in A/B") { throw "46: composed title missing: $titleRow" }
pass "46: composed title 'My Link in A/B' from REFERENCES"

# Call $DDB directly (the `ddb` wrapper throws on non-zero exit) and flatten
# stderr+stdout with Out-String so -match operates on a single string instead
# of a per-line array (where -notmatch returns non-matching elements, not a
# bool).
$badOut = & $DDB query "ALTER TABLE tt_membership SET TITLE TEMPLATE '{link.does_not_exist}'" 2>&1 | Out-String
if ($badOut -notmatch "does not exist on tt_link") { throw "46: bad path was not rejected: $badOut" }
pass "46: ALTER TABLE rejects bad dotted path"

Pop-Location
Remove-Item -Recurse -Force $TT_DIR

# 47. ALTER TABLE ALTER COLUMN TYPE (PRD 00128).
Write-Host "=== ALTER TABLE ALTER COLUMN TYPE ==="
$AC_DIR = New-Item -ItemType Directory -Path (Join-Path $env:TEMP ([guid]::NewGuid().ToString())) | ForEach-Object { $_.FullName }
Push-Location $AC_DIR
ddb init | Out-Null
ddb query "CREATE TABLE ac_link (url VARCHAR(32))" | Out-Null

$acShort = 'a' * 32
$acId1 = (ddb query "INSERT INTO ac_link (title, url) VALUES ('boundary', '$acShort')").Trim()
if (-not $acId1) { throw "47: expected boundary insert to return an id" }
pass "47: baseline VARCHAR(32) insert at boundary"

$acLong = 'b' * 80
$acFailOut = & $DDB query "INSERT INTO ac_link (title, url) VALUES ('toolong', '$acLong')" 2>&1 | Out-String
if ($acFailOut -notmatch "exceeds limit") { throw "47: pre-ALTER 80-char insert should have been rejected: $acFailOut" }
pass "47: pre-ALTER INSERT rejects 80-char value for VARCHAR(32)"

ddb query "ALTER TABLE ac_link ALTER COLUMN url TYPE VARCHAR(100)" | Out-Null
pass "47: widen VARCHAR(32) -> VARCHAR(100) succeeds"

Start-Sleep -Seconds 1
$acId2 = (ddb query "INSERT INTO ac_link (title, url) VALUES ('now-ok', '$acLong')").Trim()
if (-not $acId2) { throw "47: post-ALTER 80-char insert should succeed" }
pass "47: post-ALTER INSERT accepts 80-char value"

$acNarrowOut = & $DDB query "ALTER TABLE ac_link ALTER COLUMN url TYPE VARCHAR(5)" 2>&1 | Out-String
if ($acNarrowOut -notmatch "cannot narrow") { throw "47: narrowing should emit 'cannot narrow': $acNarrowOut" }
if ($acNarrowOut -notmatch "existing rows exceed limit") { throw "47: narrowing should include row-count message: $acNarrowOut" }
pass "47: narrowing rejects with cannot-narrow row-count message"

ddb query "ALTER TABLE ac_link ALTER COLUMN url TYPE TEXT" | Out-Null
Start-Sleep -Seconds 1
$acHuge = 'c' * 2000
$acId3 = (ddb query "INSERT INTO ac_link (title, url) VALUES ('text-row', '$acHuge')").Trim()
if (-not $acId3) { throw "47: 2000-char insert should succeed after widening to TEXT" }
pass "47: VARCHAR -> TEXT widening persists and accepts long values"

Pop-Location
Remove-Item -Recurse -Force $AC_DIR

# 48. PRD 00129: typed write blockers + ON DELETE CASCADE + INDEX no-op.
Write-Host "=== PRD 00129: typed write blockers + ON DELETE CASCADE + INDEX no-op ==="
$P9_DIR = New-TempDir
Push-Location $P9_DIR
ddb init | Out-Null

# §3b: CREATE INDEX IF NOT EXISTS is accepted as a no-op.
ddb query "CREATE TABLE p9_link (title TEXT, url VARCHAR(255))" | Out-Null
$p9IndexOut = ddb query "CREATE INDEX IF NOT EXISTS idx_p9_url ON p9_link(url)" 2>&1
if ($p9IndexOut -notmatch "ignored") { throw "48: CREATE INDEX IF NOT EXISTS should emit 'ignored': $p9IndexOut" }
pass "48: CREATE INDEX IF NOT EXISTS accepted as no-op"

$p9PlainOut = & $DDB query "CREATE INDEX idx_plain ON p9_link(url)" 2>&1 | Out-String
if ($p9PlainOut -notmatch "CREATE INDEX not supported") { throw "48: plain CREATE INDEX should reject: $p9PlainOut" }
pass "48: plain CREATE INDEX still rejects"

# §2: ON DELETE CASCADE walks one level.
ddb query "CREATE TABLE p9_membership (title TEXT, link VARCHAR(255) REFERENCES p9_link(id) ON DELETE CASCADE)" | Out-Null
$p9LinkId = (ddb query "INSERT INTO p9_link (title, url) VALUES ('Parent', 'https://x')").Trim()
Start-Sleep -Seconds 1
$p9MemId = (ddb query "INSERT INTO p9_membership (title, link) VALUES ('Child', '$p9LinkId')").Trim()
if (-not $p9MemId) { throw "48: typed insert into cascade-bound child failed" }
pass "48: typed insert into cascade-bound child succeeds"

ddb delete $p9LinkId | Out-Null
$p9AfterLink = ddb query "SELECT id FROM p9_link WHERE id = '$p9LinkId'" 2>&1
$p9AfterMem = ddb query "SELECT id FROM p9_membership WHERE id = '$p9MemId'" 2>&1
if ($p9AfterLink -match $p9LinkId) { throw "48: parent should be gone after CASCADE delete" }
if ($p9AfterMem -match $p9MemId) { throw "48: child should be gone after CASCADE delete" }
pass "48: ON DELETE CASCADE removes parent and child in one delete"

# §2: ON DELETE RESTRICT (default) blocks parent delete.
ddb query "CREATE TABLE p9_blocker (title TEXT, link VARCHAR(255) NOT NULL REFERENCES p9_link(id))" | Out-Null
$p9LinkId2 = (ddb query "INSERT INTO p9_link (title, url) VALUES ('R Parent', 'https://r')").Trim()
Start-Sleep -Seconds 1
ddb query "INSERT INTO p9_blocker (title, link) VALUES ('Block', '$p9LinkId2')" | Out-Null
$p9RestrictOut = & $DDB delete $p9LinkId2 2>&1 | Out-String
if ($p9RestrictOut -notmatch "NOT NULL REFERENCES from p9_blocker.link") { throw "48: RESTRICT should block parent delete: $p9RestrictOut" }
pass "48: ON DELETE RESTRICT (default) rejects parent delete"

# §2: cascade cycle detection.
ddb query "CREATE TABLE p9_a (title TEXT)" | Out-Null
ddb query "CREATE TABLE p9_b (title TEXT)" | Out-Null
ddb query "ALTER TABLE p9_a ADD COLUMN b VARCHAR(255) REFERENCES p9_b(id) ON DELETE CASCADE" | Out-Null
ddb query "ALTER TABLE p9_b ADD COLUMN a VARCHAR(255) REFERENCES p9_a(id) ON DELETE CASCADE" | Out-Null
$p9AId = (ddb query "INSERT INTO p9_a (title) VALUES ('A')").Trim()
Start-Sleep -Seconds 1
$p9BId = (ddb query "INSERT INTO p9_b (title) VALUES ('B')").Trim()
ddb query "UPDATE p9_a SET b = '$p9BId' WHERE id = '$p9AId'" | Out-Null
ddb query "UPDATE p9_b SET a = '$p9AId' WHERE id = '$p9BId'" | Out-Null
$p9CycleOut = & $DDB delete $p9AId 2>&1 | Out-String
if ($p9CycleOut -notmatch "cascade delete would form a cycle") { throw "48: cycle should be detected: $p9CycleOut" }
pass "48: ON DELETE CASCADE cycle detection rejects"

Pop-Location
Remove-Item -Recurse -Force $P9_DIR

# 49. PRD 00133 unify-typed-write-paths: CLI typed `create` on a registered
# typedef must populate the reference zone for REFERENCES columns and reject
# FK to wrong-type ids. Unit-test suite covers GraphQL parity end-to-end
# (`service::tests::batch_create_*`, `create_doogat_with_extra_*`); this
# section drives the CLI/FFI surface so cross-shell regressions surface in CI.
Write-Host "=== PRD 00133: unified typed-write paths (CLI parity) ==="
$TW_DIR = New-TemporaryDirectory
Push-Location $TW_DIR
ddb init | Out-Null
ddb query "CREATE TABLE tw_category (label VARCHAR(64))" | Out-Null
$twCat1 = (ddb query "INSERT INTO tw_category (title, label) VALUES ('c1', 'alpha')").Trim()
Start-Sleep -Seconds 1
$twCat2 = (ddb query "INSERT INTO tw_category (title, label) VALUES ('c2', 'beta')").Trim()
if (-not $twCat1 -or -not $twCat2) { throw "49: failed to seed tw_category rows" }
pass "49: junction typedef setup with two category rows"

ddb query "CREATE TABLE tw_membership (link VARCHAR(64) REFERENCES tw_category, parent VARCHAR(64) REFERENCES tw_category)" | Out-Null
Start-Sleep -Seconds 1
$twMemId = (ddb create --type tw_membership --title "M1" --set "link=$twCat1" --set "parent=$twCat2").Trim()
if (-not $twMemId) { throw "49: CLI create on junction typedef returned empty id" }
pass "49: CLI create on junction-style typedef succeeds"

$twRaw = (ddb get $twMemId | Out-String)
if ($twRaw -notmatch [regex]::Escape("link:: [[$twCat1]]")) { throw "49: link reference zone missing: $twRaw" }
if ($twRaw -notmatch [regex]::Escape("parent:: [[$twCat2]]")) { throw "49: parent reference zone missing: $twRaw" }
pass "49: CLI create populates reference zone for REFERENCES columns"

ddb query "CREATE TABLE tw_link (label VARCHAR(64))" | Out-Null
Start-Sleep -Seconds 1
$twLinkId = (ddb query "INSERT INTO tw_link (title, label) VALUES ('not a category', 'plain')").Trim()
if (-not $twLinkId) { throw "49: tw_link seed failed" }
$twBadOut = (& $DDB create --type tw_membership --title "Bogus" --set "link=$twLinkId" --set "parent=$twCat1" 2>&1 | Out-String)
if ($twBadOut -notmatch "references non-existent tw_category") { throw "49: expected wrong-type FK rejection, got: $twBadOut" }
pass "49: CLI create rejects FK to wrong-type id"

Pop-Location
Remove-Item -Recurse -Force $TW_DIR

# 44.L - CLI ddb create populates auto-junction atomically (PRD 00134
# cycle-1 review C1 task #4). Mirrors bash section 44.L through the CLI/FFI
# entry point so the typed materialization path is exercised end-to-end
# across surfaces. Unit tests cover the service-layer typed-create.
$J134L_DIR = New-TempDir
Push-Location $J134L_DIR
ddb init | Out-Null
ddb query "CREATE TABLE j134l_cat (label VARCHAR(64))" | Out-Null
Start-Sleep -Seconds 1
$j134lCat = (ddb query "INSERT INTO j134l_cat (title, label) VALUES ('lcat', 'alpha')").Trim()
if (-not $j134lCat) { throw "44.L: failed to seed j134l_cat" }
ddb query "CREATE TABLE j134l_bm (url VARCHAR(200), category VARCHAR(64) REFERENCES j134l_cat)" | Out-Null
Start-Sleep -Seconds 1
$j134lBm = (ddb create --type j134l_bm --title "L1" --set "url=https://l.example" --set "category=$j134lCat").Trim()
if (-not $j134lBm) { throw "44.L: ddb create did not return id" }
$j134lJoin = ddb query "SELECT bm.id FROM j134l_bm bm JOIN j134l_bm_category j ON j.j134l_bm_id = bm.id WHERE j.category_id = '$j134lCat'"
if ($j134lJoin -notmatch [regex]::Escape($j134lBm)) { throw "44.L: JOIN did not return the bookmark via auto-junction: $j134lJoin" }
pass "PRD 00134: CLI ddb create populates auto-junction for REFERENCES column (44.L)"
Pop-Location
Remove-Item -Recurse -Force $J134L_DIR

# 50. PRD 00136 / #16 - cross-process FK freshness on `ddb create`.
# The bug shape: process 1 creates a typed parent via `ddb create`, then
# process 2 (a fresh DoogatService) creates a typed child whose FK
# references the parent. Pre-fix, process 2's stale SqlEngine rejected
# with REFERENCES_VIOLATION even though the parent existed in git and in
# the global doogats index. Distinct from 49 / 44.L (which seed parents
# via ddb query "INSERT INTO ..."); only the back-to-back CLI ddb create
# shape reproduces the pre-PRD-00136 stale-index FK rejection.
#
# PowerShell's $ErrorActionPreference = "Stop" does NOT apply to native
# executables, so each ddb invocation needs an explicit $LASTEXITCODE
# check or the test would pass vacuously on a regression.
$INT136_DIR = New-TempDir
Push-Location $INT136_DIR
ddb init | Out-Null
if ($LASTEXITCODE -ne 0) { throw "50: ddb init failed" }
ddb query "CREATE TABLE int136cat (fqn VARCHAR(255))" | Out-Null
if ($LASTEXITCODE -ne 0) { throw "50: int136cat create failed" }
ddb query "CREATE TABLE int136link (url TEXT, category TEXT REFERENCES int136cat)" | Out-Null
if ($LASTEXITCODE -ne 0) { throw "50: int136link create failed" }
Start-Sleep -Seconds 1
$int136Cat = (ddb create --type int136cat --title "Cat 136" --set "fqn=test.fqn").Trim()
if ($LASTEXITCODE -ne 0) { throw "50: ddb create int136cat exited $LASTEXITCODE" }
if ($int136Cat -notmatch '^\d{14}$') { throw "50: int136cat id malformed: $int136Cat" }
$int136Link = (ddb create --type int136link --title "Link 136" --set "url=https://a" --set "category=$int136Cat").Trim()
if ($LASTEXITCODE -ne 0) { throw "50: ddb create int136link exited $LASTEXITCODE (FK validation regression?)" }
if ($int136Link -notmatch '^\d{14}$') { throw "50: int136link id malformed: $int136Link" }
pass "PRD 00136 / #16: cross-process FK freshness on ddb create"
Pop-Location
Remove-Item -Recurse -Force $INT136_DIR

Cleanup
Write-Host "=== all integration tests passed ==="
