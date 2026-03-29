#!/usr/bin/env pwsh
param(
    [ValidateSet("quick", "full")]
    [string]$SmokeProfile = $(if ($env:SMOKE_PROFILE) { $env:SMOKE_PROFILE } else { "full" })
)

# Windows smoke test — PowerShell port of tests/smoke.sh
$ErrorActionPreference = "Stop"

# Build and lint for the full profile when DDB_BIN is not injected.
$prepLabel = "prebuilt binary"
if (-not $env:DDB_BIN) {
    cargo build --quiet
    if ($SmokeProfile -eq "full") {
        cargo clippy --workspace --quiet
        cargo bench --no-run --quiet 2>$null
        $prepLabel = "clippy + bench compile"
    } else {
        $prepLabel = "build"
    }
}

if ($env:DDB_BIN) {
    $DDB = $env:DDB_BIN
} else {
    $meta = cargo metadata --format-version=1 --no-deps | ConvertFrom-Json
    $DDB = Join-Path $meta.target_directory "debug" "ddb.exe"
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
    # On CI, skip cleanup — the runner wipes the workspace. Local Remove-Item on
    # Windows git repos can hang for minutes due to file locks and antivirus scans.
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
    # Filter out tracing log lines that leak from stderr via 2>&1 on Windows.
    # ErrorRecord objects from stderr may contain ANSI escapes, so strip those
    # before matching the timestamp pattern.
    $lines = @($raw) | ForEach-Object { "$_" -replace '\x1b\[[0-9;]*m', '' } |
        Where-Object { $_ -notmatch '^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}' -and $_ -ne '' }
    $text = [string]::Join("`n", @($lines))
    if ($LASTEXITCODE -ne 0) { throw "ddb $($args -join ' ') failed: $text" }
    return $text
}

# Expect failure: returns true if command fails
function ddb-fails {
    & $DDB @args 2>&1 | Out-Null
    return ($LASTEXITCODE -ne 0)
}

Write-Host "=== smoke test ($SmokeProfile) ==="

pass $prepLabel

# 1. init
ddb init . | Out-Null
pass "init"

# 2. create doogats
$ID1 = ddb create --title "First note" --tags "test,smoke" --body "Hello world"
$ID2 = ddb create --title "Links to first" --body "See [[$ID1]]"
$ID3 = ddb create --title "Project Alpha" --type project --tags "active" --body "A project doogat"
if ($ID1 -eq $ID2 -or $ID2 -eq $ID3 -or $ID1 -eq $ID3) { throw "IDs not unique" }
pass "create (3 unique IDs: $ID1 $ID2 $ID3)"

# 3. read
$output = ddb read $ID1
if ($output -notmatch "First note") { throw "read failed" }
pass "read"

# 4. update
ddb update $ID1 --title "First note (edited)" --tags "test,smoke,updated"
$output = ddb read $ID1
if ($output -notmatch "First note \(edited\)") { throw "update failed" }
pass "update"

# 5. delete
ddb delete $ID3
if (-not (ddb-fails read $ID3)) { throw "read after delete should fail" }
if (-not (ddb-fails delete "99999999999999")) { throw "delete nonexistent should fail" }
pass "delete"

# 6. status
$output = ddb status
if ($output -notmatch "^head:") { throw "status missing head" }
pass "status"

# 6b. broken backlink report on delete
$BL_TARGET = ddb create --title "Backlink Target" --body "I will be deleted"
Start-Sleep -Seconds 1
$BL_SOURCE = ddb create --title "Backlink Source" --body "See [[$BL_TARGET]]"
ddb reindex | Out-Null
$output = ddb delete $BL_TARGET
if ($output -notmatch "broken backlinks") { throw "delete missing broken backlink report" }
$output = ddb status
if ($output -notmatch "broken backlinks") { throw "status missing broken backlinks" }
# Clean up: delete source so broken backlinks don't affect later tests
ddb delete $BL_SOURCE | Out-Null
pass "broken backlink report on delete"

# 7. reindex
$output = ddb reindex
if ($output -notmatch "indexed 2 doogats") { throw "reindex count wrong" }
pass "reindex"

