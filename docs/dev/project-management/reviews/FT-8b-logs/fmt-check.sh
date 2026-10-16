#!/usr/bin/env bash
# rustfmt --check via stdin (no child-module resolution) for base and candidate copies.
BASE=/Users/bob/git/src/github.com/doogat/ddb
CAND=$BASE/dev/local/tmp/worktrees/00204-ft-8b
for f in ddb-cli/src/commands/sync.rs ddb-server/src/schema/mod.rs \
  ddb-server/src/schema/mutations/mod.rs ddb-server/src/schema/mutations/operations.rs \
  ddb-server/src/schema/mutations/types.rs tests/e2e/collision.rs; do
  git -C "$BASE" show "d7867d2:$f" | rustfmt +1.89.0 --edition 2021 --check >/dev/null 2>&1; b=$?
  rustfmt +1.89.0 --edition 2021 --check < "$CAND/$f" > "/tmp/fmt-$(basename "$f").diff" 2>&1; c=$?
  echo "$f base=$b cand=$c"
done
