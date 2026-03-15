#!/usr/bin/env pwsh
param(
    [ValidateSet("quick", "full")]
    [string]$SmokeProfile = $(if ($env:SMOKE_PROFILE) { $env:SMOKE_PROFILE } else { "full" })
)

# Windows smoke test — PowerShell port of tests/smoke.sh
$ErrorActionPreference = "Stop"

# Build and lint for the full profile when ZDB_BIN is not injected.
$prepLabel = "prebuilt binary"
if (-not $env:ZDB_BIN) {
    cargo build --quiet
    if ($SmokeProfile -eq "full") {
        cargo clippy --workspace --quiet
        cargo bench --no-run --quiet 2>$null
        $prepLabel = "clippy + bench compile"
    } else {
        $prepLabel = "build"
    }
}

if ($env:ZDB_BIN) {
    $ZDB = $env:ZDB_BIN
} else {
    $meta = cargo metadata --format-version=1 --no-deps | ConvertFrom-Json
    $ZDB = Join-Path $meta.target_directory "debug" "zdb.exe"
}

# Work in temp directories, clean up on exit
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

function zdb {
    $output = & $ZDB @args 2>&1
    $text = [string]::Join("`n", @($output))
    if ($LASTEXITCODE -ne 0) { throw "zdb $($args -join ' ') failed: $text" }
    return $text
}

# Expect failure: returns true if command fails
function zdb-fails {
    & $ZDB @args 2>&1 | Out-Null
    return ($LASTEXITCODE -ne 0)
}

Write-Host "=== smoke test ($SmokeProfile) ==="

pass $prepLabel

# 1. init
zdb init . | Out-Null
pass "init"

# 2. create zettels
$ID1 = zdb create --title "First note" --tags "test,smoke" --body "Hello world"
$ID2 = zdb create --title "Links to first" --body "See [[$ID1]]"
$ID3 = zdb create --title "Project Alpha" --type project --tags "active" --body "A project zettel"
if ($ID1 -eq $ID2 -or $ID2 -eq $ID3 -or $ID1 -eq $ID3) { throw "IDs not unique" }
pass "create (3 unique IDs: $ID1 $ID2 $ID3)"

# 3. read
$output = zdb read $ID1
if ($output -notmatch "First note") { throw "read failed" }
pass "read"

# 4. update
zdb update $ID1 --title "First note (edited)" --tags "test,smoke,updated"
$output = zdb read $ID1
if ($output -notmatch "First note \(edited\)") { throw "update failed" }
pass "update"

# 5. delete
zdb delete $ID3
if (-not (zdb-fails read $ID3)) { throw "read after delete should fail" }
if (-not (zdb-fails delete "99999999999999")) { throw "delete nonexistent should fail" }
pass "delete"

# 6. status
$output = zdb status
if ($output -notmatch "^head:") { throw "status missing head" }
pass "status"

# 6b. broken backlink report on delete
$BL_TARGET = zdb create --title "Backlink Target" --body "I will be deleted"
Start-Sleep -Seconds 1
$BL_SOURCE = zdb create --title "Backlink Source" --body "See [[$BL_TARGET]]"
zdb reindex | Out-Null
$output = zdb delete $BL_TARGET
if ($output -notmatch "broken backlinks") { throw "delete missing broken backlink report" }
$output = zdb status
if ($output -notmatch "broken backlinks") { throw "status missing broken backlinks" }
# Clean up: delete source so broken backlinks don't affect later tests
zdb delete $BL_SOURCE | Out-Null
pass "broken backlink report on delete"

# 7. reindex
$output = zdb reindex
if ($output -notmatch "indexed 2 zettels") { throw "reindex count wrong" }
pass "reindex"

# 7b. hashtag extraction
zdb update $ID1 --body "Updated with #gtd/act/next hashtag"
zdb reindex | Out-Null
$output = zdb query "SELECT tag, source FROM _zdb_tags WHERE tag = 'gtd/act/next'"
if ($output -notmatch "body") { throw "hashtag not indexed" }
pass "hashtag extraction and indexing"

# 7c. checkbox parsing
zdb update $ID1 --body "- [ ] open task`n- [x] done task`n- [i] 2026-01-01 10:00 - info note"
zdb reindex | Out-Null
$output = zdb query "SELECT state, content FROM _zdb_checkboxes WHERE state = 'open'"
if ($output -notmatch "open task") { throw "checkbox not indexed" }
pass "checkbox parsing and indexing"