# 7b. hashtag extraction
ddb update $ID1 --body "Updated with #gtd/act/next hashtag"
ddb reindex | Out-Null
$output = ddb query "SELECT tag, source FROM _ddb_tags WHERE tag = 'gtd/act/next'"
if ($output -notmatch "body") { throw "hashtag not indexed" }
pass "hashtag extraction and indexing"

# 7c. checkbox parsing
ddb update $ID1 --body "- [ ] open task`n- [x] done task`n- [i] 2026-01-01 10:00 - info note"
ddb reindex | Out-Null
$output = ddb query "SELECT state, content FROM _ddb_checkboxes WHERE state = 'open'"
if ($output -notmatch "open task") { throw "checkbox not indexed" }
pass "checkbox parsing and indexing"

# 7d. folder namespace
& $DDB query "CREATE TABLE widget (color TEXT)" | Out-Null
$widgetTypedef = Get-ChildItem "$TMPDIR/ddb/_typedef/*.md" | Where-Object { (Get-Content $_) -match "title: widget" } | Select-Object -First 1
(Get-Content $widgetTypedef.FullName -Raw) -replace "type: _typedef", "type: _typedef`nfolder: true" | Set-Content $widgetTypedef.FullName
git -C $TMPDIR add -A 2>$null; git -C $TMPDIR commit -m "add folder to widget" 2>$null | Out-Null
& $DDB reindex | Out-Null
$widgetId = (& $DDB query "INSERT INTO widget (color) VALUES ('red')").Trim()
if (-not (Test-Path "$TMPDIR/ddb/widget/$widgetId.md")) { throw "folder namespace: file not in subdirectory" }
pass "folder namespace: typed doogat in subdirectory"

# 8. full-text search
$output = ddb search "First note"
if ($output -notmatch $ID1) { throw "search failed" }
pass "search"

# 8b. paginated search
$output = ddb search "First note" --limit 1 --offset 0
if ($output -notmatch "Showing 1-1 of") { throw "paginated search failed" }
pass "paginated search"

# 9. SQL queries
$output = ddb query "SELECT id, title FROM doogats"
if ($output -notmatch "First note \(edited\)") { throw "sql select failed" }
$output = ddb query "SELECT z.id, z.title FROM doogats z JOIN _ddb_tags t ON t.doogat_id = z.id WHERE t.tag LIKE '%smoke%'"
if ($output -notmatch $ID1) { throw "sql join failed" }
pass "sql queries"

# 10. wikilinks
$output = ddb query "SELECT * FROM _ddb_links"
if ($output -notmatch $ID1) { throw "wikilinks failed" }
pass "wikilinks"

# 10a. link kinds (wikilink, markdown, embed, bare_url)
$lkBody = "See [[$ID1]] wiki.`n[md link](target.md)`n![[$ID2]]`nhttps://example.com"
$LK_ID = ddb create --title "Link Kinds" --body $lkBody
ddb reindex | Out-Null
$lkOut = ddb query "SELECT kind FROM _ddb_links WHERE source_id = '$LK_ID' ORDER BY kind"
if ($lkOut -notmatch "\burl\b") { throw "url kind missing" }
if ($lkOut -notmatch "embed") { throw "embed kind missing" }
if ($lkOut -notmatch "markdown") { throw "markdown kind missing" }
if ($lkOut -notmatch "wikilink") { throw "wikilink kind missing" }
pass "link kinds (4 types indexed)"

# 10b. rename with backlink rewrite
$RENAME_TARGET = ddb create --title "Rename Target" --body "I will move."
ddb create --title "Rename Linker" --body "See [[$RENAME_TARGET|Target]]." | Out-Null
ddb reindex | Out-Null
$output = ddb rename $RENAME_TARGET "ddb/contact/${RENAME_TARGET}.md"
if ($output -notmatch "1 backlinks updated") { throw "rename failed" }
if (-not (Test-Path "ddb/contact/${RENAME_TARGET}.md")) { throw "renamed file missing" }
pass "rename with backlink rewrite"

