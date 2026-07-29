#!/usr/bin/env bash
# Offline tests for contrib/ai-commit. No network: every case stubs the API
# response via AI_COMMIT_STUB_RESPONSE.

set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=contrib/test-lib.sh
. "$HERE/test-lib.sh"

AI_COMMIT="$HERE/ai-commit"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

DIFF='diff --git a/a.txt b/a.txt
--- a/a.txt
+++ b/a.txt
@@ -0,0 +1 @@
+hello'

printf '\n  ai-commit\n\n'

# --- empty diff -----------------------------------------------------------
out="$(printf '' | "$AI_COMMIT" 2>&1)"
status=$?
assert_eq "empty diff exits 1" 1 "$status"
assert_contains "empty diff explains itself" "$out" "No staged changes"

# --- happy path -----------------------------------------------------------
cat >"$TMP/ok.json" <<'EOF'
{"choices":[{"message":{"content":"feat: add a.txt\n\n- add a greeting file","reasoning_content":"deliberating"},"finish_reason":"stop"}]}
EOF
out="$(printf '%s' "$DIFF" | AI_COMMIT_STUB_RESPONSE="$TMP/ok.json" "$AI_COMMIT" 2>&1)"
status=$?
assert_eq "happy path exits 0" 0 "$status"
assert_eq "happy path prints content, not reasoning" "feat: add a.txt

- add a greeting file" "$out"

# --- API error ------------------------------------------------------------
cat >"$TMP/err.json" <<'EOF'
{"error":{"message":"Authentication Fails, Your api key: ****ogus is invalid"}}
EOF
out="$(printf '%s' "$DIFF" | AI_COMMIT_STUB_RESPONSE="$TMP/err.json" \
    AI_COMMIT_STUB_HTTP_CODE=401 "$AI_COMMIT" 2>&1)"
status=$?
assert_eq "api error exits 1" 1 "$status"
assert_contains "api error surfaces the status" "$out" "HTTP 401"
assert_contains "api error surfaces the message" "$out" "Authentication Fails"

# --- reasoning ate the whole budget ---------------------------------------
cat >"$TMP/starved.json" <<'EOF'
{"choices":[{"message":{"content":"","reasoning_content":"thought at length"},"finish_reason":"length"}]}
EOF
out="$(printf '%s' "$DIFF" | AI_COMMIT_STUB_RESPONSE="$TMP/starved.json" \
    AI_COMMIT_MAX_TOKENS=20 "$AI_COMMIT" 2>&1)"
status=$?
assert_eq "starved budget exits 1" 1 "$status"
assert_contains "starved budget names the fix" "$out" "AI_COMMIT_MAX_TOKENS"

test_summary
