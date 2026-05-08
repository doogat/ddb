#!/usr/bin/env bash
# showboat-verify-no-contamination-test — regression test for PRD 00135.
#
# Runs `dev/bin/safe-showboat-verify` against a known walkthrough and asserts
# that the active checkout's master branch and working tree are unchanged
# afterward. This catches regressions in the wrapper or in showboat itself.
#
# This is a project-local test (depends on dev/local/walkthroughs/, gitignored)
# and does NOT run in CI. Run it manually after upgrading showboat or editing
# the wrapper.

set -euo pipefail

REPO_ROOT="$(git rev-parse --show-toplevel)"
if [[ -z "$REPO_ROOT" ]]; then
  echo "error: not inside a git repository" >&2
  exit 2
fi

WRAPPER="${REPO_ROOT}/dev/bin/safe-showboat-verify"
if [[ ! -x "$WRAPPER" ]]; then
  echo "error: wrapper not found or not executable: $WRAPPER" >&2
  exit 2
fi

# Pick a walkthrough fixture. The test must run on a walkthrough that exhibits
# the contamination pattern — bash blocks that auto-commit (ddb init/create/
# query/...) WITHOUT `cd /tmp/...` inside the block. Walkthroughs 00001-00049
# use inline `cd /tmp/...` and would NOT contaminate even if verify ran in
# repo cwd, so picking one of those would let the test pass vacuously.
#
# Strategy:
#   1. Prefer 00050-alter-table-rename.md (the historically contaminating one
#      from PRD 00135) if it qualifies.
#   2. Otherwise scan walkthroughs for the pattern: contains
#      `ddb {init|create|query|delete|update}` AND does NOT contain
#      `cd /tmp` or `cd /var/tmp`.
#   3. If none qualify, fail fast — running the test against a self-isolating
#      walkthrough proves nothing.
is_contaminating_fixture() {
  local file="$1"
  # Self-isolating walkthroughs `cd /tmp/...` (or /var/tmp) inside their bash
  # blocks; they would not contaminate even if verify ran from repo cwd.
  if grep -qE 'cd (/tmp|/var/tmp)' "$file"; then
    return 1
  fi
  # The contamination risk requires ddb auto-commit commands.
  if grep -qE 'ddb (init|create|query|delete|update)' "$file"; then
    return 0
  fi
  return 1
}

FIXTURE=""
PRIMARY="${REPO_ROOT}/dev/local/walkthroughs/00050-alter-table-rename.md"
if [[ -f "$PRIMARY" ]] && is_contaminating_fixture "$PRIMARY"; then
  FIXTURE="$PRIMARY"
else
  while IFS= read -r -d '' candidate; do
    if is_contaminating_fixture "$candidate"; then
      FIXTURE="$candidate"
      break
    fi
  done < <(find "${REPO_ROOT}/dev/local/walkthroughs" -maxdepth 1 -name '*.md' -print0 | sort -z)
fi

if [[ -z "$FIXTURE" || ! -f "$FIXTURE" ]]; then
  echo "error: no contaminating walkthrough fixture available under dev/local/walkthroughs/" >&2
  echo "       a contaminating fixture has bash blocks invoking ddb init/create/query" >&2
  echo "       WITHOUT a 'cd /tmp/...' inside the block. Walkthroughs 00050+ qualify." >&2
  echo "       this test depends on local walkthroughs (gitignored)." >&2
  exit 2
fi

pass() { printf '  ✓ %s\n' "$1"; }
fail() { printf '  ✗ %s\n' "$1"; exit 1; }

echo "=== showboat-verify contamination regression test ==="
echo "fixture: $FIXTURE"

# --- pre-state ---
PRE_HEAD="$(git -C "$REPO_ROOT" rev-parse HEAD)"
PRE_STATUS="$(git -C "$REPO_ROOT" status --porcelain)"
PRE_WORKTREES="$(git -C "$REPO_ROOT" worktree list)"

declare -a DATA_DIRS=("ddb" ".ddb" ".crdt" ".nodes")
declare -A PRE_DATA_HASH=()
for d in "${DATA_DIRS[@]}"; do
  if [[ -d "${REPO_ROOT}/${d}" ]]; then
    PRE_DATA_HASH[$d]="$(find "${REPO_ROOT}/${d}" -type f -print0 | sort -z | xargs -0 sha256sum 2>/dev/null | sha256sum | awk '{print $1}')"
  else
    PRE_DATA_HASH[$d]="MISSING"
  fi
done
pass "pre-state captured (HEAD=${PRE_HEAD:0:10})"

# --- run wrapper ---
# Wrapper may exit 1 if verify reports output diffs (e.g. timestamps differ).
# That is unrelated to contamination — we only care about side effects.
# Exit ≥ 2 means the wrapper failed before verify ran (usage error, worktree
# add failure, etc.); the test cannot prove no-contamination in that case.
WRAPPER_EXIT=0
"$WRAPPER" "$FIXTURE" >/tmp/safe-showboat-verify-test.log 2>&1 || WRAPPER_EXIT=$?
pass "wrapper ran (exit=${WRAPPER_EXIT})"

if (( WRAPPER_EXIT >= 2 )); then
  echo "    wrapper log:"
  sed 's/^/      /' /tmp/safe-showboat-verify-test.log
  fail "wrapper failed before verify ran (exit=${WRAPPER_EXIT}); test inconclusive"
fi

# --- post-state ---
POST_HEAD="$(git -C "$REPO_ROOT" rev-parse HEAD)"
POST_STATUS="$(git -C "$REPO_ROOT" status --porcelain)"
POST_WORKTREES="$(git -C "$REPO_ROOT" worktree list)"

# --- assertions ---
if [[ "$PRE_HEAD" != "$POST_HEAD" ]]; then
  echo "    pre  HEAD: $PRE_HEAD"
  echo "    post HEAD: $POST_HEAD"
  echo "    new commits:"
  git -C "$REPO_ROOT" log --oneline "${PRE_HEAD}..${POST_HEAD}" | sed 's/^/      /'
  fail "HEAD changed — wrapper contaminated master"
fi
pass "HEAD unchanged"

if [[ "$PRE_STATUS" != "$POST_STATUS" ]]; then
  echo "    pre status:"
  echo "$PRE_STATUS" | sed 's/^/      /'
  echo "    post status:"
  echo "$POST_STATUS" | sed 's/^/      /'
  fail "working tree status changed"
fi
pass "working tree status unchanged"

for d in "${DATA_DIRS[@]}"; do
  if [[ -d "${REPO_ROOT}/${d}" ]]; then
    POST_HASH="$(find "${REPO_ROOT}/${d}" -type f -print0 | sort -z | xargs -0 sha256sum 2>/dev/null | sha256sum | awk '{print $1}')"
  else
    POST_HASH="MISSING"
  fi
  if [[ "${PRE_DATA_HASH[$d]}" != "$POST_HASH" ]]; then
    echo "    ${d}: pre=${PRE_DATA_HASH[$d]:0:12} post=${POST_HASH:0:12}"
    fail "${d}/ contents changed"
  fi
done
pass "data dirs (ddb/, .ddb/, .crdt/, .nodes/) unchanged"

if [[ "$PRE_WORKTREES" != "$POST_WORKTREES" ]]; then
  echo "    pre worktrees:"
  echo "$PRE_WORKTREES" | sed 's/^/      /'
  echo "    post worktrees:"
  echo "$POST_WORKTREES" | sed 's/^/      /'
  fail "worktree list changed — wrapper did not clean up"
fi
pass "worktree list unchanged (no leaked worktrees)"

echo ""
echo "OK: showboat verify did not contaminate master"
exit 0
