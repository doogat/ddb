# Search: one filter contract and one hit shape across CLI, GraphQL, and REST

*2026-10-17T23:51:38Z by Showboat 0.6.1*
<!-- showboat-id: 8a437c7e-75fe-4e79-8b08-126b7b2d3a95 -->

PRD 00176 routes search through one app-contract command, SearchCommand, with one pagination policy, PageRequest (default 50, maximum 10,000, out-of-range values rejected, never clamped). CLI, GraphQL, REST, and FFI now take the same type and tag filters and return the same ten-field hit. Before, REST dropped type/tag on search and returned four fields, and the CLI could not filter at all. This demo seeds two typed doogats and one untyped doogat in a throwaway temp dir, then runs the same filtered searches on the CLI, GraphQL, and REST. Logs are off. Ids, paths, and timestamps are replaced with placeholders so the output is stable. FFI coverage is in ddb-core/src/ffi/tests.rs (no host binding toolchain runs here).

```bash
set -eu; export RUST_LOG=off; DDB=/Users/bob/git/src/github.com/doogat/ddb/target/debug/ddb
W=$(mktemp -d /tmp/ddb-walk-00176.XXXXXX); echo "$W" > /tmp/ddb-walk-00176.path; cd "$W"
$DDB init repo >/dev/null
git -C repo config commit.gpgsign false
$DDB --repo repo query 'CREATE TABLE ws_recipe (cuisine VARCHAR(100))'
$DDB --repo repo query 'CREATE TABLE ws_dessert (cuisine VARCHAR(100))'
$DDB --repo repo create --title 'Thai soup' --type ws_recipe --tags dinner --set cuisine=thai --body 'walkterm soup' >/dev/null
$DDB --repo repo create --title 'Lime cake' --type ws_dessert --tags dinner,sweet --set cuisine=thai --body 'walkterm cake' >/dev/null
$DDB --repo repo create --title 'Plain note' --tags sweet --body 'walkterm note' >/dev/null
echo 'seeded 3 doogats'
```

```output
table ws_recipe created
table ws_dessert created
seeded 3 doogats
```

CLI. ddb search gains --type and --tag. Each hit now prints all ten fields: id, title, path, snippet, type, rank, tags, created, updated, and the typed fields (cuisine). The header shows the page window. With no --limit or --offset, the window comes from PageRequest's 50/0 defaults (the old clap default was 20). A filter that matches nothing prints 'no results', not the unfiltered set.

```bash
set -u; export RUST_LOG=off; DDB=/Users/bob/git/src/github.com/doogat/ddb/target/debug/ddb; cd "$(< /tmp/ddb-walk-00176.path)"
scrub() { sed -E -e 's/rank: [-0-9.e]+/rank: <rank>/' -e 's/[0-9]{14}/<id>/g' -e 's/[0-9]{4}-[0-9]{2}-[0-9]{2}(T[0-9:.+]+)?/<date>/g'; }
echo '== --type ws_recipe'; $DDB --repo repo search walkterm --type ws_recipe | scrub
echo '== --tag sweet'; $DDB --repo repo search walkterm --tag sweet | scrub
echo '== --type ws_dessert --tag dinner'; $DDB --repo repo search walkterm --type ws_dessert --tag dinner | scrub
echo '== --type ws_recipe --tag sweet (no match)'; $DDB --repo repo search walkterm --type ws_recipe --tag sweet | scrub
```

```output
== --type ws_recipe
Showing 1-1 of 1 results
[<id>] Thai soup (ddb/<id>.md)
  <b>walkterm</b> soup
  type: ws_recipe  rank: <rank>  tags: dinner
  created: <date>  updated: <date>
  cuisine: thai
== --tag sweet
Showing 1-2 of 2 results
[<id>] Plain note (ddb/<id>.md)
  <b>walkterm</b> note
  type: -  rank: <rank>  tags: sweet
  created: <date>  updated: <date>
[<id>] Lime cake (ddb/<id>.md)
  <b>walkterm</b> cake
  type: ws_dessert  rank: <rank>  tags: dinner, sweet
  created: <date>  updated: <date>
  cuisine: thai
== --type ws_dessert --tag dinner
Showing 1-1 of 1 results
[<id>] Lime cake (ddb/<id>.md)
  <b>walkterm</b> cake
  type: ws_dessert  rank: <rank>  tags: dinner, sweet
  created: <date>  updated: <date>
  cuisine: thai
== --type ws_recipe --tag sweet (no match)
no results
```