# 11. SQL DDL/DML
$output = ddb query "CREATE TABLE foo (bar TEXT, baz INTEGER)"
if ($output -notmatch "table foo created") { throw "create table failed" }
$FOO_ID = ddb query "INSERT INTO foo (title, bar, baz) VALUES ('test row', 'hello', 42)"
if ($FOO_ID -is [array]) { Write-Host "DEBUG: FOO_ID is array with $($FOO_ID.Count) elements: $($FOO_ID -join '|')"; $FOO_ID = $FOO_ID[-1] }
if ($FOO_ID -notmatch "^\d{14}$") { throw "insert returned bad id: [$FOO_ID] (type=$($FOO_ID.GetType().Name))" }
$output = ddb query "SELECT bar, baz FROM foo"
if ($output -notmatch "hello") { throw "select from foo failed" }
$output = ddb query "UPDATE foo SET baz = 99 WHERE id = '$FOO_ID'"
if ($output -notmatch "1 row\(s\) affected") { throw "update failed" }
$output = ddb query "SELECT baz FROM foo WHERE id = '$FOO_ID'"
if ($output -notmatch "99") { throw "select after update failed" }
$output = ddb query "DELETE FROM foo WHERE id = '$FOO_ID'"
if ($output -notmatch "1 row\(s\) affected") { throw "delete failed" }
pass "sql ddl/dml"

# 11a. ALTER TABLE SET ZONE and TITLE TEMPLATE
$output = ddb query "ALTER TABLE foo SET ZONE frontmatter FOR bar"
if ($output -notmatch "zone set to frontmatter") { throw "SET ZONE failed" }
$output = ddb query "ALTER TABLE foo SET TITLE TEMPLATE 'my-template'"
if ($output -notmatch "title template set") { throw "SET TITLE TEMPLATE failed" }
$output = ddb query "ALTER TABLE foo DROP TITLE TEMPLATE"
if ($output -notmatch "title template dropped") { throw "DROP TITLE TEMPLATE failed" }
pass "alter table zone overrides and title template"

# 11b. CREATE TABLE IF NOT EXISTS (idempotent)
$output = ddb query "CREATE TABLE IF NOT EXISTS foo (bar TEXT, baz INTEGER)"
if ($output -notmatch "already exists") { throw "IF NOT EXISTS on existing table failed" }
$output = ddb query "CREATE TABLE IF NOT EXISTS newifne (x TEXT)"
if ($output -notmatch "table newifne created") { throw "IF NOT EXISTS new table failed" }
$output = ddb query "CREATE TABLE IF NOT EXISTS newifne (x TEXT)"
if ($output -notmatch "already exists") { throw "IF NOT EXISTS idempotent failed" }
pass "create table if not exists (idempotent)"

# 12. install bundled type
$output = ddb type install contact
if ($output -notmatch "installed type") { throw "type install failed" }
pass "type install"

# 12a. hyphenated type SQL (quoted identifiers)
$output = ddb type install meeting-minutes
if ($output -notmatch "installed type") { throw "meeting-minutes install failed" }
$HYP_ID = (ddb query 'INSERT INTO "meeting-minutes" (date, attendees) VALUES (''2026-03-10'', ''alice,bob'')').Trim()
$output = ddb query "SELECT date FROM `"meeting-minutes`" WHERE id = '$HYP_ID'"
if ($output -notmatch "2026-03-10") { throw "hyphenated select failed" }
$output = ddb query "DELETE FROM `"meeting-minutes`" WHERE id = '$HYP_ID'"
if ($output -notmatch "1 row") { throw "hyphenated delete failed" }
pass "hyphenated type sql (quoted identifiers)"

# 13. type suggest
ddb query "INSERT INTO foo (title, bar, baz) VALUES ('for suggest', 'val', 1)" | Out-Null
$output = ddb type suggest foo
if ($output -notmatch "bar") { throw "type suggest failed" }
pass "type suggest"

# 14. register node + compact
$output = ddb register-node "smoke-test-laptop"
if ($output -notmatch "registered node") { throw "register-node failed" }
$output = ddb status
if ($output -notmatch "registered nodes: 1") { throw "status missing node" }
$output = ddb compact --force
if ($output -notmatch "backup:") { throw "compact missing backup path" }
if ($output -notmatch "gc: ok") { throw "compact failed" }
if ($output -notmatch "crdt temp:") { throw "compact missing crdt temp stats" }
if ($output -notmatch "repo \(\.git\):") { throw "compact missing repo stats" }
pass "register-node + compact"