# 8. full-text search
$output = zdb search "Hello"
if ($output -notmatch $ID1) { throw "search failed" }
pass "search"

# 8b. paginated search
$output = zdb search "Hello" --limit 1 --offset 0
if ($output -notmatch "Showing 1-1 of") { throw "paginated search failed" }
pass "paginated search"

# 9. SQL queries
$output = zdb query "SELECT id, title FROM zettels"
if ($output -notmatch "First note \(edited\)") { throw "sql select failed" }
$output = zdb query "SELECT z.id, z.title FROM zettels z JOIN _zdb_tags t ON t.zettel_id = z.id WHERE t.tag LIKE '%smoke%'"
if ($output -notmatch $ID1) { throw "sql join failed" }
pass "sql queries"

# 10. wikilinks
$output = zdb query "SELECT * FROM _zdb_links"
if ($output -notmatch $ID1) { throw "wikilinks failed" }
pass "wikilinks"

# 10b. rename with backlink rewrite
$RENAME_TARGET = zdb create --title "Rename Target" --body "I will move."
zdb create --title "Rename Linker" --body "See [[$RENAME_TARGET|Target]]." | Out-Null
zdb reindex | Out-Null
$output = zdb rename $RENAME_TARGET "zettelkasten/contact/${RENAME_TARGET}.md"
if ($output -notmatch "1 backlinks updated") { throw "rename failed" }
if (-not (Test-Path "zettelkasten/contact/${RENAME_TARGET}.md")) { throw "renamed file missing" }
pass "rename with backlink rewrite"

# 11. SQL DDL/DML
$output = zdb query "CREATE TABLE foo (bar TEXT, baz INTEGER)"
if ($output -notmatch "table foo created") { throw "create table failed" }
$FOO_ID = zdb query "INSERT INTO foo (title, bar, baz) VALUES ('test row', 'hello', 42)"
if ($FOO_ID -notmatch "^\d{14}$") { throw "insert returned bad id" }
$output = zdb query "SELECT bar, baz FROM foo"
if ($output -notmatch "hello") { throw "select from foo failed" }
$output = zdb query "UPDATE foo SET baz = 99 WHERE id = '$FOO_ID'"
if ($output -notmatch "1 row\(s\) affected") { throw "update failed" }
$output = zdb query "SELECT baz FROM foo WHERE id = '$FOO_ID'"
if ($output -notmatch "99") { throw "select after update failed" }
$output = zdb query "DELETE FROM foo WHERE id = '$FOO_ID'"
if ($output -notmatch "1 row\(s\) affected") { throw "delete failed" }
pass "sql ddl/dml"

# 11b. CREATE TABLE IF NOT EXISTS (idempotent)
$output = zdb query "CREATE TABLE IF NOT EXISTS foo (bar TEXT, baz INTEGER)"
if ($output -notmatch "already exists") { throw "IF NOT EXISTS on existing table failed" }
$output = zdb query "CREATE TABLE IF NOT EXISTS newifne (x TEXT)"
if ($output -notmatch "table newifne created") { throw "IF NOT EXISTS new table failed" }
$output = zdb query "CREATE TABLE IF NOT EXISTS newifne (x TEXT)"
if ($output -notmatch "already exists") { throw "IF NOT EXISTS idempotent failed" }
pass "create table if not exists (idempotent)"

# 12. install bundled type
$output = zdb type install contact
if ($output -notmatch "installed type") { throw "type install failed" }
pass "type install"

# 12a. hyphenated type SQL (quoted identifiers)
$output = zdb type install meeting-minutes
if ($output -notmatch "installed type") { throw "meeting-minutes install failed" }
$HYP_ID = (zdb query 'INSERT INTO "meeting-minutes" (date, attendees) VALUES (''2026-03-10'', ''alice,bob'')').Trim()
$output = zdb query "SELECT date FROM `"meeting-minutes`" WHERE id = '$HYP_ID'"
if ($output -notmatch "2026-03-10") { throw "hyphenated select failed" }
$output = zdb query "DELETE FROM `"meeting-minutes`" WHERE id = '$HYP_ID'"
if ($output -notmatch "1 row") { throw "hyphenated delete failed" }
pass "hyphenated type sql (quoted identifiers)"

