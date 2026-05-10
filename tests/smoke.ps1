#!/usr/bin/env pwsh

# Smoke test: fast CLI validation (init, CRUD, search, SQL, types, compact).
# For full integration tests (server, sync, CRDT), run tests/integration.ps1.

$ErrorActionPreference = "Stop"

# Build when DDB_BIN is not injected.
$prepLabel = "prebuilt binary"
if (-not $env:DDB_BIN) {
    cargo build --quiet
    $prepLabel = "build"
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

function Cleanup {
    # On CI, skip cleanup — the runner wipes the workspace. Local Remove-Item on
    # Windows git repos can hang for minutes due to file locks and antivirus scans.
    if ($env:CI) { return }
    if (Test-Path $TMPDIR) { Remove-Item -Recurse -Force $TMPDIR -ErrorAction SilentlyContinue }
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

Write-Host "=== smoke test ==="

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

# 11c. Ghost-row + cross-mutation recovery pin (PRD 00124 group A1, issue #4).
$output = ddb query "CREATE TABLE smokeghost (name TEXT, UNIQUE(name))"
if ($output -notmatch "table smokeghost created") { throw "smokeghost create failed" }
$SG_ID = ddb query "INSERT INTO smokeghost (title, name) VALUES ('first', 'uq_a')"
if ($SG_ID -is [array]) { $SG_ID = $SG_ID[-1] }
if ($SG_ID -notmatch "^\d{14}$") { throw "smokeghost insert returned bad id: [$SG_ID]" }
$dup = & $DDB query "INSERT INTO smokeghost (title, name) VALUES ('dup', 'uq_a')" 2>&1 | Out-String
if ($LASTEXITCODE -eq 0) { throw "smokeghost duplicate should have failed, got: $dup" }
if ($dup -notmatch "UNIQUE") { throw "smokeghost duplicate should error with UNIQUE, got: $dup" }
$output = ddb query "UPDATE smokeghost SET title = 'recovered' WHERE id = '$SG_ID'"
if ($output -notmatch "1 row\(s\) affected") { throw "smokeghost post-failure UPDATE failed: $output" }
$output = ddb query "SELECT title FROM smokeghost WHERE id = '$SG_ID'"
if ($output -notmatch "recovered") { throw "smokeghost SELECT returned unexpected: $output" }
$output = ddb query "DROP TABLE smokeghost CASCADE"
if ($output -notmatch "dropped") { throw "smokeghost DROP failed" }
pass "smoke-ghost: UNIQUE rollback + cross-mutation recovery (issue #4 A1)"

# 11d. JOIN works smoke pin (PRD 00124 group E1, issue #8 obsolete).
$output = ddb query "CREATE TABLE smokelink (url TEXT)"
if ($output -notmatch "table smokelink created") { throw "smokelink create failed" }
$output = ddb query "CREATE TABLE smokenum (count INTEGER)"
if ($output -notmatch "table smokenum created") { throw "smokenum create failed" }
ddb query "INSERT INTO smokelink (title, url) VALUES ('a', 'https://a.com')" | Out-Null
ddb query "INSERT INTO smokenum (title, count) VALUES ('a', 1)" | Out-Null
$output = ddb query "SELECT l.title, n.count FROM smokelink l JOIN smokenum n ON l.title = n.title"
if ($output -notmatch "a \| 1") { throw "smoke-join: expected 'a | 1', got: $output" }
$output = ddb query "DROP TABLE smokelink CASCADE"
if ($output -notmatch "dropped") { throw "smokelink DROP failed" }
$output = ddb query "DROP TABLE smokenum CASCADE"
if ($output -notmatch "dropped") { throw "smokenum DROP failed" }
pass "smoke-join: SELECT ... JOIN returns joined rows (issue #8 obsolete)"

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

# 11e. ALTER TABLE RENAME TO (PRD 00132)
$output = ddb query "CREATE TABLE smokerename_src (note TEXT)"
if ($output -notmatch "table smokerename_src created") { throw "rename src create failed" }
ddb query "INSERT INTO smokerename_src (title, note) VALUES ('hi', 'first')" | Out-Null
$output = ddb query "ALTER TABLE smokerename_src RENAME TO smokerename_dst"
if ($output -notmatch "renamed to smokerename_dst") { throw "rename to failed: $output" }
$output = ddb query "SELECT count(*) FROM smokerename_dst"
if ($output -notmatch "1") { throw "row count after rename failed: $output" }
# Bypass the throwing `ddb` wrapper and call the binary directly so a non-zero
# exit (the expected reserved-name rejection) doesn't abort the script.
$output = (& $DDB query "ALTER TABLE smokerename_dst RENAME TO doogats" 2>&1 | Out-String)
if ($output -notmatch "reserved") { throw "expected reserved-name rejection, got: $output" }
$output = ddb query "DROP TABLE smokerename_dst CASCADE"
if ($output -notmatch "dropped") { throw "rename cleanup drop failed" }
pass "alter table rename to + reserved-name rejection"

# 11f. Cross-process FK freshness on `ddb create` (PRD 00136, #16).
# Each `ddb` call here spawns a fresh process. The second `ddb create`
# must resolve the FK against the category created by the first call
# WITHOUT an intermediate `ddb reindex` — that's the whole point of #16.
# The `ddb` wrapper at the top of this file checks $LASTEXITCODE and
# throws on non-zero exit, so a regression where the second `ddb create`
# rejects with REFERENCES_VIOLATION aborts this script via the wrapper.
$output = ddb query "CREATE TABLE smoke136cat (fqn VARCHAR(255))"
if ($output -notmatch "table smoke136cat created") { throw "smoke136cat create failed" }
$output = ddb query "CREATE TABLE smoke136link (url TEXT, category TEXT REFERENCES smoke136cat)"
if ($output -notmatch "table smoke136link created") { throw "smoke136link create failed" }
$SMOKE136_CAT = (ddb create --type smoke136cat --title "Cat 136" --set "fqn=test.fqn").Trim()
if ($SMOKE136_CAT -notmatch '^\d{14}$') { throw "category id malformed: $SMOKE136_CAT" }
ddb create --type smoke136link --title "Link 136" --set "url=https://a" --set "category=$SMOKE136_CAT" | Out-Null
pass "cross-process FK freshness on ddb create (#16)"

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

$output = ddb discover recent --days 30
if ($output -notmatch $ID1) { throw "discover recent should include $ID1" }
pass "discover recent"

$output = ddb discover link-density
if ($output -notmatch "density:") { throw "discover link-density output missing density:" }
pass "discover link-density"

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

# --log-level flag accepted
ddb --log-level debug status *>$null
pass "--log-level flag accepted"

# app-building end-to-end flow
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

# 17. cascade delete
ddb query "CREATE TABLE cdcategory (label VARCHAR(100))" | Out-Null
ddb query "CREATE TABLE cdbookmark (url TEXT, cdcategory TEXT REFERENCES cdcategory)" | Out-Null
$CAT_ID = ddb query "INSERT INTO cdcategory (label) VALUES ('work')"
Start-Sleep -Seconds 1
$BM_ID = ddb query "INSERT INTO cdbookmark (url) VALUES ('https://example.com')"
ddb query "INSERT INTO cdbookmark_cdcategory (cdbookmark_id, cdcategory_id) VALUES ('$BM_ID', '$CAT_ID')" | Out-Null
# Verify junction row exists
$output = ddb query "SELECT COUNT(*) FROM cdbookmark_cdcategory WHERE cdcategory_id = '$CAT_ID'"
if ($output -notmatch "1") { throw "junction row not created" }
# Delete the category — should cascade
ddb query "DELETE FROM cdcategory WHERE id = '$CAT_ID'" | Out-Null
# Junction row should be gone
$output = ddb query "SELECT COUNT(*) FROM cdbookmark_cdcategory WHERE cdcategory_id = '$CAT_ID'"
if ($output -notmatch "0") { throw "junction row not cascaded: $output" }
# Wikilink to deleted category should be removed from bookmark
$output = ddb read $BM_ID
if ($output -match "\[\[$CAT_ID\]\]") { throw "wikilink to deleted category still present in bookmark" }
# Clean up
ddb query "DROP TABLE cdbookmark CASCADE" | Out-Null
ddb query "DROP TABLE cdcategory CASCADE" | Out-Null
pass "cascade delete"

# 17b. cascade delete (parent direction, PRD 00137)
# Mirrors §17 but deletes the *owner* of the auto-junction row instead of
# the referenced target. Pre-fix the junction row stayed dangling; post-fix
# the same DELETE removes it in the same transaction.
ddb query "CREATE TABLE pdc_cat (label VARCHAR(100))" | Out-Null
ddb query "CREATE TABLE pdc_bm (url TEXT, pdc_cat TEXT REFERENCES pdc_cat)" | Out-Null
$PDC_CAT_ID = ddb query "INSERT INTO pdc_cat (label) VALUES ('alpha')"
Start-Sleep -Seconds 1
$PDC_BM_ID = ddb query "INSERT INTO pdc_bm (url) VALUES ('https://pdc.example.com')"
ddb query "INSERT INTO pdc_bm_pdc_cat (pdc_bm_id, pdc_cat_id) VALUES ('$PDC_BM_ID', '$PDC_CAT_ID')" | Out-Null
# Sanity: junction holds the owner-side row.
$output = ddb query "SELECT COUNT(*) FROM pdc_bm_pdc_cat WHERE pdc_bm_id = '$PDC_BM_ID'"
if ($output -notmatch "1") { throw "PRD 00137: junction row not created: $output" }
# Delete the bookmark (parent direction).
ddb query "DELETE FROM pdc_bm WHERE id = '$PDC_BM_ID'" | Out-Null
# Owner-side junction row must be gone.
$output = ddb query "SELECT COUNT(*) FROM pdc_bm_pdc_cat WHERE pdc_bm_id = '$PDC_BM_ID'"
if ($output -notmatch "0") { throw "PRD 00137: owner-side junction not cascaded: $output" }
# Whole junction empty (only row was the deleted parent's).
$output = ddb query "SELECT COUNT(*) FROM pdc_bm_pdc_cat"
if ($output -notmatch "0") { throw "PRD 00137: junction table not empty after parent delete: $output" }
ddb query "DROP TABLE pdc_bm CASCADE" | Out-Null
ddb query "DROP TABLE pdc_cat CASCADE" | Out-Null
pass "PRD 00137: parent-delete clears owned auto-junction rows"

# 18. cascade delete via ddb delete (service path)
ddb query "CREATE TABLE cdcat2 (label VARCHAR(100))" | Out-Null
ddb query "CREATE TABLE cdbm2 (url TEXT, cdcat2 TEXT REFERENCES cdcat2)" | Out-Null
$CAT2_ID = ddb query "INSERT INTO cdcat2 (label) VALUES ('svc')"
Start-Sleep -Seconds 1
$BM2_ID = ddb query "INSERT INTO cdbm2 (url) VALUES ('https://svc.example.com')"
ddb query "INSERT INTO cdbm2_cdcat2 (cdbm2_id, cdcat2_id) VALUES ('$BM2_ID', '$CAT2_ID')" | Out-Null
# Delete via ddb delete (service path)
ddb delete $CAT2_ID 2>$null
# Junction row should be gone
$output = ddb query "SELECT COUNT(*) FROM cdbm2_cdcat2 WHERE cdcat2_id = '$CAT2_ID'"
if ($output -notmatch "0") { throw "junction row not cascaded (service path): $output" }
# Wikilink removed from bookmark
$output = ddb read $BM2_ID
if ($output -match "\[\[$CAT2_ID\]\]") { throw "wikilink to deleted category still present (service path)" }
ddb query "DROP TABLE cdbm2 CASCADE" | Out-Null
ddb query "DROP TABLE cdcat2 CASCADE" | Out-Null
pass "cascade delete (service path)"

# 18b. RESTRICT on NOT NULL REFERENCES (#10)
ddb query "CREATE TABLE r10link (url VARCHAR(255) NOT NULL)" | Out-Null
ddb query "CREATE TABLE r10cat (name VARCHAR(255) NOT NULL)" | Out-Null
ddb query 'CREATE TABLE "r10-mem" (link_id VARCHAR(255) NOT NULL REFERENCES r10link(id), cat_id VARCHAR(255) NOT NULL REFERENCES r10cat(id), UNIQUE(link_id, cat_id))' | Out-Null
$R10_L = ddb query "INSERT INTO r10link (title, url) VALUES ('L', 'https://r10.example')"
Start-Sleep -Seconds 1
$R10_C = ddb query "INSERT INTO r10cat (title, name) VALUES ('C', 'c')"
ddb query "INSERT INTO `"r10-mem`" (title, link_id, cat_id) VALUES ('M', '$R10_L', '$R10_C')" | Out-Null
# SQL DELETE of parent must fail with the RESTRICT message
$R10_ERR = & $DDB query "DELETE FROM r10link WHERE id = '$R10_L'" 2>&1
if ($LASTEXITCODE -eq 0) { throw "DELETE of parent with NOT NULL REFERENCES child should have failed" }
$R10_ERR_TEXT = [string]::Join("`n", (@($R10_ERR) | ForEach-Object { "$_" }))
if ($R10_ERR_TEXT -notmatch "NOT NULL REFERENCES") { throw "expected RESTRICT error, got: $R10_ERR_TEXT" }
if ($R10_ERR_TEXT -notmatch "r10-mem") { throw "expected blocking table name in error, got: $R10_ERR_TEXT" }
# Parent still present
$output = ddb query "SELECT COUNT(*) FROM r10link WHERE id = '$R10_L'"
if ($output -notmatch "1") { throw "parent deleted despite RESTRICT: $output" }
# `ddb delete` of parent must also fail
if (-not (ddb-fails delete $R10_L)) { throw "ddb delete of parent with NOT NULL REFERENCES child should fail" }
# After removing the child, parent delete succeeds
ddb query "DELETE FROM `"r10-mem`" WHERE link_id = '$R10_L'" | Out-Null
ddb query "DELETE FROM r10link WHERE id = '$R10_L'" | Out-Null
ddb query "DROP TABLE `"r10-mem`" CASCADE" | Out-Null
ddb query "DROP TABLE r10link CASCADE" | Out-Null
ddb query "DROP TABLE r10cat CASCADE" | Out-Null
pass "issue-10: RESTRICT blocks delete with NOT NULL REFERENCES child"

# 19. boolean consistency in SQL responses
ddb query "CREATE TABLE booltest (label TEXT, active BOOLEAN)" | Out-Null
ddb query "INSERT INTO booltest (label, active) VALUES ('on', true)" | Out-Null
Start-Sleep -Seconds 1
ddb query "INSERT INTO booltest (label, active) VALUES ('off', false)" | Out-Null
# Boolean true should be "true", not "1"
$output = ddb query "SELECT active FROM booltest WHERE active = 1"
if ($output -notmatch "true") { throw "boolean true not coerced: $output" }
# Boolean false should be "false", not "0"
$output = ddb query "SELECT active FROM booltest WHERE active = 0"
if ($output -notmatch "false") { throw "boolean false not coerced: $output" }
# NULL boolean stays NULL
Start-Sleep -Seconds 1
ddb query "INSERT INTO booltest (label) VALUES ('none')" | Out-Null
$output = ddb query "SELECT active FROM booltest WHERE label = 'none'"
if ($output -notmatch "NULL") { throw "boolean null not preserved: $output" }
# Mixed columns: only booleans coerced
$output = ddb query "SELECT label, active FROM booltest WHERE label = 'on'"
if ($output -notmatch "on") { throw "non-boolean column corrupted: $output" }
if ($output -notmatch "true") { throw "boolean column not coerced in mix: $output" }
ddb query "DROP TABLE booltest CASCADE" | Out-Null
pass "boolean consistency (SQL responses)"

# 20. type tables are self-contained (no JOIN needed for core columns)
$scId = ddb query "INSERT INTO foo (title, bar, baz) VALUES ('Self-Contained Test', 'val', 1)"
$output = ddb query "SELECT id, title, date, updated_at, bar FROM foo WHERE id = '$scId'"
if ($output -notmatch $scId) { throw "self-contained: id missing: $output" }
if ($output -notmatch "Self-Contained Test") { throw "self-contained: title missing: $output" }
pass "type table self-contained (core columns without JOIN)"
ddb query "DROP TABLE foo CASCADE" | Out-Null

# 21. DEFAULT NEXT auto-increment
ddb query "CREATE TABLE nexttbl (name TEXT, pos INTEGER DEFAULT NEXT)" | Out-Null
ddb query "INSERT INTO nexttbl (name) VALUES ('a')" | Out-Null
Start-Sleep -Seconds 1
ddb query "INSERT INTO nexttbl (name) VALUES ('b')" | Out-Null
Start-Sleep -Seconds 1
ddb query "INSERT INTO nexttbl (name) VALUES ('c')" | Out-Null
# Verify auto-assigned values 1, 2, 3
$output = ddb query "SELECT pos FROM nexttbl ORDER BY pos"
if ($output -notmatch "1") { throw "NEXT default: expected 1: $output" }
if ($output -notmatch "2") { throw "NEXT default: expected 2: $output" }
if ($output -notmatch "3") { throw "NEXT default: expected 3: $output" }
# Explicit override
Start-Sleep -Seconds 1
ddb query "INSERT INTO nexttbl (name, pos) VALUES ('d', 99)" | Out-Null
Start-Sleep -Seconds 1
ddb query "INSERT INTO nexttbl (name) VALUES ('e')" | Out-Null
$output = ddb query "SELECT MAX(pos) FROM nexttbl"
if ($output -notmatch "100") { throw "NEXT default: expected MAX 100: $output" }
ddb query "DROP TABLE nexttbl CASCADE" | Out-Null
pass "DEFAULT NEXT auto-increment"

# 22. FTS5 search boost (search_boost on typedef columns)
ddb type install contact 2>$null | Out-Null
$BOOST_ID = ddb create --type contact --title "Boost Test Contact" --set email=uniquexyz@example.com
ddb reindex | Out-Null
$output = ddb search "uniquexyz"
if ($output -notmatch $BOOST_ID) { throw "fts5 search boost: boosted field not found in results" }
pass "fts5 search boost (boosted field match)"

# 23. SQL expression support (COALESCE, IFNULL, arithmetic in VALUES/SET)
$output = ddb query "CREATE TABLE exprtbl (sort_order INTEGER)"
if ($output -notmatch "table exprtbl created") { throw "create exprtbl failed" }
$EXPR_ID1 = ddb query "INSERT INTO exprtbl (sort_order) VALUES (COALESCE((SELECT MAX(sort_order) FROM exprtbl), 0))"
if ($EXPR_ID1 -notmatch "^\d{14}$") { throw "insert coalesce bad id: $EXPR_ID1" }
$output = ddb query "SELECT sort_order FROM exprtbl WHERE id = '$EXPR_ID1'"
if ($output -notmatch "0") { throw "coalesce should yield 0: $output" }
Start-Sleep -Seconds 1
$EXPR_ID2 = ddb query "INSERT INTO exprtbl (sort_order) VALUES (COALESCE((SELECT MAX(sort_order) FROM exprtbl), 0) + 1)"
if ($EXPR_ID2 -notmatch "^\d{14}$") { throw "insert coalesce+1 bad id: $EXPR_ID2" }
$output = ddb query "SELECT sort_order FROM exprtbl WHERE id = '$EXPR_ID2'"
if ($output -notmatch "1") { throw "coalesce+1 should yield 1: $output" }
$output = ddb query "UPDATE exprtbl SET sort_order = IFNULL(NULL, 42) WHERE id = '$EXPR_ID1'"
if ($output -notmatch "1 row") { throw "update ifnull failed: $output" }
$output = ddb query "SELECT sort_order FROM exprtbl WHERE id = '$EXPR_ID1'"
if ($output -notmatch "42") { throw "ifnull should yield 42: $output" }
# NULLIF returns null (empty in ddb) for equal args
ddb query "INSERT INTO exprtbl (sort_order) VALUES (NULLIF(0, 0))" | Out-Null
# Nested expression: ABS(-1) + LENGTH('hi') = 1 + 2 = 3
$NESTED_ID = ddb query "INSERT INTO exprtbl (sort_order) VALUES (COALESCE(ABS(-1) + LENGTH('hi'), 0))"
$output = ddb query "SELECT sort_order FROM exprtbl WHERE id = '$NESTED_ID'"
if ($output -notmatch "3") { throw "nested expression should yield 3: $output" }
# Unlisted function should be rejected
if (-not (ddb-fails query "INSERT INTO exprtbl (sort_order) VALUES (BADFUNC(1))")) { throw "unlisted function should be rejected" }
ddb query "DROP TABLE exprtbl CASCADE" | Out-Null
pass "sql expression support (COALESCE, IFNULL, arithmetic)"

# 24. --set / --unset flags on create and update
$SET_ID = ddb create --title "SetTest" --set color=red --set size=large
if ($SET_ID -notmatch "^\d{14}$") { throw "--set create bad id: $SET_ID" }
$output = ddb read $SET_ID
if ($output -notmatch "color: red") { throw "--set color not found" }
if ($output -notmatch "size: large") { throw "--set size not found" }
ddb update $SET_ID --set color=blue
$output = ddb read $SET_ID
if ($output -notmatch "color: blue") { throw "--set update color failed" }
ddb update $SET_ID --set "color="
$output = ddb read $SET_ID
if ($output -match "color: blue") { throw "--set k= should clear value" }
ddb update $SET_ID --unset size
$output = ddb read $SET_ID
if ($output -match "size:") { throw "--unset should remove field" }
if (-not (ddb-fails create --title "Bad" --set "noequals")) { throw "malformed --set should error" }
pass "--set / --unset flags"

# 25. SQL INSERT defaults date from doogat ID
ddb query "CREATE TABLE datetest (name TEXT)"
$DATE_ID = ddb query "INSERT INTO datetest (name) VALUES ('hello')"
$DATE_EXPECTED = "$($DATE_ID.Substring(0,4))-$($DATE_ID.Substring(4,2))-$($DATE_ID.Substring(6,2))"
$output = ddb read $DATE_ID
if ($output -notmatch "date: $DATE_EXPECTED") { throw "date not derived from ID: $output" }
$EXPLICIT_ID = ddb query "INSERT INTO datetest (name, date) VALUES ('world', '2025-01-15')"
$output = ddb read $EXPLICIT_ID
if ($output -notmatch "date: 2025-01-15") { throw "explicit date not preserved: $output" }
pass "SQL INSERT defaults date from ID"

# 26. FTS negation search
$NEG_ID1 = ddb create --title "Important Design" --body "important design review"
$NEG_ID2 = ddb create --title "Important Meeting" --body "important meeting notes"
$NEG_ID3 = ddb create --title "Daily Standup" --body "daily standup agenda"
ddb reindex | Out-Null
$NEG_RESULT = ddb search "important NOT meeting"
if ($NEG_RESULT -notmatch $NEG_ID1) { throw "fts negation: positive term not found" }
if ($NEG_RESULT -match $NEG_ID2) { throw "fts negation: negated term should be excluded" }
pass "fts negation (positive NOT negative)"
$NEG_ALL = ddb search "NOT standup"
if ($NEG_ALL -match $NEG_ID3) { throw "fts negation: all-negative should exclude matching" }
if ($NEG_ALL -notmatch $NEG_ID1) { throw "fts negation: all-negative should include non-matching" }
pass "fts negation (all-negative query)"

# 27. ON CONFLICT DO NOTHING (upsert)
ddb query "CREATE TABLE upsert_test (code TEXT, label TEXT)" | Out-Null
# Patch typedef to add unique_together on [code]
$utFile = Get-ChildItem -Path ddb/_typedef -Filter *.md | Where-Object {
    (Get-Content $_.FullName -Raw) -match "title: upsert_test"
} | Select-Object -First 1
$utContent = Get-Content $utFile.FullName -Raw
$utContent = $utContent -replace "type: _typedef", "type: _typedef`nunique_together:`n  - - code"
Set-Content -Path $utFile.FullName -Value $utContent -NoNewline
git add -A | Out-Null
git commit -m "add unique_together" --quiet | Out-Null
ddb reindex | Out-Null
$UPSERT_ID1 = ddb query "INSERT INTO upsert_test (title, code, label) VALUES ('First', 'ABC', 'original')"
if ($UPSERT_ID1 -notmatch "^\d{14}$") { throw "upsert: bad id: $UPSERT_ID1" }
Start-Sleep -Seconds 1
# Duplicate with ON CONFLICT DO NOTHING should return existing ID
$UPSERT_ID2 = ddb query "INSERT INTO upsert_test (title, code, label) VALUES ('Second', 'ABC', 'duplicate') ON CONFLICT DO NOTHING"
if ($UPSERT_ID2 -ne $UPSERT_ID1) { throw "upsert: duplicate should return existing ID: got $UPSERT_ID2 expected $UPSERT_ID1" }
Start-Sleep -Seconds 1
# Non-duplicate should create new
$UPSERT_ID3 = ddb query "INSERT INTO upsert_test (title, code, label) VALUES ('Third', 'DEF', 'new') ON CONFLICT DO NOTHING"
if ($UPSERT_ID3 -eq $UPSERT_ID1) { throw "upsert: non-duplicate should create new ID" }
ddb query "DROP TABLE upsert_test CASCADE" | Out-Null
pass "ON CONFLICT DO NOTHING (upsert)"

# 28. in-query field filter substring + REFERENCES title resolution (PRD 00133)
ddb query "CREATE TABLE smoke_cat (label VARCHAR(100))" | Out-Null
ddb query "CREATE TABLE smoke_link (url TEXT, smoke_cat VARCHAR(14) REFERENCES smoke_cat(id))" | Out-Null
$scDevId = ddb query "INSERT INTO smoke_cat (title, label) VALUES ('Development', 'dev')"
ddb query "INSERT INTO smoke_link (title, url, smoke_cat) VALUES ('Rust Async', 'https://example.com/rust-async', '$scDevId')" | Out-Null
ddb query "INSERT INTO smoke_link (title, url) VALUES ('Meeting Notes Archive', 'https://example.com/archive')" | Out-Null
ddb reindex | Out-Null

$titleLikeResult = ddb search "title=Archive" | Out-String
if ($titleLikeResult -notmatch "Meeting Notes Archive") { throw "in-query title=X did not substring-match: $titleLikeResult" }
pass "in-query title=X does substring match"

$catResolveResult = ddb search "smoke_cat=Development" | Out-String
if ($catResolveResult -notmatch "Rust Async") { throw "in-query <ref_col>=X did not resolve via referenced title: $catResolveResult" }
pass "in-query <ref_col>=X resolves via referenced typedef title"

ddb query "DROP TABLE smoke_link CASCADE" | Out-Null
ddb query "DROP TABLE smoke_cat CASCADE" | Out-Null

# 29. ALTER TABLE ALTER COLUMN TYPE (PRD 00128)
ddb query "CREATE TABLE alter_type_smoke (url VARCHAR(10))" | Out-Null
$smokeAtId = ddb query "INSERT INTO alter_type_smoke (title, url) VALUES ('start', '1234567890')"
if ($smokeAtId -notmatch "^\d{14}$") { throw "alter-col-type: bad id: $smokeAtId" }
ddb query "ALTER TABLE alter_type_smoke ALTER COLUMN url TYPE TEXT" | Out-Null
Start-Sleep -Seconds 1
$longUrl = 'x' * 500
$smokeAtId2 = ddb query "INSERT INTO alter_type_smoke (title, url) VALUES ('post', '$longUrl')"
if ($smokeAtId2 -notmatch "^\d{14}$") { throw "alter-col-type: post-widen insert failed: $smokeAtId2" }
$rows = ddb query "SELECT id FROM alter_type_smoke"
if ($rows -notmatch $smokeAtId2) { throw "alter-col-type: widened row not visible" }
pass "ALTER TABLE ALTER COLUMN TYPE widening (VARCHAR -> TEXT)"

Cleanup
Write-Host "=== all smoke tests passed ==="
