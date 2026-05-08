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

# Pick a walkthrough fixture. 00050-alter-table-rename.md is the historically
# contaminating one (PRD 00135). Fall back to any other walkthrough if missing.
FIXTURE="${REPO_ROOT}/dev/local/walkthroughs/00050-alter-table-rename.md"
if [[ ! -f "$FIXTURE" ]]; then
  FIXTURE="$(find "${REPO_ROOT}/dev/local/walkthroughs" -maxdepth 1 -name '*.md' -print -quit 2>/dev/null || true)"
fi
if [[ -z "$FIXTURE" || ! -f "$FIXTURE" ]]; then
  echo "error: no walkthrough fixture available under dev/local/walkthroughs/" >&2
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
WRAPPER_EXIT=0
"$WRAPPER" "$FIXTURE" >/tmp/safe-showboat-verify-test.log 2>&1 || WRAPPER_EXIT=$?
pass "wrapper ran (exit=${WRAPPER_EXIT})"

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