The CLI pages through the same PageRequest. An explicit window works as before. A limit of 0 or above 10,000 is a validation error, not a clamp.

```bash
set -u; export RUST_LOG=off; DDB=/Users/bob/git/src/github.com/doogat/ddb/target/debug/ddb; cd "$(< /tmp/ddb-walk-00176.path)"
$DDB --repo repo search walkterm --limit 1 --offset 1 | head -n 1
$DDB --repo repo search walkterm --limit 10001 2>&1; echo "exit=$?"
$DDB --repo repo search walkterm --limit 0 2>&1; echo "exit=$?"
```

```output
Showing 2-2 of 3 results
error: validation: limit must be between 1 and 10000, got 10001
exit=1
error: validation: limit must be between 1 and 10000, got 0
exit=1
```

Start ddb serve on the same repo. HOME points at the temp dir, so the auth token file lands there and not in the real ~/.config/ddb.

```bash
set -u; export RUST_LOG=off; DDB=/Users/bob/git/src/github.com/doogat/ddb/target/debug/ddb; W=$(< /tmp/ddb-walk-00176.path); cd "$W"
HOME="$W" $DDB --repo repo --log-dir "$W/logs" serve --port 19761 --pg-port 19762 > serve.out 2> serve.log &
echo $! > serve.pid
for i in $(seq 1 100); do grep -q 'pgwire listening on' serve.log 2>/dev/null && break; sleep 0.1; done
test -s "$W/.config/ddb/token" && echo 'server up, token written under the temp HOME'
```

```output
server up, token written under the temp HOME
```

GraphQL and REST, same query and filter (type ws_dessert, tag dinner). GraphQL without a where argument now builds a SearchCommand. REST GET /rest/doogats?q= used to drop type and tag and return four fields. It now applies both filters and returns the same ten fields as GraphQL. REST names the type field 'type', as GraphQL does, and leaves out 'fields' when a hit has none.

```bash
set -u; W=$(< /tmp/ddb-walk-00176.path); T=$(< "$W/.config/ddb/token")
scrub() { sed -E -e 's/"rank": [-+0-9.eE]+/"rank": "<rank>"/' -e 's/[0-9]{14}/<id>/g' -e 's/[0-9]{4}-[0-9]{2}-[0-9]{2}(T[0-9:.+]+)?/<date>/g'; }
echo '== GraphQL'
curl -s http://127.0.0.1:19761/graphql -H "Authorization: Bearer $T" -H 'Content-Type: application/json' \
  -d '{"query":"{ search(query: \"walkterm\", types: [\"ws_dessert\"], tag: \"dinner\") { totalCount hits { id title path snippet rank updated_at tags type fields created_at } } }"}' \
  | jq '.data.search' | scrub
echo '== REST'
curl -s -H "Authorization: Bearer $T" 'http://127.0.0.1:19761/rest/doogats?q=walkterm&type=ws_dessert&tag=dinner' | jq . | scrub
```

```output
== GraphQL
{
  "totalCount": 1,
  "hits": [
    {
      "id": "<id>",
      "title": "Lime cake",
      "path": "ddb/<id>.md",
      "snippet": "<b>walkterm</b> cake",
      "rank": "<rank>",
      "updated_at": "<date>",
      "tags": [
        "dinner",
        "sweet"
      ],
      "type": "ws_dessert",
      "fields": {
        "cuisine": "thai"
      },
      "created_at": "<date>"
    }
  ]
}
== REST
{
  "data": [
    {
      "created_at": "<date>",
      "fields": {
        "cuisine": "thai"
      },
      "id": "<id>",
      "path": "ddb/<id>.md",
      "rank": "<rank>",
      "snippet": "<b>walkterm</b> cake",
      "tags": [
        "dinner",
        "sweet"
      ],
      "title": "Lime cake",
      "type": "ws_dessert",
      "updated_at": "<date>"
    }
  ],
  "total_count": 1
}
```

