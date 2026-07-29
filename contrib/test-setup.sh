#!/usr/bin/env bash
# Tests for contrib/setup.sh and contrib/doctor.sh, driven against scratch homes.

set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=contrib/test-lib.sh
. "$HERE/test-lib.sh"

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

# Run a contrib script against a scratch HOME.
in_home() { # home script [args...]
    local home="$1"; shift
    HOME="$home" XDG_CONFIG_HOME="$home/.config" "$@" 2>&1
}

printf '\n  setup\n\n'

# --- fresh home -----------------------------------------------------------
FRESH="$TMP/fresh"
mkdir -p "$FRESH"
out="$(in_home "$FRESH" "$HERE/setup.sh")"
status=$?
assert_eq "setup exits 0 on a fresh home" 0 "$status"
assert_eq "helper symlink target" "$HERE/ai-commit" \
    "$(readlink "$FRESH/.local/bin/ai-lazygitrs-commit")"
assert_eq "config symlink target" "$HERE/config.yml" \
    "$(readlink "$FRESH/.config/lazygitrs/config.yml")"

# --- idempotency ----------------------------------------------------------
out="$(in_home "$FRESH" "$HERE/setup.sh")"
status=$?
assert_eq "re-running setup exits 0" 0 "$status"
assert_contains "re-running setup is a no-op" "$out" "already linked"

# --- never clobber a real file -------------------------------------------
OCCUPIED="$TMP/occupied"
mkdir -p "$OCCUPIED/.config/lazygitrs"
printf 'gui:\n  showFileTree: false\n' >"$OCCUPIED/.config/lazygitrs/config.yml"
out="$(in_home "$OCCUPIED" "$HERE/setup.sh")"
assert_contains "an existing config is backed up" "$out" "backed up"
assert_contains "the backup keeps the original content" \
    "$(cat "$OCCUPIED/.config/lazygitrs/config.yml.bak")" "showFileTree: false"
assert_eq "the config is now the tracked symlink" "$HERE/config.yml" \
    "$(readlink "$OCCUPIED/.config/lazygitrs/config.yml")"

# --- a second distinct real file must not clobber the first backup --------
# Regression: mv previously overwrote <name>.bak unconditionally, so if a real
# file reappeared at the link path after a first backup was already made, the
# second run silently destroyed the first backup's content.
COLLIDE="$TMP/collide"
mkdir -p "$COLLIDE/.config/lazygitrs"
printf 'ORIGINAL-A\n' >"$COLLIDE/.config/lazygitrs/config.yml"
in_home "$COLLIDE" "$HERE/setup.sh" >/dev/null
rm -f "$COLLIDE/.config/lazygitrs/config.yml"
printf 'REPLACEMENT-B\n' >"$COLLIDE/.config/lazygitrs/config.yml"
in_home "$COLLIDE" "$HERE/setup.sh" >/dev/null
assert_contains "the first backup survives a second real file" \
    "$(cat "$COLLIDE/.config/lazygitrs/config.yml.bak")" "ORIGINAL-A"
assert_contains "the second backup gets a distinct name" \
    "$(cat "$COLLIDE/.config/lazygitrs/config.yml.bak.1")" "REPLACEMENT-B"

# --- warns when an existing lazygit config would be shadowed --------------
# config_dir_candidates() (src/config/mod.rs) checks .../lazygitrs before
# .../lazygit with no merging, so linking the lazygitrs config silently stops
# an existing lazygit config.yml from being read. Warning only — nothing is
# deleted or moved, and it must not affect setup's exit code.
LEGACY="$TMP/legacy"
mkdir -p "$LEGACY/.config/lazygit"
printf 'gui:\n  showFileTree: true\n' >"$LEGACY/.config/lazygit/config.yml"
out="$(in_home "$LEGACY" "$HERE/setup.sh")"
status=$?
assert_eq "setup still exits 0 when warning about a legacy config" 0 "$status"
assert_contains "setup warns about a shadowed legacy lazygit config" "$out" \
    "lazygitrs will now read the lazygitrs config instead"

NO_LEGACY="$TMP/no-legacy"
mkdir -p "$NO_LEGACY"
out="$(in_home "$NO_LEGACY" "$HERE/setup.sh")"
case "$out" in
    *"lazygitrs will now read the lazygitrs config instead"*) shadow_warning="present" ;;
    *) shadow_warning="absent" ;;
esac
assert_eq "no shadow warning without a legacy lazygit config" "absent" "$shadow_warning"

printf '\n  doctor\n\n'

# Assertions below check messages rather than exit codes: doctor's exit status
# also depends on whether lazygitrs/jq/curl exist in the test environment.

# --- generateCommand that cannot resolve ---------------------------------
BROKEN="$TMP/broken"
mkdir -p "$BROKEN/.config/lazygitrs"
printf "git:\n  commit:\n    generateCommand: '/nonexistent/ai-helper'\n" \
    >"$BROKEN/.config/lazygitrs/config.yml"
out="$(in_home "$BROKEN" "$HERE/doctor.sh")"
assert_contains "doctor names the unresolvable path" "$out" "/nonexistent/ai-helper"
assert_contains "doctor explains the failure" "$out" "does not exist or is not executable"

# --- generateCommand that resolves --------------------------------------
GOOD="$TMP/good"
mkdir -p "$GOOD/.config/lazygitrs"
printf "git:\n  commit:\n    generateCommand: '/bin/sh'\n" \
    >"$GOOD/.config/lazygitrs/config.yml"
