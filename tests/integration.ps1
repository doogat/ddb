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

# --- Run smoke tests first ---
$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
& "$scriptDir/smoke.ps1"

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
$result = gql '{"query":"{ typeDefs { name } }"}'
if ($result -notmatch '"typeDefs"') { throw "graphql query failed" }
pass "serve: graphql query"

# Test mutation -- create
$result = gql '{"query":"mutation { createDoogat(input: { title: \"Smoke Server\" }) { id title } }"}'
if ($result -notmatch '"Smoke Server"') { throw "graphql create failed" }
$GQL_ID = if ($result -match '"id":"([^"]+)"') { $Matches[1] } else { throw "no id in response" }
pass "serve: graphql create"

# 18. expanded GraphQL operations
$result = gql "{`"query`":`"mutation { updateDoogat(input: { id: \`"$GQL_ID\`", title: \`"Smoke Updated\`" }) { id title } }`"}"
if ($result -notmatch '"Smoke Updated"') { throw "graphql update failed" }
pass "serve: graphql update"

$result = gql '{"query":"{ search(query: \"Smoke\") { totalCount hits { id title } } }"}'
if ($result -notmatch '"search"') { throw "graphql search failed" }
pass "serve: graphql search"

$result = gql '{"query":"{ doogats { id title } }"}'
if ($result -notmatch '"doogats"') { throw "graphql doogats failed" }
pass "serve: graphql doogats"

$result = gql "{`"query`":`"mutation { deleteDoogat(id: \`"$GQL_ID\`") }`"}"
if ($result -notmatch "true") { throw "graphql delete failed" }
pass "serve: graphql delete"

# 18b. GraphQL checkbox queries
$result = gql '{"query":"{ openActions { state content } }"}'
if ($result -notmatch '"openActions"') { throw "graphql openActions failed" }
pass "serve: graphql openActions"

# 18c. GraphQL tag queries
$result = gql '{"query":"mutation { createDoogat(input: { title: \"Tag Test\", tags: [\"alpha\", \"beta\"] }) { id title tags } }"}'
if ($result -notmatch '"alpha"') { throw "graphql create with tags failed" }
$TAG_ID = if ($result -match '"id":"([^"]+)"') { $Matches[1] } else { throw "no tag id in response" }
pass "serve: graphql create with tags"

$result = gql '{"query":"{ tags { name count } }"}'
if ($result -notmatch '"alpha"') { throw "tags query missing alpha" }
if ($result -notmatch '"beta"') { throw "tags query missing beta" }
pass "serve: graphql tags query"

$result = gql '{"query":"{ doogats(tag: \"alpha\") { id title tags } }"}'
if ($result -notmatch $TAG_ID) { throw "tag filter missing expected doogat" }
pass "serve: graphql doogats tag filter"

gql "{`"query`":`"mutation { deleteDoogat(id: \`"$TAG_ID\`") }`"}" | Out-Null

# 18c2. GraphQL updated_at and created_at fields
$ts_result = gql '{"query":"mutation { createDoogat(input: { title: \"Timestamp Test\" }) { id } }"}'
$TS_ID = if ($ts_result -match '"id":"([^"]+)"') { $Matches[1] }
$ts_query = gql "{`"query`":`"{ doogat(id: \`"$TS_ID\`") { updated_at created_at date } }`"}"
if ($ts_query -notmatch '"updated_at"') { throw "missing updated_at" }
if ($ts_query -notmatch '"created_at"') { throw "missing created_at" }
pass "serve: graphql updated_at and created_at fields"

$ts_date = if ($ts_query -match '"date":"([^"]+)"') { $Matches[1] }
$ts_created = if ($ts_query -match '"created_at":"([^"]+)"') { $Matches[1] }
if ($ts_date -ne $ts_created) { throw "created_at does not equal date" }
pass "serve: created_at equals date"

$ts_search = gql '{"query":"{ search(query: \"Timestamp Test\") { hits { id updated_at } } }"}'
if ($ts_search -notmatch '"updated_at"') { throw "search hit missing updated_at" }
pass "serve: search hits include updated_at"

