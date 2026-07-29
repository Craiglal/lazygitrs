#!/usr/bin/env bash
# Install the tools lazygitrs' config references. Opt-in: setup.sh never calls
# this, because installing system packages should be an explicit choice.
#
#   jq, git-delta   official Arch repositories
#   lazyworktree    AUR — needs paru or yay
#
# Pass --dry-run to print the commands without running them.

set -euo pipefail

DRY_RUN=0
if [ "${1:-}" = "--dry-run" ]; then
    DRY_RUN=1
fi

run() {
    if [ "$DRY_RUN" -eq 1 ]; then
        printf '  would run: %s\n' "$*"
    else
        printf '  running: %s\n' "$*"
        "$@"
    fi
}

printf '\n  lazygitrs setup-deps\n\n'

AUR_HELPER=""
for candidate in paru yay; do
    if command -v "$candidate" >/dev/null 2>&1; then
        AUR_HELPER="$candidate"
        break
    fi
done

if [ -n "$AUR_HELPER" ]; then
    run "$AUR_HELPER" -S --needed --noconfirm jq git-delta lazyworktree
elif command -v pacman >/dev/null 2>&1; then
    run sudo pacman -S --needed --noconfirm jq git-delta
    printf '  ! lazyworktree is an AUR package — install paru or yay, then re-run\n'
else
    printf '  \033[31m✗\033[0m no supported package manager found (looked for paru, yay, pacman)\n' >&2
    printf '    install these manually: jq, git-delta, lazyworktree\n' >&2
    exit 1
fi

printf '\n  done — verify with: make doctor\n\n'
