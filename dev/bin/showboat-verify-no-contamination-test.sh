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

# `git rev-parse` exits non-zero when not in a repo. With `set -e` that aborts
# the script with git's own "fatal: not a git repository" message, so we don't
# need an extra empty-string check here.
REPO_ROOT="$(git rev-parse --show-toplevel)"

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
#   2. Otherwise scan walkthroughs for the pattern: any fenced code block
#      that contains `ddb {init|create|query|delete|update}` AND lacks an
#      inline `cd /tmp` or `cd /var/tmp` in THAT same block.
#   3. If none qualify, fail fast — running the test against a self-isolating
#      walkthrough proves nothing.
#
# Per-block evaluation matters: per AGENTS.md, each `showboat exec` runs in
# its own shell, so a `cd /tmp` in block N does NOT carry over to block N+1.
# Whole-file grep would miss this, classifying a walkthrough that has cd in
# block 1 and a bare `ddb create` in block 2 as self-isolating when it isn't.
# Restricting the scan to fenced code blocks also avoids prose false-positives
# (e.g. a comment line "no cd /tmp needed" must not count as self-isolation).
is_contaminating_fixture() {
  local file="$1"
  awk '
    BEGIN { in_block = 0; has_ddb = 0; has_cd = 0; contaminating = 0 }
    /^```/ {
      if (in_block) {
        if (has_ddb && !has_cd) contaminating = 1
        in_block = 0; has_ddb = 0; has_cd = 0
      } else {
        in_block = 1
      }
      next
    }
    in_block {
      if ($0 ~ /ddb (init|create|query|delete|update)/) has_ddb = 1
      if ($0 ~ /cd (\/tmp|\/var\/tmp)/) has_cd = 1
    }
    END { exit contaminating ? 0 : 1 }
  ' "$file"
}

FIXTURE=""
PRIMARY="${REPO_ROOT}/dev/local/walkthroughs/00050-alter-table-rename.md"
if [[ -f "$PRIMARY" ]] && is_contaminating_fixture "$PRIMARY"; then
  FIXTURE="$PRIMARY"
else
  # Restrict the fallback scan to 00050+ walkthroughs. AGENTS.md documents this
  # bound, and walkthroughs 00001-00049 use inline `cd /tmp/...` in every block
  # so they would not exhibit the contamination pattern anyway — but enforcing
  # the lower bound here keeps documentation and behavior in lockstep.
  while IFS= read -r -d '' candidate; do
    base="$(basename "$candidate")"
    # 00050-style basenames sort lexicographically; anything < "00050-" is older.
    [[ "$base" < "00050-" ]] && continue
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
    PRE_DATA_HASH[$d]="$(find "${REPO_ROOT}/${d}" -type f -print0 | sort -z | xargs -0 shasum -a 256 2>/dev/null | shasum -a 256 | awk '{print $1}')"
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
# Use mktemp so two parallel runs don't clobber each other's log.
WRAPPER_LOG="$(mktemp -t safe-showboat-verify-test.XXXXXX)"
trap 'rm -f "$WRAPPER_LOG"' EXIT
WRAPPER_EXIT=0
"$WRAPPER" "$FIXTURE" >"$WRAPPER_LOG" 2>&1 || WRAPPER_EXIT=$?
pass "wrapper ran (exit=${WRAPPER_EXIT})"

if (( WRAPPER_EXIT >= 2 )); then
  echo "    wrapper log:"
  sed 's/^/      /' "$WRAPPER_LOG"
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
    POST_HASH="$(find "${REPO_ROOT}/${d}" -type f -print0 | sort -z | xargs -0 shasum -a 256 2>/dev/null | shasum -a 256 | awk '{print $1}')"
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