# 15. node list + retire
$output = ddb node list
if ($output -notmatch "smoke-test-laptop") { throw "node list failed" }
$NODE_UUID = (($output -split "\r?\n") | Select-String "smoke-test-laptop").ToString().Split()[0]
$output = ddb node retire $NODE_UUID
if ($output -notmatch "retired node") { throw "node retire failed" }
pass "node list + retire"

# 16. compact --dry-run
$output = ddb compact --dry-run
if ($output -notmatch "dry run") { throw "compact dry-run failed" }
if ($output -notmatch "backup would write:") { throw "dry-run missing backup info" }
pass "compact --dry-run"

# 16a. compact --no-backup
ddb register-node "no-backup-test" | Out-Null
$output = ddb compact --no-backup --force
if ($output -notmatch "gc: ok") { throw "compact --no-backup failed" }
if ($output -match "backup:") { throw "--no-backup should suppress backup" }
pass "compact --no-backup"

# 16b. compact --backup-path
$customBackup = Join-Path $env:TEMP "custom-backup.bundle.tar"
$output = ddb compact --force --backup-path $customBackup
if ($output -notmatch "backup:") { throw "compact --backup-path missing backup line" }
if ($output -notmatch [regex]::Escape($customBackup)) { throw "compact --backup-path wrong path" }
if (-not (Test-Path $customBackup)) { throw "custom backup file not created" }
pass "compact --backup-path"

# 16c. maintenance
$output = ddb maintenance run
if ($output -notmatch "maintenance:") { throw "maintenance run missing output" }
pass "maintenance run"

$output = ddb maintenance auto status
if ($output -notmatch "off") { throw "maintenance auto status should default to off" }
pass "maintenance auto status (default off)"

ddb maintenance auto on | Out-Null
$output = ddb maintenance auto status
if ($output -notmatch "on") { throw "maintenance auto status should be on" }
pass "maintenance auto on"

ddb maintenance auto off | Out-Null
$output = ddb maintenance auto status
if ($output -notmatch "off") { throw "maintenance auto status should be off" }
pass "maintenance auto off"

# 16d. discover
ddb discover stale | Out-Null
pass "discover stale"

$output = ddb discover orphans
if ($output -notmatch "\d{14}") { throw "discover orphans returned nothing" }
pass "discover orphans"

# Create a doogat that mentions ID1's title without linking
$MENTION_ID = ddb create --title "Review notes" --body "About First note (edited) topic"
ddb reindex | Out-Null
$output = ddb discover mentions $ID1
if ($output -notmatch $MENTION_ID) { throw "discover mentions failed" }
pass "discover mentions"

$output = ddb discover similar $ID1
if ($output -notmatch "\d{14}") { throw "discover similar returned nothing" }
pass "discover similar"

# 16e. consistency fix
$FIX_ID = ddb create --title "Fix Test" --tags "#gtd,zebra,apple"
$beforeHead = git rev-parse HEAD
$fixDry = ddb fix --dry-run
if ($fixDry -notmatch "would fix") { throw "dry run should report fixes" }
$afterHead = git rev-parse HEAD
if ($beforeHead -ne $afterHead) { throw "dry run should not commit" }
pass "fix dry-run"

$fixApply = ddb fix
if ($fixApply -notmatch "fixed") { throw "fix should report applied" }
pass "fix apply"

$fixAgain = ddb fix
if ($fixAgain -notmatch "no issues") { throw "second fix should find nothing" }
pass "fix idempotent"

$fixContent = ddb read $FIX_ID
if ($fixContent -notmatch "  - apple") { throw "tags should be sorted" }
pass "fix result verified"

# 16f. sequence navigation
$SEQ_ROOT = ddb create --title "Seq Root"
$SEQ_CHILD1 = ddb create --title "Seq Child 1"
$SEQ_CHILD1_PATH = "ddb/$SEQ_CHILD1.md"
@"
---
id: $SEQ_CHILD1
title: Seq Child 1
sequence: $SEQ_ROOT
---

"@ | Set-Content -Path $SEQ_CHILD1_PATH -NoNewline
git add $SEQ_CHILD1_PATH
git commit -m "add sequence field" --quiet