gql "{`"query`":`"mutation { deleteDoogat(id: \`"$TS_ID\`") }`"}" | Out-Null

# 18d. GraphQL search filters
$sf1 = gql '{"query":"mutation { createDoogat(input: { title: \"SearchFilter Alpha\", type: \"link\", tags: [\"sf-tag\"] }) { id } }"}'
$SF1_ID = if ($sf1 -match '"id":"([^"]+)"') { $Matches[1] }
$sf2 = gql '{"query":"mutation { createDoogat(input: { title: \"SearchFilter Beta\", type: \"note\", tags: [\"sf-tag\"] }) { id } }"}'
$SF2_ID = if ($sf2 -match '"id":"([^"]+)"') { $Matches[1] }
$sf3 = gql '{"query":"mutation { createDoogat(input: { title: \"SearchFilter Gamma\", type: \"link\" }) { id } }"}'
$SF3_ID = if ($sf3 -match '"id":"([^"]+)"') { $Matches[1] }

$result = gql '{"query":"{ search(query: \"SearchFilter\", types: [\"link\"]) { totalCount hits { id } } }"}'
if ($result -notmatch '"totalCount":2') { throw "search type filter: expected 2, got $result" }
pass "serve: search filter by type"

$result = gql '{"query":"{ search(query: \"SearchFilter\", tag: \"sf-tag\") { totalCount hits { id } } }"}'
if ($result -notmatch '"totalCount":2') { throw "search tag filter: expected 2, got $result" }
pass "serve: search filter by tag"

$result = gql '{"query":"{ search(query: \"SearchFilter\", types: [\"link\"], tag: \"sf-tag\") { totalCount hits { id } } }"}'
if ($result -notmatch '"totalCount":1') { throw "search combined filter: expected 1, got $result" }
pass "serve: search filter combined type+tag"

gql "{`"query`":`"mutation { deleteDoogat(id: \`"$SF1_ID\`") }`"}" | Out-Null
gql "{`"query`":`"mutation { deleteDoogat(id: \`"$SF2_ID\`") }`"}" | Out-Null
gql "{`"query`":`"mutation { deleteDoogat(id: \`"$SF3_ID\`") }`"}" | Out-Null

# 18e. Boolean and phrase search queries
$bq1 = gql '{"query":"mutation { createDoogat(input: { title: \"BoolSearch Rust CRDT\", content: \"rust crdt patterns\" }) { id } }"}'
$BQ1_ID = if ($bq1 -match '"id":"([^"]+)"') { $Matches[1] }
$bq2 = gql '{"query":"mutation { createDoogat(input: { title: \"BoolSearch Rust Only\", content: \"rust programming\" }) { id } }"}'
$BQ2_ID = if ($bq2 -match '"id":"([^"]+)"') { $Matches[1] }
$bq3 = gql '{"query":"mutation { createDoogat(input: { title: \"BoolSearch Golang\", content: \"golang programming\" }) { id } }"}'
$BQ3_ID = if ($bq3 -match '"id":"([^"]+)"') { $Matches[1] }

$result = gql '{"query":"{ search(query: \"rust AND crdt\") { totalCount } }"}'
if ($result -notmatch '"totalCount":1') { throw "search AND: expected 1, got $result" }
pass "serve: search boolean AND"

$result = gql '{"query":"{ search(query: \"rust OR golang\") { totalCount } }"}'
if ($result -notmatch '"totalCount":3') { throw "search OR: expected 3, got $result" }
pass "serve: search boolean OR"

$result = gql '{"query":"{ search(query: \"rust NOT crdt\") { totalCount } }"}'
if ($result -notmatch '"totalCount":1') { throw "search NOT: expected 1, got $result" }
pass "serve: search boolean NOT"

$result = gql '{"query":"{ search(query: \"\\\"rust crdt\\\"\") { totalCount } }"}'
if ($result -notmatch '"totalCount":1') { throw "search phrase: expected 1, got $result" }
pass "serve: search quoted phrase"