# 13. type suggest
zdb query "INSERT INTO foo (title, bar, baz) VALUES ('for suggest', 'val', 1)" | Out-Null
$output = zdb type suggest foo
if ($output -notmatch "bar") { throw "type suggest failed" }
pass "type suggest"

# 14. register node + compact
$output = zdb register-node "smoke-test-laptop"
if ($output -notmatch "registered node") { throw "register-node failed" }
$output = zdb status
if ($output -notmatch "registered nodes: 1") { throw "status missing node" }
$output = zdb compact --force
if ($output -notmatch "backup:") { throw "compact missing backup path" }
if ($output -notmatch "gc: ok") { throw "compact failed" }
if ($output -notmatch "crdt temp:") { throw "compact missing crdt temp stats" }
if ($output -notmatch "repo \(\.git\):") { throw "compact missing repo stats" }
pass "register-node + compact"

# 15. node list + retire
$output = zdb node list
if ($output -notmatch "smoke-test-laptop") { throw "node list failed" }
$NODE_UUID = (($output -split "\r?\n") | Select-String "smoke-test-laptop").ToString().Split()[0]
$output = zdb node retire $NODE_UUID
if ($output -notmatch "retired node") { throw "node retire failed" }
pass "node list + retire"

# 16. compact --dry-run
$output = zdb compact --dry-run
if ($output -notmatch "dry run") { throw "compact dry-run failed" }
if ($output -notmatch "backup would write:") { throw "dry-run missing backup info" }
pass "compact --dry-run"

# 16a. compact --no-backup
zdb register-node "no-backup-test" | Out-Null
$output = zdb compact --no-backup --force
if ($output -notmatch "gc: ok") { throw "compact --no-backup failed" }
if ($output -match "backup:") { throw "--no-backup should suppress backup" }
pass "compact --no-backup"

# 16b. compact --backup-path
$customBackup = Join-Path $env:TEMP "custom-backup.bundle.tar"
$output = zdb compact --force --backup-path $customBackup
if ($output -notmatch "backup:") { throw "compact --backup-path missing backup line" }
if ($output -notmatch [regex]::Escape($customBackup)) { throw "compact --backup-path wrong path" }
if (-not (Test-Path $customBackup)) { throw "custom backup file not created" }
pass "compact --backup-path"

# 16c. maintenance
$output = zdb maintenance run
if ($output -notmatch "maintenance:") { throw "maintenance run missing output" }
pass "maintenance run"

$output = zdb maintenance auto status
if ($output -notmatch "off") { throw "maintenance auto status should default to off" }
pass "maintenance auto status (default off)"

zdb maintenance auto on | Out-Null
$output = zdb maintenance auto status
if ($output -notmatch "on") { throw "maintenance auto status should be on" }
pass "maintenance auto on"

zdb maintenance auto off | Out-Null
$output = zdb maintenance auto status
if ($output -notmatch "off") { throw "maintenance auto status should be off" }
pass "maintenance auto off"

if ($SmokeProfile -eq "quick") {
    pass "quick profile complete"
    exit 0
}

# 17. GraphQL server
$SERVER_PORT = 19200 + (Get-Random -Maximum 800)
$PG_PORT = $SERVER_PORT + 1
$serverProc = Start-Process -FilePath $ZDB -ArgumentList "serve","--port","$SERVER_PORT","--pg-port","$PG_PORT" -PassThru -NoNewWindow

# Wait for server to start
$tokenPath = Join-Path $env:USERPROFILE ".config" "zetteldb" "token"
$TOKEN = if (Test-Path $tokenPath) { Get-Content $tokenPath -Raw } else { "" }
$TOKEN = $TOKEN.Trim()

