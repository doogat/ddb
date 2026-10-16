#!/usr/bin/env bash
set -euo pipefail
ddb=/Users/bob/git/src/github.com/doogat/ddb/target/debug/ddb
probe_repo=$(mktemp -d /tmp/ddb-review-probes.XXXXXX)
trap 'rm -rf "$probe_repo"' EXIT
cd "$probe_repo"
"$ddb" init >/dev/null
query() { "$ddb" query "$1"; }
query 'CREATE TABLE parent (name TEXT)' >/dev/null
query 'CREATE TABLE child (name TEXT, parent VARCHAR(255) REFERENCES parent(id) ON DELETE CASCADE)' >/dev/null
query 'CREATE TABLE survivor (name TEXT, parent VARCHAR(255) REFERENCES parent(id), child VARCHAR(255) REFERENCES child(id))' >/dev/null
parent=$(query "INSERT INTO parent (name) VALUES ('parent')")
child=$(query "INSERT INTO child (name, parent) VALUES ('child', '$parent')")
survivor=$(query "INSERT INTO survivor (name, parent, child) VALUES ('original', '$parent', '$child')")
before=$(git rev-parse HEAD)
query "BEGIN; UPDATE survivor SET name = 'buffered' WHERE id = '$survivor'; DELETE FROM parent WHERE id = '$parent'; COMMIT" >/dev/null
test "$(query 'SELECT name FROM survivor')" = buffered
test "$(query 'SELECT COUNT(*) FROM child')" = 0
test "$(query 'SELECT COUNT(*) FROM survivor_parent')" = 0
test "$(query 'SELECT COUNT(*) FROM survivor_child')" = 0
test "$(git rev-parse HEAD^)" = "$before"
content=$("$ddb" read "$survivor")
[[ "$content" != *"$parent"* && "$content" != *"$child"* ]]
printf '%s\n' 'PASS: buffered survivor edit composes with cascade in one commit'

query 'CREATE TABLE node (name TEXT, first VARCHAR(255) REFERENCES node(id) ON DELETE CASCADE, second VARCHAR(255) REFERENCES node(id) ON DELETE CASCADE)' >/dev/null
for lane in service sql; do
    root=$(query "INSERT INTO node (name) VALUES ('root')")
    a=$(query "INSERT INTO node (name, first) VALUES ('a', '$root')")
    b=$(query "INSERT INTO node (name, first) VALUES ('b', '$root')")
    leaf=$(query "INSERT INTO node (name, first, second) VALUES ('leaf', '$a', '$b')")
    before=$(git rev-parse HEAD)
    if [[ "$lane" = sql ]]; then
        test "$(query "DELETE FROM node WHERE id = '$root'")" = '1 row(s) affected'
    else
        "$ddb" delete "$root" >/dev/null
    fi
    test "$(query 'SELECT COUNT(*) FROM node')" = 0
    test "$(query 'SELECT COUNT(*) FROM node_first')" = 0
    test "$(query 'SELECT COUNT(*) FROM node_second')" = 0
    test "$(git rev-parse HEAD^)" = "$before"
    printf 'PASS: %s deletes a four-node diamond once in one commit\n' "$lane"
done

root=$(query "INSERT INTO node (name) VALUES ('root')")
a=$(query "INSERT INTO node (name, first) VALUES ('a', '$root')")
b=$(query "INSERT INTO node (name, first, second) VALUES ('b', '$root', '$a')")
query "UPDATE node SET second = '$b' WHERE id = '$a'" >/dev/null
before=$(git rev-parse HEAD)
for lane in service sql; do
    if [[ "$lane" = sql ]]; then
        if result=$(query "DELETE FROM node WHERE id = '$root'" 2>&1); then exit 1; fi
    else
        if result=$("$ddb" delete "$root" 2>&1); then exit 1; fi
    fi
    [[ "$result" = *'cascade delete would form a cycle'* ]]
    test "$(git rev-parse HEAD)" = "$before"
    test "$(query 'SELECT COUNT(*) FROM node')" = 3
    printf 'PASS: %s rejects cross-sibling cycle without changing Git or rows\n' "$lane"
done