$result = try { gql '{"query":"{ search(query: \"AND AND\") { totalCount } }"}' } catch { $_.Exception.Message }
if ($result -notmatch 'BAD_REQUEST') { throw "search malformed: expected BAD_REQUEST, got $result" }
pass "serve: search malformed query returns BAD_REQUEST"

gql "{`"query`":`"mutation { deleteDoogat(id: \`"$BQ1_ID\`") }`"}" | Out-Null
gql "{`"query`":`"mutation { deleteDoogat(id: \`"$BQ2_ID\`") }`"}" | Out-Null
gql "{`"query`":`"mutation { deleteDoogat(id: \`"$BQ3_ID\`") }`"}" | Out-Null

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
gql '{"query":"mutation{executeSql(sql:\"CREATE TABLE smokeitem (label TEXT NOT NULL, priority INTEGER)\"){message}}"}'
gql '{"query":"mutation{executeSql(sql:\"INSERT INTO smokeitem (label, priority) VALUES (''Smoke1'', 7)\"){message}}"}'
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

# error sanitization — SQL error must not leak raw details
$result = gql '{"query":"mutation { executeSql(sql: \"SELCT * FORM oops\") { message } }"}'
if ($result -notmatch "errors") { throw "expected errors in response" }
if ($result -notmatch "(?i)query failed|internal error") { throw "expected sanitized message" }
if ($result -match "(?i)SELCT|syntax error|sqlite") { throw "raw SQL details leaked" }
pass "serve: sql error sanitized (no raw details)"

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
$intro = gql '{"query":"{ __schema { queryType { fields { name } } } }"}'
if ($intro -match "_ddb_") { throw "introspection leaked internal table: $intro" }
pass "serve: introspection hides internal tables"

# compact mutation
$result = gql '{"query":"mutation { compact { filesRemoved crdtDocsCompacted gcSuccess crdtTempBytesBefore crdtTempBytesAfter crdtTempFilesBefore crdtTempFilesAfter repoBytesBefore repoBytesAfter backupPath } }"}'
if ($result -notmatch "gcSuccess") { throw "compact mutation failed" }
pass "serve: compact mutation"

# compact(force: true)
$result = gql '{"query":"mutation { compact(force: true) { filesRemoved crdtDocsCompacted gcSuccess crdtTempBytesBefore crdtTempBytesAfter repoBytesBefore repoBytesAfter backupPath } }"}'
if ($result -notmatch "gcSuccess") { throw "compact(force:true) mutation failed" }
if ($result -notmatch "backupPath") { throw "compact(force:true) missing backupPath" }
pass "serve: compact(force: true) mutation"

# compact(noBackup: true)
$result = gql '{"query":"mutation { compact(force: true, noBackup: true) { gcSuccess backupPath } }"}'
if ($result -notmatch "gcSuccess") { throw "compact(noBackup:true) failed" }
if ($result -notmatch '"backupPath":null') { throw "compact(noBackup:true) should have null backupPath" }
pass "serve: compact(noBackup: true) mutation"

# compact(backupPath: custom)
$gqlBackup = Join-Path $env:TEMP "gql-backup.bundle.tar"
$gqlBackupEsc = $gqlBackup -replace '\\', '\\\\'
$result = gql "{`"query`":`"mutation { compact(force: true, backupPath: \`"$gqlBackupEsc\`") { gcSuccess backupPath } }`"}"
if ($result -notmatch "gcSuccess") { throw "compact(backupPath) failed" }
if ($result -notmatch "backupPath") { throw "compact(backupPath) missing backupPath" }
if (-not (Test-Path $gqlBackup)) { throw "compact(backupPath) file not created" }
pass "serve: compact(backupPath) mutation"

# maintenance mutation
$result = gql '{"query":"mutation { maintenance { success durationMs fallbackUsed tasksRun } }"}'
if ($result -notmatch "success") { throw "maintenance mutation failed" }
pass "serve: maintenance mutation"

