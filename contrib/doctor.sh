#!/usr/bin/env bash
# Diagnose a lazygitrs installation. Read-only: changes nothing.
#
# Exit codes:
#   0  every required check passed (optional tools may still be missing)
#   1  at least one required check failed
#
# Required : lazygitrs on PATH, config.yml present, generateCommand resolves,
#            jq, curl, and DEEPSEEK_API_KEY when generateCommand resolves to
#            the bundled ai-lazygitrs-commit helper specifically
# Optional : delta, lazyworktree

set -uo pipefail

CONFIG_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/lazygitrs"
CONFIG_FILE="$CONFIG_DIR/config.yml"
FAILED=0

ok()   { printf '  \033[32m✓\033[0m %s\n' "$*"; }
warn() { printf '  \033[33m!\033[0m %s\n' "$*"; }
bad()  { printf '  \033[31m✗\033[0m %s\n' "$*"; FAILED=1; }

# Pull the generateCommand value from a lazygit-style config.yml. Deliberately
# simple: it matches the common single-line `generateCommand: <value>` form and
# strips one layer of surrounding quotes.
extract_generate_command() {
    sed -n 's/^[[:space:]]*generateCommand:[[:space:]]*//p' "$1" \
        | head -n1 \
        | sed -e "s/^'\(.*\)'\$/\1/" -e 's/^"\(.*\)"$/\1/'
}

printf '\n  lazygitrs doctor\n\n'

# --- the binary -----------------------------------------------------------
if command -v lazygitrs >/dev/null 2>&1; then
    ok "lazygitrs on PATH ($(lazygitrs --version 2>/dev/null | head -n1))"
else
    bad "lazygitrs not on PATH (required) — run: make install"
fi

# --- the config -----------------------------------------------------------
if [ -L "$CONFIG_FILE" ]; then
    if [ -e "$CONFIG_FILE" ]; then
        ok "config.yml → $(readlink "$CONFIG_FILE")"
    else
        bad "config.yml is a broken symlink → $(readlink "$CONFIG_FILE") (required) — run: make setup"
    fi
elif [ -f "$CONFIG_FILE" ]; then
    warn "config.yml is a regular file, not the tracked symlink — run: make setup"
else
    bad "config.yml missing at $CONFIG_FILE (required) — run: make setup"
fi

# --- generateCommand ------------------------------------------------------
gen_cmd=""
if [ -e "$CONFIG_FILE" ]; then
    gen_cmd="$(extract_generate_command "$CONFIG_FILE")"
fi

# Set only when the resolved first word names the bundled helper; gates
# whether DEEPSEEK_API_KEY is required below. Every other generateCommand
# (claude, opencode, codex exec, modelcli, ...) manages its own credentials.
is_bundled_helper=0

if [ -z "$gen_cmd" ]; then
    warn "no generateCommand configured — AI commit is disabled"
else
    case "$gen_cmd" in
        *'$('* | *'&&'* | *'|'* | *';'*)
            warn "generateCommand is a shell expression; skipping the executable check"
            ;;
        *)
            # A leading quote means the intended token can contain spaces
            # (e.g. `"/path with spaces/cmd" --flag`); strip one layer of
            # quotes and take everything up to the matching closing quote
            # rather than splitting on the first space, which would truncate
            # the path mid-token. Unquoted values keep the plain first-word
            # split, matching how sh -c would word-split them too.
            case "$gen_cmd" in
                '"'*)
                    rest="${gen_cmd#\"}"
                    first_word="${rest%%\"*}"
                    ;;
                "'"*)
                    rest="${gen_cmd#\'}"
                    first_word="${rest%%\'*}"
                    ;;
                *)
                    first_word="${gen_cmd%% *}"
                    ;;
            esac
            case "$first_word" in
                '$HOME'*)   first_word="$HOME${first_word#\$HOME}" ;;
                '${HOME}'*) first_word="$HOME${first_word#\$\{HOME\}}" ;;
                '~'*)       first_word="$HOME${first_word#\~}" ;;
            esac

            case "$first_word" in
                *=*)
                    warn "generateCommand starts with an env assignment; skipping the executable check"
                    ;;
                *)
                    case "$first_word" in
                        *ai-lazygitrs-commit) is_bundled_helper=1 ;;
                    esac
                    if command -v "$first_word" >/dev/null 2>&1; then
                        ok "generateCommand → $first_word"
                    else
                        bad "generateCommand → $first_word does not exist or is not executable (required)"
                    fi
                    ;;
            esac
            ;;
    esac
fi

# --- dependencies ---------------------------------------------------------
for required in jq curl; do
    if command -v "$required" >/dev/null 2>&1; then
        ok "$required found"
    else
        bad "$required missing (required) — run: make setup-deps"
    fi
done

if command -v delta >/dev/null 2>&1; then
    ok "delta found"
else
    warn "delta missing (optional) — used by git.pagers; run: make setup-deps"
fi

if command -v lazyworktree >/dev/null 2>&1; then
    ok "lazyworktree found"
else
    warn "lazyworktree missing (optional) — used by the K custom command; run: make setup-deps"
fi

# --- the API key ----------------------------------------------------------
# Presence only. The value is never printed. Only the bundled helper
# (ai-lazygitrs-commit) needs this key; other generateCommand options
# (claude, opencode, codex exec, modelcli, ...) manage their own credentials.
if [ "$is_bundled_helper" -eq 1 ]; then
    if [ -n "${DEEPSEEK_API_KEY:-}" ]; then
        ok "DEEPSEEK_API_KEY is set"
    else
        bad "DEEPSEEK_API_KEY is not set (required by generateCommand) — add to your shell's env file (e.g. ~/.zshenv): export DEEPSEEK_API_KEY=<key>"
    fi
fi

printf '\n'
if [ "$FAILED" -eq 0 ]; then
    printf '  \033[32mall required checks passed\033[0m\n\n'
else
    printf '  \033[31mone or more required checks failed\033[0m\n\n'
fi

exit "$FAILED"