$SEQ_CHILD2 = ddb create --title "Seq Child 2"
$SEQ_CHILD2_PATH = "ddb/$SEQ_CHILD2.md"
@"
---
id: $SEQ_CHILD2
title: Seq Child 2
sequence: $SEQ_ROOT
---

"@ | Set-Content -Path $SEQ_CHILD2_PATH -NoNewline
git add $SEQ_CHILD2_PATH
git commit -m "add sequence field" --quiet

ddb reindex | Out-Null
$output = ddb sequence tree $SEQ_ROOT
if ($output -notmatch $SEQ_CHILD1) { throw "sequence tree missing child" }
pass "sequence tree"

$output = ddb sequence breadcrumb $SEQ_CHILD1
if ($output -notmatch $SEQ_ROOT) { throw "sequence breadcrumb missing root" }
pass "sequence breadcrumb"

$SEQ_BROKEN = ddb create --title "Seq Broken"
$SEQ_BROKEN_PATH = "ddb/$SEQ_BROKEN.md"
@"
---
id: $SEQ_BROKEN
title: Seq Broken
sequence: "99999999999999"
---

"@ | Set-Content -Path $SEQ_BROKEN_PATH -NoNewline
git add $SEQ_BROKEN_PATH
git commit -m "broken sequence ref" --quiet
ddb reindex | Out-Null
$output = ddb sequence broken
if ($output -notmatch "not found") { throw "sequence broken not detected" }
pass "sequence broken"

# 16b. --log-level flag accepted
ddb --log-level debug status *>$null
pass "--log-level flag accepted"

# 16d. app-building end-to-end flow
$output = ddb query "CREATE TABLE abcategory (name VARCHAR(100), priority ENUM('low','medium','high'))"
if ($output -notmatch "table abcategory created") { throw "create abcategory failed" }
$AB_CAT_ID = ddb query "INSERT INTO abcategory (name, priority) VALUES ('work', 'high')"
if ($AB_CAT_ID -notmatch "^\d{14}$") { throw "insert abcategory bad id: $AB_CAT_ID" }
$output = ddb query "CREATE TABLE abbookmark (url VARCHAR(2048), description TEXT, abcategory TEXT REFERENCES abcategory)"
if ($output -notmatch "table abbookmark created") { throw "create abbookmark failed" }
$output = ddb query "ALTER TABLE abbookmark SET ZONE reference FOR url"
if ($output -notmatch "zone set to reference") { throw "SET ZONE failed" }
$output = ddb query "ALTER TABLE abbookmark SET TITLE TEMPLATE '{url}'"
if ($output -notmatch "title template set") { throw "SET TITLE TEMPLATE failed" }
# Insert with explicit title
Start-Sleep -Seconds 1
$AB_BM1 = ddb query "INSERT INTO abbookmark (title, url, description) VALUES ('Rust Book', 'https://doc.rust-lang.org', 'The official Rust book')"
if ($AB_BM1 -notmatch "^\d{14}$") { throw "insert bookmark1 bad id: $AB_BM1" }
# Insert with template-derived title (no explicit title)
Start-Sleep -Seconds 1
$AB_BM2 = ddb query "INSERT INTO abbookmark (url, description) VALUES ('https://crates.io', 'Rust package registry')"
if ($AB_BM2 -notmatch "^\d{14}$") { throw "insert bookmark2 bad id: $AB_BM2" }
# Link both bookmarks to category via junction table
$output = ddb query "INSERT INTO abbookmark_abcategory (abbookmark_id, abcategory_id) VALUES ('$AB_BM1', '$AB_CAT_ID')"
if ($output -notmatch "1 row") { throw "junction insert bm1 failed: $output" }
$output = ddb query "INSERT INTO abbookmark_abcategory (abbookmark_id, abcategory_id) VALUES ('$AB_BM2', '$AB_CAT_ID')"
if ($output -notmatch "1 row") { throw "junction insert bm2 failed: $output" }
# SELECT from main table
$output = ddb query "SELECT url FROM abbookmark"
if ($output -notmatch "rust-lang") { throw "select bookmark failed" }
# SELECT from junction table — both bookmarks linked
$output = ddb query "SELECT COUNT(*) FROM abbookmark_abcategory"
if ($output -notmatch "2") { throw "junction count wrong: $output" }
# Verify ENUM allowed_values stored in typedef
$output = ddb query "SELECT priority FROM abcategory"
if ($output -notmatch "high") { throw "ENUM priority not stored: $output" }
# help create-app guide available
$output = ddb help create-app
if ($output -notmatch "CREATE TABLE") { throw "help create-app failed" }
# Clean up
$output = ddb query "DROP TABLE abbookmark CASCADE"
if ($output -notmatch "dropped") { throw "drop abbookmark failed" }
$output = ddb query "DROP TABLE abcategory CASCADE"
if ($output -notmatch "dropped") { throw "drop abcategory failed" }
pass "app-building end-to-end flow"