# sync mutation — no remote configured, expect error not panic
$result = gql '{"query":"mutation { sync { direction commitsTransferred conflictsResolved resurrected } }"}'
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
$readResult = gql '{"query":"{ doogats { id title } }"}'
if ($readResult -notmatch "doogats") { throw "read-under-write: read failed" }
$writeResult = Receive-Job -Job $writeJob -Wait | ConvertTo-Json
if ($writeResult -notmatch "id") { throw "read-under-write: write failed" }
Remove-Job $writeJob
pass "serve: read-under-write (concurrent read + write)"

# 38b. multi-value references via GraphQL + REST
gql '{"query":"mutation{executeSql(sql:\"CREATE TABLE mvcategory (name VARCHAR(100))\"){message}}"}' | Out-Null
gql '{"query":"mutation{executeSql(sql:\"CREATE TABLE mvbookmark (mvcategory TEXT REFERENCES mvcategory)\"){message}}"}' | Out-Null
Start-Sleep -Seconds 1
$mvCat1 = (gql '{"query":"mutation{executeSql(sql:\"INSERT INTO mvcategory (name) VALUES (''Science'')\"){message}}"}') -replace '.*"message":"(\d+)".*','$1'
$mvCat2 = (gql '{"query":"mutation{executeSql(sql:\"INSERT INTO mvcategory (name) VALUES (''Math'')\"){message}}"}') -replace '.*"message":"(\d+)".*','$1'
$mvBm = (gql "{`"query`":`"mutation{executeSql(sql:\`"INSERT INTO mvbookmark (mvcategory) VALUES ('$mvCat1')\`"){message}}`"}") -replace '.*"message":"(\d+)".*','$1'
gql "{`"query`":`"mutation{executeSql(sql:\`"INSERT INTO mvbookmark_mvcategory (mvbookmark_id, mvcategory_id) VALUES ('$mvBm', '$mvCat2')\`"){message}}`"}" | Out-Null
$mvResult = gql '{"query":"{ mvbookmarks { items { id mvcategories { id } } } }"}'
if ($mvResult -notmatch $mvCat1) { throw "multi-value ref: cat1 not in graphql list field" }
if ($mvResult -notmatch $mvCat2) { throw "multi-value ref: cat2 not in graphql list field" }
pass "serve: graphql multi-value ref list field"
$mvRest = rest "/doogats/$mvBm"
if ($mvRest -notmatch '"references"') { throw "multi-value ref: no references in rest json" }
if ($mvRest -notmatch '"mvcategory"') { throw "multi-value ref: no category key in references" }
pass "serve: rest multi-value ref structured json"

# 38b2. REFERENCES relation resolution
gql '{"query":"mutation { executeSql(sql: \"CREATE TABLE smokecat (label TEXT)\") { message } }"}' | Out-Null
gql '{"query":"mutation { executeSql(sql: \"CREATE TABLE smokebm (url TEXT, smokecat TEXT REFERENCES smokecat)\") { message } }"}' | Out-Null
$scat = gql "{`"query`":`"mutation { executeSql(sql: \`"INSERT INTO smokecat (title, label) VALUES ('Tech', 'tech')\`") { message } }`"}"
$SCAT_ID = if ($scat -match '"message":"([^"]+)"') { $Matches[1] }
Start-Sleep -Seconds 1
$sbm = gql "{`"query`":`"mutation { executeSql(sql: \`"INSERT INTO smokebm (title, url) VALUES ('Example', 'https://example.com')\`") { message } }`"}"
$SBM_ID = if ($sbm -match '"message":"([^"]+)"') { $Matches[1] }
gql "{`"query`":`"mutation { executeSql(sql: \`"INSERT INTO smokebm_smokecat (smokebm_id, smokecat_id) VALUES ('$SBM_ID', '$SCAT_ID')\`") { message } }`"}" | Out-Null
$result = gql '{"query":"{ smokebms { items { smokecat { id label } } } }"}'
if ($result -notmatch '"label":"tech"') { throw "singular relation resolution failed: $result" }
pass "serve: relation singular resolves object"
$result = gql '{"query":"{ smokebms { items { smokecats { id label } } } }"}'
if ($result -notmatch '"label":"tech"') { throw "plural relation resolution failed: $result" }
pass "serve: relation plural resolves object list"
Start-Sleep -Seconds 1
gql "{`"query`":`"mutation { executeSql(sql: \`"INSERT INTO smokebm (title, url) VALUES ('No Cat', 'https://nocat.com')\`") { message } }`"}" | Out-Null
$result = gql '{"query":"{ smokebms { items { id smokecat { id } smokecats { id } } } }"}'
if ($result -notmatch '"smokecat":null') { throw "null relation failed: $result" }
if ($result -notmatch '"smokecats":\[\]') { throw "empty plural relation failed: $result" }
pass "serve: relation null returns null and empty list"
gql '{"query":"mutation { executeSql(sql: \"DROP TABLE smokebm CASCADE\") { message } }"}' | Out-Null
gql '{"query":"mutation { executeSql(sql: \"DROP TABLE smokecat CASCADE\") { message } }"}' | Out-Null

