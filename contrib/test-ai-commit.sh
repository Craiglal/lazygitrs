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

# --- large diff -----------------------------------------------------------
# `printf '%s' "$x" | grep -q` under `set -o pipefail` used to fail the pipeline
# with SIGPIPE once the payload outgrew the 64 KiB pipe buffer: grep -q exits at
# the first match while printf is still writing. That reported any large staged
# diff as "No staged changes to describe."
BIG_DIFF="$DIFF
$(head -c 200000 /dev/zero | tr '\0' 'x')"
out="$(printf '%s' "$BIG_DIFF" | AI_COMMIT_STUB_RESPONSE="$TMP/ok.json" "$AI_COMMIT" 2>&1)"
status=$?
assert_eq "large diff exits 0" 0 "$status"
assert_contains "large diff is not mistaken for an empty one" "$out" "feat: add a.txt"

# Same pipeline hazard on the response side.
head -c 200000 /dev/zero | tr '\0' 'y' >"$TMP/big-message.txt"
jq -n --rawfile c "$TMP/big-message.txt" \
    '{choices:[{message:{content:$c},finish_reason:"stop"}]}' >"$TMP/big.json"
out="$(printf '%s' "$DIFF" | AI_COMMIT_STUB_RESPONSE="$TMP/big.json" "$AI_COMMIT" 2>&1)"
status=$?
assert_eq "large message exits 0" 0 "$status"
assert_eq "large message is not mistaken for an empty one" 200000 "${#out}"

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
    AI_COMMIT_REASONING_EFFORT=high AI_COMMIT_MAX_TOKENS=20 "$AI_COMMIT" 2>&1)"
status=$?
assert_eq "starved budget exits 1" 1 "$status"
assert_contains "starved budget names the fix" "$out" "AI_COMMIT_MAX_TOKENS"
assert_contains "starved budget offers to disable reasoning" "$out" \
    "AI_COMMIT_REASONING_EFFORT=none"

# With reasoning already off, an empty message means the budget was too small
# for the message itself — blaming reasoning would misdirect the fix.
out="$(printf '%s' "$DIFF" | AI_COMMIT_STUB_RESPONSE="$TMP/starved.json" \
    AI_COMMIT_MAX_TOKENS=20 "$AI_COMMIT" 2>&1)"
assert_contains "empty message with reasoning off does not blame reasoning" "$out" \
    "before producing any message"

# --- message cut off at the budget ----------------------------------------
# A message that stops mid-word is not a commit message; failing loudly beats
# handing lazygitrs a truncated one to commit.
cat >"$TMP/truncated.json" <<'EOF'
{"choices":[{"message":{"content":"feat: add a.txt\n\n- add a greeting file and then stop mid-"},"finish_reason":"length"}]}
EOF
out="$(printf '%s' "$DIFF" | AI_COMMIT_STUB_RESPONSE="$TMP/truncated.json" "$AI_COMMIT" 2>&1)"
status=$?
assert_eq "truncated message exits 1" 1 "$status"
assert_contains "truncated message says it was truncated" "$out" "truncated"
assert_contains "truncated message names the fix" "$out" "AI_COMMIT_MAX_TOKENS"

# --- request payload ------------------------------------------------------
# The model draws reasoning tokens from the same max_tokens budget as the
# message, so reasoning is off by default: a diff big enough to provoke long
# reasoning would otherwise leave nothing for the commit message.
got="$(printf '%s' "$DIFF" | AI_COMMIT_STUB_RESPONSE="$TMP/ok.json" \
    AI_COMMIT_DUMP_PAYLOAD="$TMP/payload.json" "$AI_COMMIT" >/dev/null 2>&1
    jq -r '.reasoning_effort' "$TMP/payload.json" 2>/dev/null)"
assert_eq "payload disables reasoning by default" "none" "$got"

got="$(printf '%s' "$DIFF" | AI_COMMIT_STUB_RESPONSE="$TMP/ok.json" \
    AI_COMMIT_DUMP_PAYLOAD="$TMP/payload.json" AI_COMMIT_REASONING_EFFORT=high \
    "$AI_COMMIT" >/dev/null 2>&1
    jq -r '.reasoning_effort' "$TMP/payload.json" 2>/dev/null)"
assert_eq "payload honours AI_COMMIT_REASONING_EFFORT" "high" "$got"

# Empty means "send no reasoning_effort at all", for API deployments that
# reject the field.
got="$(printf '%s' "$DIFF" | AI_COMMIT_STUB_RESPONSE="$TMP/ok.json" \
    AI_COMMIT_DUMP_PAYLOAD="$TMP/payload.json" AI_COMMIT_REASONING_EFFORT= \
    "$AI_COMMIT" >/dev/null 2>&1
    jq -r 'has("reasoning_effort")' "$TMP/payload.json" 2>/dev/null)"
assert_eq "empty AI_COMMIT_REASONING_EFFORT omits the field" "false" "$got"

test_summary