if ($SmokeProfile -eq "quick") {
    pass "quick profile complete"
    exit 0
}

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

# 18e. search where-filter (field predicates)
gql '{"query":"mutation { executeSql(sql: \"CREATE TABLE sfitem (status VARCHAR(20))\") { message } }"}' | Out-Null
$sfw1 = gql "{`"query`":`"mutation { executeSql(sql: \`"INSERT INTO sfitem (title, status) VALUES ('WhereTest Active', 'active')\`") { message } }`"}"
$SF_W1_ID = if ($sfw1 -match '"message":"([^"]+)"') { $Matches[1] }
$sfw2 = gql "{`"query`":`"mutation { executeSql(sql: \`"INSERT INTO sfitem (title, status) VALUES ('WhereTest Archived', 'archived')\`") { message } }`"}"
$SF_W2_ID = if ($sfw2 -match '"message":"([^"]+)"') { $Matches[1] }

$result = gql '{"query":"{ search(query: \"WhereTest\", where: [{ field: \"status\", eq: \"active\" }]) { totalCount hits { id } } }"}'
if ($result -notmatch '"totalCount":1') { throw "search where eq: expected 1, got $result" }
pass "serve: search where-filter eq"

$result = gql '{"query":"{ search(query: \"WhereTest\", where: [{ field: \"status\", contains: \"arch\" }]) { totalCount hits { id } } }"}'
if ($result -notmatch '"totalCount":1') { throw "search where contains: expected 1, got $result" }
pass "serve: search where-filter contains"

gql "{`"query`":`"mutation { executeSql(sql: \`"DELETE FROM sfitem WHERE id = '$SF_W1_ID'\`") { message } }`"}" | Out-Null
gql "{`"query`":`"mutation { executeSql(sql: \`"DELETE FROM sfitem WHERE id = '$SF_W2_ID'\`") { message } }`"}" | Out-Null
gql '{"query":"mutation { executeSql(sql: \"DROP TABLE sfitem CASCADE\") { message } }"}' | Out-Null

# 18f. REFERENCES relation resolution
gql '{"query":"mutation { executeSql(sql: \"CREATE TABLE smokecat (label TEXT)\") { message } }"}' | Out-Null
gql '{"query":"mutation { executeSql(sql: \"CREATE TABLE smokebm (url TEXT, smokecat TEXT REFERENCES smokecat)\") { message } }"}' | Out-Null
$scat = gql "{`"query`":`"mutation { executeSql(sql: \`"INSERT INTO smokecat (title, label) VALUES ('Tech', 'tech')\`") { message } }`"}"
$SCAT_ID = if ($scat -match '"message":"([^"]+)"') { $Matches[1] }
Start-Sleep -Seconds 1
$sbm = gql "{`"query`":`"mutation { executeSql(sql: \`"INSERT INTO smokebm (title, url) VALUES ('Example', 'https://example.com')\`") { message } }`"}"
$SBM_ID = if ($sbm -match '"message":"([^"]+)"') { $Matches[1] }
gql "{`"query`":`"mutation { executeSql(sql: \`"INSERT INTO smokebm_smokecat (smokebm_id, smokecat_id) VALUES ('$SBM_ID', '$SCAT_ID')\`") { message } }`"}" | Out-Null

# Singular: resolves as object
$result = gql '{"query":"{ smokebms { items { smokecat { id label } } } }"}'
if ($result -notmatch '"label":"tech"') { throw "singular relation resolution failed: $result" }
pass "serve: relation singular resolves object"