# 38b3. typed connection includes tags
gql '{"query":"mutation { executeSql(sql: \"CREATE TABLE tagarticle (topic TEXT)\") { message } }"}' | Out-Null
$taId = (gql '{"query":"mutation { executeSql(sql: \"INSERT INTO tagarticle (topic) VALUES (\\\"rust\\\")\") { message } }"}') -replace '.*"message":"(\d+)".*','$1'
gql "{`"query`":`"mutation { updateDoogat(input: { id: \`"$taId\`", tags: [\`"coding\`", \`"systems\`"] }) { id } }`"}" | Out-Null
$result = gql '{"query":"{ tagarticles { items { id tags topic } } }"}'
if ($result -notmatch '"coding"') { throw "typed connection tags: missing coding tag: $result" }
if ($result -notmatch '"systems"') { throw "typed connection tags: missing systems tag: $result" }
pass "serve: typed connection includes tags"
gql '{"query":"mutation { executeSql(sql: \"DROP TABLE tagarticle CASCADE\") { message } }"}' | Out-Null

# 38c. sql-materialization (columns, boolean normalization, core fields)
$result = gql '{"query":"{ sql(query: \"SELECT id, title FROM doogats\") { columns rows } }"}'
if ($result -notmatch '"columns"') { throw "sql columns: missing columns field" }
if ($result -notmatch '"id"') { throw "sql columns: missing id column" }
if ($result -notmatch '"title"') { throw "sql columns: missing title column" }
pass "serve: sql columns in response"

# 38c2. sql format:objects returns keyed rows
$result = gql '{"query":"{ sql(query: \"SELECT id, title FROM doogats\", format: \"objects\") { columns rows } }"}'
if ($result -notmatch '"id":') { throw "sql format objects: missing id key in row object" }
if ($result -notmatch '"title":') { throw "sql format objects: missing title key in row object" }
pass "serve: sql format objects returns keyed rows"

gql '{"query":"mutation{executeSql(sql:\"CREATE TABLE smokepin (pinned BOOLEAN)\"){message}}"}' | Out-Null
$smokepinId = (gql "{`"query`":`"mutation{executeSql(sql:\`"INSERT INTO smokepin (title, pinned) VALUES ('PinTest', true)\`"){message}}`"}") -replace '.*"message":"(\d+)".*','$1'
if (-not $smokepinId) { throw "smokepin insert failed" }
$result = gql "{`"query`":`"{ sql(query: \`"SELECT pinned FROM smokepin WHERE pinned = 1\`") { rows } }`"}"
if ($result -notmatch '[\\"]true[\\"]') { throw "boolean not coerced to true" }
pass "serve: boolean coerced to true/false"