out="$(in_home "$GOOD" "$HERE/doctor.sh")"
assert_contains "doctor accepts a resolvable command" "$out" "generateCommand → /bin/sh"

# --- $HOME expansion ----------------------------------------------------
EXPAND="$TMP/expand"
mkdir -p "$EXPAND/.config/lazygitrs" "$EXPAND/.local/bin"
printf '#!/bin/sh\ntrue\n' >"$EXPAND/.local/bin/helper"
chmod +x "$EXPAND/.local/bin/helper"
printf "git:\n  commit:\n    generateCommand: '\$HOME/.local/bin/helper'\n" \
    >"$EXPAND/.config/lazygitrs/config.yml"
out="$(in_home "$EXPAND" "$HERE/doctor.sh")"
assert_contains "doctor expands \$HOME" "$out" "generateCommand → $EXPAND/.local/bin/helper"

# --- quoted path containing spaces resolves -------------------------------
# Regression coverage: doctor's generateCommand resolution used to split on
# the first space (`${gen_cmd%% *}`), which truncated a quoted path mid-token
# and falsely reported a working install as broken. It now strips one layer
# of surrounding quotes from the first token before checking it.
QUOTED="$TMP/quoted"
mkdir -p "$QUOTED/.config/lazygitrs" "$QUOTED/my bin"
printf '#!/bin/sh\ntrue\n' >"$QUOTED/my bin/helper"
chmod +x "$QUOTED/my bin/helper"
printf 'git:\n  commit:\n    generateCommand: '"'"'"$HOME/my bin/helper" --flag'"'"'\n' \
    >"$QUOTED/.config/lazygitrs/config.yml"
out="$(in_home "$QUOTED" "$HERE/doctor.sh")"
assert_contains "doctor resolves a quoted spaced path" "$out" \
    "generateCommand → $QUOTED/my bin/helper"

# --- bare unquoted spaced path still fails --------------------------------
# The mirror case: without quotes, lazygitrs runs the value through sh -c,
# which word-splits on the space and fails — so doctor's ✗ here is correct,
# not a regression. A test asserting this resolves would be wrong.
UNQUOTED="$TMP/unquoted"
mkdir -p "$UNQUOTED/.config/lazygitrs" "$UNQUOTED/my bin"
printf '#!/bin/sh\ntrue\n' >"$UNQUOTED/my bin/helper"
chmod +x "$UNQUOTED/my bin/helper"
printf 'git:\n  commit:\n    generateCommand: %s/my bin/helper --flag\n' "\$HOME" \
    >"$UNQUOTED/.config/lazygitrs/config.yml"
out="$(in_home "$UNQUOTED" "$HERE/doctor.sh")"
assert_contains "doctor still fails a bare unquoted spaced path" "$out" \
    "does not exist or is not executable"

# --- DEEPSEEK_API_KEY is required only for the bundled helper -------------
# Assertions check messages, not exit codes: doctor's exit status also
# depends on lazygitrs/jq/curl presence in the test environment.

# A non-DeepSeek generateCommand that resolves must not fail on the key.
NO_DEEPSEEK="$TMP/no-deepseek"
mkdir -p "$NO_DEEPSEEK/.config/lazygitrs"
printf "git:\n  commit:\n    generateCommand: '/bin/sh'\n" \
    >"$NO_DEEPSEEK/.config/lazygitrs/config.yml"
out="$(in_home "$NO_DEEPSEEK" env -u DEEPSEEK_API_KEY "$HERE/doctor.sh")"
case "$out" in
    *"DEEPSEEK_API_KEY is not set"*) key_failure="present" ;;
    *) key_failure="absent" ;;
esac
assert_eq "non-bundled generateCommand does not require the key" "absent" "$key_failure"

# A generateCommand resolving to the bundled helper must still require it.
BUNDLED="$TMP/bundled-helper"
mkdir -p "$BUNDLED/.config/lazygitrs" "$BUNDLED/.local/bin"
printf '#!/bin/sh\ntrue\n' >"$BUNDLED/.local/bin/ai-lazygitrs-commit"
chmod +x "$BUNDLED/.local/bin/ai-lazygitrs-commit"
printf "git:\n  commit:\n    generateCommand: '\$HOME/.local/bin/ai-lazygitrs-commit'\n" \
    >"$BUNDLED/.config/lazygitrs/config.yml"
out="$(in_home "$BUNDLED" env -u DEEPSEEK_API_KEY "$HERE/doctor.sh")"
assert_contains "bundled helper still requires the key" "$out" \
    "DEEPSEEK_API_KEY is not set (required by generateCommand)"

printf '\n  config sanity\n\n'

# --- Global Constraint: no machine-specific absolute path in the tracked
# config. A literal /home/<user> is what silently broke AI commit after a
# machine move (see the header comment in contrib/config.yml); this makes the
# constraint structural rather than comment-only.
CONFIG_CONTENTS="$(cat "$HERE/config.yml")"
case "$CONFIG_CONTENTS" in
    *"/home/"*) home_literal="present" ;;
    *) home_literal="absent" ;;
esac
assert_eq "tracked config.yml has no /home/ literal" "absent" "$home_literal"

case "$CONFIG_CONTENTS" in
    *"/Users/"*) users_literal="present" ;;
    *) users_literal="absent" ;;
esac
assert_eq "tracked config.yml has no /Users/ literal" "absent" "$users_literal"

test_summary