# Plural: resolves as list of objects
$result = gql '{"query":"{ smokebms { items { smokecats { id label } } } }"}'
if ($result -notmatch '"label":"tech"') { throw "plural relation resolution failed: $result" }
pass "serve: relation plural resolves object list"

# Null reference: bookmark without link
Start-Sleep -Seconds 1
gql "{`"query`":`"mutation { executeSql(sql: \`"INSERT INTO smokebm (title, url) VALUES ('No Cat', 'https://nocat.com')\`") { message } }`"}" | Out-Null
$result = gql '{"query":"{ smokebms { items { id smokecat { id } smokecats { id } } } }"}'
if ($result -notmatch '"smokecat":null') { throw "null relation failed: $result" }
pass "serve: relation null returns null"

gql '{"query":"mutation { executeSql(sql: \"DROP TABLE smokebm CASCADE\") { message } }"}' | Out-Null
gql '{"query":"mutation { executeSql(sql: \"DROP TABLE smokecat CASCADE\") { message } }"}' | Out-Null

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
$mvCat1 = (gql '{"query":"mutation{executeSql(sql:\"INSERT INTO mvcategory (name) VALUES (''Science'')\"){message}}"}') -replace '.*"message":"(\d+)".*','$1'
$mvCat2 = (gql '{"query":"mutation{executeSql(sql:\"INSERT INTO mvcategory (name) VALUES (''Math'')\"){message}}"}') -replace '.*"message":"(\d+)".*','$1'
$mvBm = (gql "{`"query`":`"mutation{executeSql(sql:\`"INSERT INTO mvbookmark (mvcategory) VALUES ('$mvCat1')\`"){message}}`"}") -replace '.*"message":"(\d+)".*','$1'
gql "{`"query`":`"mutation{executeSql(sql:\`"INSERT INTO mvbookmark_mvcategory (mvbookmark_id, mvcategory_id) VALUES ('$mvBm', '$mvCat2')\`"){message}}`"}" | Out-Null
$mvResult = gql '{"query":"{ mvbookmarks { items { id mvcategories } } }"}'
if ($mvResult -notmatch $mvCat1) { throw "multi-value ref: cat1 not in graphql list field" }
if ($mvResult -notmatch $mvCat2) { throw "multi-value ref: cat2 not in graphql list field" }
pass "serve: graphql multi-value ref list field"
$mvRest = rest "/doogats/$mvBm"
if ($mvRest -notmatch '"references"') { throw "multi-value ref: no references in rest json" }
if ($mvRest -notmatch '"mvcategory"') { throw "multi-value ref: no category key in references" }
pass "serve: rest multi-value ref structured json"

# 38c. sql-materialization (columns, boolean normalization, core fields)
$result = gql '{"query":"{ sql(query: \"SELECT id, title FROM doogats\") { columns rows } }"}'
if ($result -notmatch '"columns"') { throw "sql columns: missing columns field" }
if ($result -notmatch '"id"') { throw "sql columns: missing id column" }
if ($result -notmatch '"title"') { throw "sql columns: missing title column" }
pass "serve: sql columns in response"

gql '{"query":"mutation{executeSql(sql:\"CREATE TABLE smokepin (pinned BOOLEAN)\"){message}}"}' | Out-Null
$smokepinId = (gql "{`"query`":`"mutation{executeSql(sql:\`"INSERT INTO smokepin (title, pinned) VALUES ('PinTest', true)\`"){message}}`"}") -replace '.*"message":"(\d+)".*','$1'
if (-not $smokepinId) { throw "smokepin insert failed" }
$result = gql "{`"query`":`"{ sql(query: \`"SELECT pinned FROM smokepin WHERE pinned = 1\`") { rows } }`"}"
if ($result -notmatch '[\\"]1[\\"]') { throw "boolean not normalized to 1" }
pass "serve: boolean normalized to 1/0"

$result = gql '{"query":"{ sql(query: \"SELECT title FROM smokepin\") { rows } }"}'
if ($result -notmatch 'PinTest') { throw "core fields: title missing from type table" }
pass "serve: core fields in type table"

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

# Return to original directory
Set-Location $TMPDIR

Cleanup
Write-Host "=== all passed ==="