# Boolean false
Start-Sleep -Seconds 1
gql "{`"query`":`"mutation{executeSql(sql:\`"INSERT INTO smokepin (title, pinned) VALUES ('FalseTest', false)\`"){message}}`"}" | Out-Null
$result = gql "{`"query`":`"{ sql(query: \`"SELECT pinned FROM smokepin WHERE pinned = 0\`") { rows } }`"}"
if ($result -notmatch '[\\"]false[\\"]') { throw "boolean false not coerced" }
pass "serve: boolean false coerced"

$result = gql '{"query":"{ sql(query: \"SELECT title FROM smokepin\") { rows } }"}'
if ($result -notmatch 'PinTest') { throw "core fields: title missing from type table" }
pass "serve: core fields in type table"

# 38d. DISTINCT on typed connection queries
gql "{`"query`":`"mutation{executeSql(sql:\`"INSERT INTO foo (title, bar, baz) VALUES ('dup1', 'val', 2)\`"){message}}`"}" | Out-Null
gql "{`"query`":`"mutation{executeSql(sql:\`"INSERT INTO foo (title, bar, baz) VALUES ('uniq', 'other', 3)\`"){message}}`"}" | Out-Null
Start-Sleep -Seconds 1
$result = gql '{"query":"{ foos(distinct: \"bar\") { items { bar } totalCount } }"}'
if ($result -notmatch '"totalCount":2') { throw "distinct totalCount: $result" }
pass "serve: distinct deduplicates and totalCount reflects unique count"

$result = gql '{"query":"{ foos(distinct: \"bar\", where: { baz: { gte: 2 } }) { totalCount } }"}'
if ($result -notmatch '"totalCount":2') { throw "distinct with where: $result" }
pass "serve: distinct with where filter"

# 38e. GROUP BY on typed aggregate queries
$result = gql '{"query":"{ foosAggregate(groupBy: \"bar\") { groups { key count } } }"}'
if ($result -notmatch '"key":"val"') { throw "groupBy missing val: $result" }
if ($result -notmatch '"key":"other"') { throw "groupBy missing other: $result" }
pass "serve: groupBy returns per-group counts"

$result = gql '{"query":"{ foosAggregate(groupBy: \"bar\") { groups { key count minBaz maxBaz } } }"}'
if ($result -notmatch '"minBaz"') { throw "groupBy missing minBaz: $result" }
if ($result -notmatch '"maxBaz"') { throw "groupBy missing maxBaz: $result" }
pass "serve: groupBy with numeric aggregates"

$result = gql '{"query":"{ foosAggregate(groupBy: \"bar\", where: { baz: { gte: 2 } }) { groups { key count } } }"}'
if ($result -notmatch '"key"') { throw "groupBy with where: $result" }
pass "serve: groupBy with where filter"

$result = gql '{"query":"{ foosAggregate { count } }"}'
if ($result -notmatch '"count":3') { throw "aggregate without groupBy: $result" }
pass "serve: aggregate without groupBy still works"

# 38f. executeBatch mutation
$result = gql '{"query":"mutation { executeBatch(statements: [\"INSERT INTO foo (title, bar, baz) VALUES (''batch1'', ''b1'', 10)\", \"INSERT INTO foo (title, bar, baz) VALUES (''batch2'', ''b2'', 20)\"]) { message affected } }"}'
if ($result -match '"errors"') { throw "executeBatch errors: $result" }
pass "serve: executeBatch multiple INSERTs"

$result = gql '{"query":"mutation { executeBatch(statements: [\"CREATE TABLE batchtest (col1 TEXT)\"]) { message } }"}'
if ($result -notmatch '"message"') { throw "executeBatch DDL: $result" }
Start-Sleep -Seconds 1
$result = gql '{"query":"{ batchtests { totalCount } }"}'
if ($result -notmatch '"totalCount":0') { throw "executeBatch schema reload: $result" }
pass "serve: executeBatch DDL triggers schema reload"