The other REST filters narrow the same way the CLI did above. Each line prints total_count and the hit titles. A filter that matches nothing returns an empty list, not the unfiltered set. The untyped 'Plain note' hit has type null and no fields key.

```bash
set -u; W=$(< /tmp/ddb-walk-00176.path); T=$(< "$W/.config/ddb/token")
rest() { curl -s -H "Authorization: Bearer $T" "http://127.0.0.1:19761/rest/doogats?q=walkterm$1"; }
for f in '' '&type=ws_recipe' '&tag=sweet' '&type=ws_recipe&tag=sweet'; do
  printf '%-28s ' "q=walkterm$f"; rest "$f" | jq -c '[.total_count, [.data[].title]]'
done
echo '== untyped hit keys'
rest '&tag=sweet' | jq -c '.data[] | select(.title == "Plain note") | {type, has_fields: has("fields"), keys: keys}'
```

```output
q=walkterm                   [3,["Plain note","Thai soup","Lime cake"]]
q=walkterm&type=ws_recipe    [1,["Thai soup"]]
q=walkterm&tag=sweet         [2,["Plain note","Lime cake"]]
q=walkterm&type=ws_recipe&tag=sweet [0,[]]
== untyped hit keys
{"type":null,"has_fields":false,"keys":["created_at","id","path","rank","snippet","tags","title","type","updated_at"]}
```

REST search paging now goes through PageRequest. Before 00176, per_page was clamped silently to 1-200 and page below 1 was read as 1. Now per_page 500 and 10,000 are honored (HTTP 200). per_page 10,001, per_page 0, and page 0 return HTTP 400 VALIDATION_ERROR. The plain list endpoint (no q) keeps its old 1-200 clamp, shown last.

```bash
set -u; W=$(< /tmp/ddb-walk-00176.path); T=$(< "$W/.config/ddb/token")
for p in 'per_page=500' 'per_page=10000' 'per_page=10001' 'per_page=0' 'page=0' 'page=2&per_page=2'; do
  printf '%-20s ' "$p"
  curl -s -o body.json -w '%{http_code} ' -H "Authorization: Bearer $T" "http://127.0.0.1:19761/rest/doogats?q=walkterm&$p"
  jq -c 'if .error then {error, message} else {total_count, hits: (.data | length)} end' body.json
done
printf '%-20s ' 'list per_page=10001'
curl -s -o body.json -w '%{http_code} ' -H "Authorization: Bearer $T" 'http://127.0.0.1:19761/rest/doogats?per_page=10001'
jq -c '.pagination' body.json
```

```output
per_page=500         200 {"total_count":3,"hits":3}
per_page=10000       200 {"total_count":3,"hits":3}
per_page=10001       400 {"error":"VALIDATION_ERROR","message":"limit must be between 1 and 10000, got 10001"}
per_page=0           400 {"error":"VALIDATION_ERROR","message":"limit must be between 1 and 10000, got 0"}
page=0               400 {"error":"VALIDATION_ERROR","message":"page must be >= 1, got 0"}
page=2&per_page=2    200 {"total_count":3,"hits":1}
list per_page=10001  200 {"page":1,"per_page":200,"total":5,"total_pages":1}
```

Stop the server and remove the temp dir. Tests: tests/e2e/integration_rest_search_filtered.rs (REST filters, hit shape, 400 on out-of-range paging), tests/e2e/smoke_search.rs (CLI --type/--tag and the 50/0 defaults), the filtered_search conformance fixture (CLI and GraphQL parity), and ddb-core/src/ffi/tests.rs (FFI search_filtered and search_paginated). Contract: docs/src/technical/app-contract.md. To re-run this file, use dev/bin/safe-showboat-verify, never showboat verify from the project root.

```bash
set -u; W=$(< /tmp/ddb-walk-00176.path)
kill "$(< "$W/serve.pid")" && echo 'server stopped'
rm -rf "$W" /tmp/ddb-walk-00176.path && echo 'cleaned up'
```

```output
server stopped
cleaned up
```