for ($i = 0; $i -lt 20; $i++) {
    try {
        $null = Invoke-WebRequest -Uri "http://127.0.0.1:$SERVER_PORT/graphql" `
            -Method POST -ContentType "application/json" `
            -Headers @{ Authorization = "Bearer $TOKEN" } `
            -Body '{"query":"{ typeDefs { name } }"}' -ErrorAction Stop
        break
    } catch {
        Start-Sleep -Milliseconds 200
    }
}

$GQL_URL = "http://127.0.0.1:$SERVER_PORT/graphql"
$REST_URL = "http://127.0.0.1:$SERVER_PORT/rest"

function gql($body) {
    $resp = Invoke-WebRequest -Uri $GQL_URL -Method POST -ContentType "application/json" `
        -Headers @{ Authorization = "Bearer $TOKEN" } -Body $body -ErrorAction Stop
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

# Test auth
try {
    Invoke-WebRequest -Uri $GQL_URL -Method POST -ContentType "application/json" `
        -Body '{"query":"{ typeDefs { name } }"}' -ErrorAction Stop
    throw "should have been 401"
} catch {
    if ($_.Exception.Response.StatusCode.value__ -ne 401) { throw "expected 401, got $($_.Exception.Response.StatusCode.value__)" }
}
pass "serve: auth rejects missing token"

# Test query
$result = gql '{"query":"{ typeDefs { name } }"}'
if ($result -notmatch '"typeDefs"') { throw "graphql query failed" }
pass "serve: graphql query"

# Test mutation -- create
$result = gql '{"query":"mutation { createZettel(input: { title: \"Smoke Server\" }) { id title } }"}'
if ($result -notmatch '"Smoke Server"') { throw "graphql create failed" }
$GQL_ID = if ($result -match '"id":"([^"]+)"') { $Matches[1] } else { throw "no id in response" }
pass "serve: graphql create"

# 18. expanded GraphQL operations
$result = gql "{`"query`":`"mutation { updateZettel(input: { id: \`"$GQL_ID\`", title: \`"Smoke Updated\`" }) { id title } }`"}"
if ($result -notmatch '"Smoke Updated"') { throw "graphql update failed" }
pass "serve: graphql update"

$result = gql '{"query":"{ search(query: \"Smoke\") { totalCount hits { id title } } }"}'
if ($result -notmatch '"search"') { throw "graphql search failed" }
pass "serve: graphql search"

$result = gql '{"query":"{ zettels { id title } }"}'
if ($result -notmatch '"zettels"') { throw "graphql zettels failed" }
pass "serve: graphql zettels"

$result = gql "{`"query`":`"mutation { deleteZettel(id: \`"$GQL_ID\`") }`"}"
if ($result -notmatch "true") { throw "graphql delete failed" }
pass "serve: graphql delete"

# 18b. GraphQL checkbox queries
$result = gql '{"query":"{ openActions { state content } }"}'
if ($result -notmatch '"openActions"') { throw "graphql openActions failed" }
pass "serve: graphql openActions"

# 19. REST API CRUD
try {
    Invoke-WebRequest -Uri "$REST_URL/zettels" -Method POST -ContentType "application/json" `
        -Body '{"title":"REST No Auth"}' -ErrorAction Stop
    throw "should have been 401"
} catch {
    if ($_.Exception.Response.StatusCode.value__ -ne 401) { throw "expected 401" }
}
pass "rest: auth rejects missing token"

$resp = rest "/zettels" "POST" '{"title":"REST Smoke","body":"rest body","tags":["rest"]}'
if ($resp.StatusCode -ne 201) { throw "rest create expected 201" }
$REST_ID = if ($resp.Content -match '"id":"([^"]+)"') { $Matches[1] } else { throw "no id" }
pass "rest: create"

$resp = rest "/zettels/$REST_ID"
if ($resp.Content -notmatch "REST Smoke") { throw "rest get failed" }
pass "rest: get"

$resp = rest "/zettels/$REST_ID" "PUT" '{"title":"REST Updated"}'
if ($resp.Content -notmatch "REST Updated") { throw "rest update failed" }
pass "rest: update"

$resp = rest "/zettels?tag=rest"
if ($resp.Content -notmatch $REST_ID) { throw "rest list failed" }
pass "rest: list with filter"

# Field filtering
gql '{"query":"mutation{executeSql(sql:\"CREATE TABLE smokeitem (label TEXT NOT NULL, priority INTEGER)\"){message}}"}'
gql '{"query":"mutation{executeSql(sql:\"INSERT INTO smokeitem (label, priority) VALUES ('\''Smoke1'\'', 7)\"){message}}"}'
$resp = rest "/zettels?field.priority=7"
if ($resp.Content -notmatch "Smoke1") { throw "field filter failed" }
pass "rest: field filter"

$resp = rest "/zettels?field.priority=999"
if ($resp.Content -notmatch '"data":\[\]') { throw "field filter no-match failed" }
pass "rest: field filter no match"

$resp = Invoke-WebRequest -Uri "$REST_URL/zettels/$REST_ID" -Method DELETE `
    -Headers @{ Authorization = "Bearer $TOKEN" } -ErrorAction Stop
if ($resp.StatusCode -ne 204) { throw "rest delete expected 204" }
pass "rest: delete"

try {
    Invoke-WebRequest -Uri "$REST_URL/zettels/$REST_ID" -Method GET `
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
$result = gql "{`"query`":`"mutation { compact(force: true, backupPath: \`"$gqlBackup\`") { gcSuccess backupPath } }`"}"
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
    Invoke-RestMethod -Uri $url -Method Post -Headers $headers -Body '{"query":"mutation { createZettel(input: { title: \"ReadPoolWrite\" }) { id } }"}'
} -ArgumentList $GQL_URL, $TOKEN
$readResult = gql '{"query":"{ zettels { id title } }"}'
if ($readResult -notmatch "zettels") { throw "read-under-write: read failed" }
$writeResult = Receive-Job -Job $writeJob -Wait | ConvertTo-Json
if ($writeResult -notmatch "id") { throw "read-under-write: write failed" }
Remove-Job $writeJob
pass "serve: read-under-write (concurrent read + write)"

Stop-Process -Id $serverProc.Id -Force -ErrorAction SilentlyContinue
Start-Sleep -Milliseconds 500
pass "serve: clean shutdown"

Write-Host "=== sync conflict scenarios ==="

# --- Two-node setup ---
git init --bare $REMOTE_DIR 2>&1 | Out-Null

# node1: init + push
Push-Location $NODE1_DIR
zdb init . | Out-Null
git remote add origin $REMOTE_DIR
zdb register-node "Laptop" | Out-Null

# 21. fast-forward sync
$SYNC_ID = zdb create --title "Shared note" --tags "shared" --body "Original body"
git push -u origin master 2>&1 | Out-Null

# clone to node2
git clone $REMOTE_DIR $NODE2_DIR 2>&1 | Out-Null
Push-Location $NODE2_DIR
zdb reindex | Out-Null
zdb register-node "Desktop" | Out-Null

$output = zdb read $SYNC_ID
if ($output -notmatch "Shared note") { throw "fast-forward failed" }
pass "fast-forward sync"

# 22. non-overlapping edits
Pop-Location  # back to NODE1_DIR
zdb update $SYNC_ID --title "Updated Title" --tags "shared,laptop"

Push-Location $NODE2_DIR
zdb update $SYNC_ID --body "Modified body"

Pop-Location  # back to NODE1_DIR
zdb sync origin master | Out-Null

Push-Location $NODE2_DIR
$output = zdb sync origin master
if ($output -notmatch "conflicts resolved: 0") { throw "expected 0 conflicts" }

$output = zdb read $SYNC_ID
if ($output -notmatch "Updated Title") { throw "title not merged" }
if ($output -notmatch "Modified body") { throw "body not merged" }
pass "non-overlapping edits (clean merge)"

# 23. frontmatter scalar conflict (title)
Pop-Location  # NODE1_DIR
zdb sync origin master | Out-Null
zdb update $SYNC_ID --title "Laptop Title"

Push-Location $NODE2_DIR
zdb update $SYNC_ID --title "Desktop Title"

Pop-Location  # NODE1_DIR
zdb sync origin master | Out-Null

Push-Location $NODE2_DIR
$output = zdb sync origin master
if ($output -notmatch "conflicts resolved: 1") { throw "expected 1 conflict" }

$title = ((zdb read $SYNC_ID) -split "\r?\n") | Select-String "^title:"
if ($title -notmatch "(Laptop Title|Desktop Title)") { throw "title not resolved" }
pass "frontmatter scalar conflict (CRDT)"

# 24. frontmatter list conflict (tags)
Pop-Location  # NODE1_DIR
zdb sync origin master | Out-Null
zdb update $SYNC_ID --tags "base,alpha"

Push-Location $NODE2_DIR
zdb update $SYNC_ID --tags "base,beta"

Pop-Location  # NODE1_DIR
zdb sync origin master | Out-Null

Push-Location $NODE2_DIR
zdb sync origin master | Out-Null

$output = zdb read $SYNC_ID
if ($output -notmatch "alpha") { throw "alpha tag missing" }
if ($output -notmatch "beta") { throw "beta tag missing" }
pass "frontmatter list conflict (tag union)"

# 25. body conflict
Pop-Location  # NODE1_DIR
zdb sync origin master | Out-Null
zdb update $SYNC_ID --body "Line one LAPTOP.`nLine two.`nLine three."

Push-Location $NODE2_DIR
zdb update $SYNC_ID --body "Line one.`nLine two DESKTOP.`nLine three."

Pop-Location  # NODE1_DIR
zdb sync origin master | Out-Null

Push-Location $NODE2_DIR
$output = zdb sync origin master
if ($output -notmatch "conflicts resolved: 1") { throw "expected 1 conflict" }

$output = zdb read $SYNC_ID
if ($output -notmatch "LAPTOP") { throw "LAPTOP missing" }
if ($output -notmatch "DESKTOP") { throw "DESKTOP missing" }
pass "body conflict (CRDT text merge)"

# 26. reference section conflict
Pop-Location  # NODE1_DIR
zdb sync origin master | Out-Null

$ZETTEL_FILE = "zettelkasten/${SYNC_ID}.md"

$content = Get-Content $ZETTEL_FILE -Raw
Set-Content $ZETTEL_FILE -Value "$content`n---`n- laptop note:: Added from laptop`n" -NoNewline
git add $ZETTEL_FILE
git commit -m "node1 add reference" 2>&1 | Out-Null
git push origin master 2>&1 | Out-Null

Push-Location $NODE2_DIR
$content = Get-Content $ZETTEL_FILE -Raw
Set-Content $ZETTEL_FILE -Value "$content`n---`n- desktop note:: Added from desktop`n" -NoNewline
git add $ZETTEL_FILE
git commit -m "node2 add reference" 2>&1 | Out-Null

$output = zdb sync origin master
if ($output -notmatch "conflicts resolved: 1") { throw "expected 1 conflict" }

$output = zdb read $SYNC_ID
if ($output -notmatch "laptop note") { throw "laptop note missing" }
if ($output -notmatch "desktop note") { throw "desktop note missing" }
pass "reference section conflict (CRDT union)"

# 27b. delete-vs-edit conflict
Pop-Location  # NODE1_DIR
zdb sync origin master | Out-Null
$DEL_ID = zdb create --title "To be deleted" --body "Original content"
zdb sync origin master | Out-Null

Push-Location $NODE2_DIR
zdb sync origin master | Out-Null
$output = zdb read $DEL_ID
if ($output -notmatch "To be deleted") { throw "pre-delete read failed" }

# node1 deletes, node2 edits
Pop-Location  # NODE1_DIR
zdb delete $DEL_ID

Push-Location $NODE2_DIR
zdb update $DEL_ID --body "Edited on desktop"

Pop-Location  # NODE1_DIR
zdb sync origin master | Out-Null

Push-Location $NODE2_DIR
zdb sync origin master | Out-Null

$output = zdb read $DEL_ID
if ($output -notmatch "Edited on desktop") { throw "edit-wins failed" }
$output = zdb status
if ($output -notmatch "resurrected") { throw "resurrected missing" }
pass "delete-vs-edit conflict (edit wins, resurrected)"

Write-Host "=== bundle sync ==="

# 27. bundle export --full + import
Pop-Location  # NODE1_DIR
zdb sync origin master | Out-Null
zdb bundle export --full --output (Join-Path $TMPDIR "full-bundle.tar") | Out-Null

Push-Location $NODE3_DIR
zdb init . | Out-Null
zdb register-node "Tablet" | Out-Null
$output = zdb bundle import (Join-Path $TMPDIR "full-bundle.tar")
if ($output -notmatch "imported") { throw "bundle import failed" }
$output = zdb read $SYNC_ID
if ($output -notmatch "laptop note") { throw "bundle content missing" }
pass "bundle export --full + import"

# 28. delta bundle export + import
Pop-Location  # NODE1_DIR
$DELTA_ID = zdb create --title "Delta note" --body "only in delta"

$NODE2_UUID = Get-Content (Join-Path $NODE2_DIR ".git" "zdb-node") -Raw
$NODE2_UUID = $NODE2_UUID.Trim()
zdb bundle export --target $NODE2_UUID --output (Join-Path $TMPDIR "delta-bundle.tar") | Out-Null

Push-Location $NODE2_DIR
$output = zdb bundle import (Join-Path $TMPDIR "delta-bundle.tar")
if ($output -notmatch "imported") { throw "delta import failed" }
$output = zdb read $DELTA_ID
if ($output -notmatch "Delta note") { throw "delta content missing" }
pass "delta bundle export + import"

# 29. update-bin help
$output = zdb update-bin --help
if ($output -notmatch "Update zdb") { throw "update-bin help failed" }
pass "update-bin --help"

# 30. ALTER TABLE + DROP TABLE + bulk UPDATE/DELETE
Pop-Location  # back to TMPDIR
Push-Location $TMPDIR

$output = zdb query "CREATE TABLE smokealt (name TEXT, score INTEGER)"
if ($output -notmatch "table smokealt created") { throw "create smokealt failed" }
zdb query "INSERT INTO smokealt (name, score) VALUES ('a', 1)" | Out-Null
Start-Sleep -Seconds 1
zdb query "INSERT INTO smokealt (name, score) VALUES ('b', 2)" | Out-Null
$output = zdb query "ALTER TABLE smokealt ADD COLUMN tag TEXT"
if ($output -notmatch "altered") { throw "alter add failed" }
$output = zdb query "SELECT name, tag FROM smokealt"
if ($output -notmatch "NULL") { throw "null check failed" }
$output = zdb query "ALTER TABLE smokealt RENAME COLUMN tag TO label"
if ($output -notmatch "renamed") { throw "alter rename failed" }
$output = zdb query "SELECT name, label FROM smokealt"
if ($output -notmatch "a") { throw "select after rename failed" }
$output = zdb query "UPDATE smokealt SET score = 99 WHERE name = 'a'"
if ($output -notmatch "1 row\(s\) affected") { throw "bulk update failed" }
$output = zdb query "DELETE FROM smokealt WHERE name = 'b'"
if ($output -notmatch "1 row\(s\) affected") { throw "bulk delete failed" }
$output = zdb query "DROP TABLE smokealt CASCADE"
if ($output -notmatch "dropped") { throw "drop table failed" }
pass "alter/drop table + bulk ops"

# 31. file attachments
$attachFile = Join-Path $TMPDIR "zdb-smoke-attach.txt"
Set-Content $attachFile -Value "hello attachment"
$output = zdb attach $ID1 $attachFile
if ($output -notmatch "attached") { throw "attach failed" }
$output = zdb attachments $ID1
if ($output -notmatch "zdb-smoke-attach.txt") { throw "attachments list failed" }
if ($output -notmatch "text/plain") { throw "mime type wrong" }
$output = zdb query "SELECT name, mime FROM _zdb_attachments WHERE zettel_id = '$ID1'"
if ($output -notmatch "zdb-smoke-attach.txt") { throw "attach query failed" }
$output = zdb detach $ID1 "zdb-smoke-attach.txt"
if ($output -notmatch "detached") { throw "detach failed" }
$output = zdb attachments $ID1
if ($output -notmatch "no attachments") { throw "post-detach failed" }
Remove-Item $attachFile -ErrorAction SilentlyContinue
pass "file attachments (attach/list/query/detach)"

# 32. NoSQL CLI commands
$output = zdb get $ID1
if ($output -notmatch "First note \(edited\)") { throw "nosql get failed" }
pass "nosql: get"

$output = zdb scan --tag test
if ($output -notmatch $ID1) { throw "nosql scan --tag failed" }
pass "nosql: scan --tag"

$output = zdb scan --type foo
if ($output -notmatch "^\d{14}$") { throw "nosql scan --type failed" }
pass "nosql: scan --type"

$output = zdb backlinks $ID1
if ($output -notmatch $ID2) { throw "nosql backlinks failed" }
pass "nosql: backlinks"

# 33. stale node resync after compaction
Write-Host "=== stale node resync ==="
$script:STALE_REMOTE = New-TempDir
$script:STALE_N1 = New-TempDir
$script:STALE_N2 = New-TempDir

git init --bare $script:STALE_REMOTE 2>&1 | Out-Null

Push-Location $script:STALE_N1
zdb init . | Out-Null
git remote add origin $script:STALE_REMOTE
zdb register-node "StaleNode1" | Out-Null
$STALE_ID = zdb create --title "Stale shared" --body "original content"
git push -u origin master 2>&1 | Out-Null

git clone $script:STALE_REMOTE $script:STALE_N2 2>&1 | Out-Null
Push-Location $script:STALE_N2
zdb reindex | Out-Null
zdb register-node "StaleNode2" | Out-Null

# Both nodes edit the same zettel
Pop-Location  # STALE_N1
zdb update $STALE_ID --body "body from node1"
git push origin master 2>&1 | Out-Null

Push-Location $script:STALE_N2
zdb update $STALE_ID --body "body from node2"
zdb sync origin master | Out-Null

# Compact to remove CRDT temp files — verify report includes byte stats
$compactOut = zdb compact --force
if ($compactOut -notmatch "crdt temp:") { throw "compact missing crdt temp stats in stale test" }
if ($compactOut -notmatch "repo \(\.git\):") { throw "compact missing repo stats in stale test" }

# Create another conflict without CRDT state
Pop-Location  # STALE_N1
zdb sync origin master | Out-Null
zdb update $STALE_ID --body "second edit node1"
git push origin master 2>&1 | Out-Null

Push-Location $script:STALE_N2
zdb update $STALE_ID --body "second edit node2"
zdb sync origin master | Out-Null

# Verify zettel is readable and valid
$output = zdb read $STALE_ID
if ($output -notmatch "title:") { throw "stale resync failed" }
pass "stale node resync after compaction"

# 34. multi-row INSERT
Push-Location $TMPDIR
zdb query "CREATE TABLE multirow (name TEXT, val INTEGER)" | Out-Null
$MULTI_IDS = zdb query "INSERT INTO multirow (name, val) VALUES ('a', 1), ('b', 2), ('c', 3)"
if ($MULTI_IDS -notmatch "^\d{14},\d{14},\d{14}$") { throw "multi-row insert did not return 3 IDs: $MULTI_IDS" }
$count = zdb query "SELECT COUNT(*) FROM multirow"
if ($count -notmatch "3") { throw "expected 3 rows, got: $count" }
pass "multi-row insert"

# 35. transaction commit + rollback
Push-Location $TMPDIR
zdb query "CREATE TABLE txntest (val TEXT)" | Out-Null
$txnOut = zdb query "BEGIN; INSERT INTO txntest (val) VALUES ('committed'); COMMIT"
if ($txnOut -notmatch "COMMIT") { throw "transaction commit failed: $txnOut" }
$txnSel = zdb query "SELECT val FROM txntest"
if ($txnSel -notmatch "committed") { throw "committed row missing" }
$rbOut = zdb query "BEGIN; INSERT INTO txntest (val) VALUES ('rolled-back'); ROLLBACK"
if ($rbOut -notmatch "ROLLBACK") { throw "rollback failed: $rbOut" }
$txnCount = zdb query "SELECT COUNT(*) FROM txntest"
if ($txnCount -notmatch "1") { throw "expected 1 row after rollback, got: $txnCount" }
pass "transaction commit + rollback"

# 36. hyphenated type SQL via quoted identifiers
Push-Location $TMPDIR
zdb query 'CREATE TABLE "my-type" (label TEXT)' | Out-Null
$MY_ID = zdb query "INSERT INTO `"my-type`" (label) VALUES ('test')"
$hSel = zdb query "SELECT label FROM `"my-type`""
if ($hSel -notmatch "test") { throw "hyphenated select failed" }
$hDel = zdb query "DELETE FROM `"my-type`" WHERE id = '$MY_ID'"
if ($hDel -notmatch "1 row") { throw "hyphenated delete failed: $hDel" }
pass "hyphenated type SQL"

# Clean up location stack
while ($true) {
    try { Pop-Location } catch { break }
}

Cleanup
Write-Host "=== all passed ==="