# executeBatch failure rolls back
$preCount = (gql '{"query":"{ foosAggregate { count } }"}') -replace '.*"count":(\d+).*','$1'
try { gql '{"query":"mutation { executeBatch(statements: [\"INSERT INTO foo (title, bar, baz) VALUES (''rollback_test'', ''rb'', 99)\", \"INSERT INTO no_such_table (title) VALUES (''bad'')\"]) { message } }"}' } catch {}
Start-Sleep -Seconds 1
$postCount = (gql '{"query":"{ foosAggregate { count } }"}') -replace '.*"count":(\d+).*','$1'
if ($preCount -ne $postCount) { throw "executeBatch rollback failed: pre=$preCount post=$postCount" }
pass "serve: executeBatch failure rolls back all statements"

gql '{"query":"mutation { executeSql(sql: \"DROP TABLE batchtest CASCADE\") { message } }"}' | Out-Null

# 38g. batchUpdate mutation
$bu1 = gql '{"query":"mutation { createDoogat(input: { title: \"BatchUp Alpha\" }) { id } }"}'
$BU1_ID = if ($bu1 -match '"id":"([^"]+)"') { $Matches[1] } else { throw "no bu1 id" }
$bu2 = gql '{"query":"mutation { createDoogat(input: { title: \"BatchUp Beta\" }) { id } }"}'
$BU2_ID = if ($bu2 -match '"id":"([^"]+)"') { $Matches[1] } else { throw "no bu2 id" }
$bu3 = gql '{"query":"mutation { createDoogat(input: { title: \"BatchUp Gamma\" }) { id } }"}'
$BU3_ID = if ($bu3 -match '"id":"([^"]+)"') { $Matches[1] } else { throw "no bu3 id" }

$result = gql "{`"query`":`"mutation { batchUpdate(updates: [{id: \`"$BU1_ID\`", title: \`"Updated Alpha\`"}, {id: \`"$BU2_ID\`", title: \`"Updated Beta\`"}, {id: \`"$BU3_ID\`", title: \`"Updated Gamma\`"}]) { id title } }`"}"
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

gql "{`"query`":`"mutation { deleteDoogat(id: \`"$BU1_ID\`") }`"}" | Out-Null
gql "{`"query`":`"mutation { deleteDoogat(id: \`"$BU2_ID\`") }`"}" | Out-Null
gql "{`"query`":`"mutation { deleteDoogat(id: \`"$BU3_ID\`") }`"}" | Out-Null

# Hyphenated type names in GraphQL
gql "{`"query`":`"mutation { executeSql(sql: \`"CREATE TABLE \\\`"test-widget\\\`" (status TEXT, priority INTEGER)\`") { message } }`"}" | Out-Null
Start-Sleep -Seconds 1
gql "{`"query`":`"mutation { executeSql(sql: \`"INSERT INTO \\\`"test-widget\\\`" (status, priority) VALUES ('active', 1)\`") { message } }`"}" | Out-Null
$result = gql "{`"query`":`"{ testWidgets { items { id status priority } totalCount } }`"}"
$parsed = $result | ConvertFrom-Json
if ($parsed.data.testWidgets.totalCount -ne 1) { throw "testWidgets expected 1 item, got $($parsed.data.testWidgets.totalCount)" }
if ($parsed.data.testWidgets.items[0].status -ne "active") { throw "status expected active, got $($parsed.data.testWidgets.items[0].status)" }
if ($parsed.data.testWidgets.items[0].priority -ne 1) { throw "priority expected 1, got $($parsed.data.testWidgets.items[0].priority)" }
pass "serve: hyphenated type typed query"

# 42. base field filters on typed queries (id, title)
gql "{`"query`":`"mutation { executeSql(sql: \`"INSERT INTO \\\`"test-widget\\\`" (title, status, priority) VALUES ('FilterTarget', 'pending', 5)\`") { message } }`"}" | Out-Null
Start-Sleep -Seconds 1

$result = gql "{`"query`":`"{ testWidgets(where: { title: { eq: \\\`"FilterTarget\\\`" } }) { items { id title } totalCount } }`"}"
$parsed = $result | ConvertFrom-Json
if ($parsed.data.testWidgets.totalCount -ne 1) { throw "title eq filter expected 1, got $($parsed.data.testWidgets.totalCount)" }
$BF_ID = $parsed.data.testWidgets.items[0].id
pass "serve: base field title eq filter"

$result = gql "{`"query`":`"{ testWidgets(where: { id: { eq: \\\`"$BF_ID\\\`" } }) { items { id title } totalCount } }`"}"
$parsed = $result | ConvertFrom-Json
if ($parsed.data.testWidgets.totalCount -ne 1) { throw "id eq filter expected 1, got $($parsed.data.testWidgets.totalCount)" }
if ($parsed.data.testWidgets.items[0].id -ne $BF_ID) { throw "id mismatch" }
pass "serve: base field id eq filter"

$result = gql "{`"query`":`"{ testWidgets(where: { title: { contains: \\\`"Target\\\`" } }) { items { id } totalCount } }`"}"
$parsed = $result | ConvertFrom-Json
if ($parsed.data.testWidgets.totalCount -ne 1) { throw "title contains filter expected 1, got $($parsed.data.testWidgets.totalCount)" }
pass "serve: base field title contains filter"

$result = gql "{`"query`":`"{ testWidgets(where: { id: { eq: \\\`"99999999999999\\\`" } }) { items { id } totalCount } }`"}"
$parsed = $result | ConvertFrom-Json
if ($parsed.data.testWidgets.totalCount -ne 0) { throw "nonexistent id expected 0, got $($parsed.data.testWidgets.totalCount)" }
pass "serve: base field id nonexistent returns empty"

gql "{`"query`":`"mutation { deleteDoogat(id: \\\`"$BF_ID\\\`") }`"}" | Out-Null
gql "{`"query`":`"mutation { executeSql(sql: \`"DROP TABLE \\\`"test-widget\\\`"\`") { message } }`"}" | Out-Null

# 43. SQL INSERT via executeSql defaults date, created_at non-null
gql "{`"query`":`"mutation{executeSql(sql:\`"CREATE TABLE datecheck (name TEXT)\`"){message}}`"}" | Out-Null
$dcResult = gql "{`"query`":`"mutation{executeSql(sql:\`"INSERT INTO datecheck (name) VALUES (\\\`"DateTest\\\`")\`"){message}}`"}"
$dcId = ($dcResult | ConvertFrom-Json).data.executeSql.message
$dcExpected = "$($dcId.Substring(0,4))-$($dcId.Substring(4,2))-$($dcId.Substring(6,2))"
$dcQuery = gql "{`"query`":`"{ datechecks { items { id created_at } } }`"}"
$dcCreated = ($dcQuery | ConvertFrom-Json).data.datechecks.items[0].created_at
if ($dcCreated -ne $dcExpected) { throw "created_at '$dcCreated' != expected '$dcExpected'" }
pass "serve: SQL INSERT defaults date, created_at matches ID"

# executeBatch also defaults date
$ebResult = gql "{`"query`":`"mutation{executeBatch(statements:[\`"INSERT INTO datecheck (name) VALUES (\\\`"BatchTest\\\`")\`"]){message}}`"}"
$ebId = ($ebResult | ConvertFrom-Json).data.executeBatch[0].message
$ebExpected = "$($ebId.Substring(0,4))-$($ebId.Substring(4,2))-$($ebId.Substring(6,2))"
$ebQuery = gql "{`"query`":`"{ doogat(id: \\\`"$ebId\\\`") { created_at } }`"}"
$ebCreated = ($ebQuery | ConvertFrom-Json).data.doogat.created_at
if ($ebCreated -ne $ebExpected) { throw "executeBatch created_at '$ebCreated' != expected '$ebExpected'" }
pass "serve: executeBatch INSERT defaults date, created_at matches ID"

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
if ($output -notmatch "^\d{14}$") { throw "nosql scan --type failed" }
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
if ($MULTI_IDS -notmatch "^\d{14},\d{14},\d{14}$") { throw "multi-row insert did not return 3 IDs: $MULTI_IDS" }
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

Cleanup
Write-Host "=== all integration tests passed ==="
