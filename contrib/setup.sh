#!/usr/bin/env bash
# Wire this checkout into the current user's environment. Idempotent, no sudo.
#
#   ~/.local/bin/ai-lazygitrs-commit -> <checkout>/contrib/ai-commit
#   ~/.config/lazygitrs/config.yml   -> <checkout>/contrib/config.yml
#
# The helper is linked into ~/.local/bin rather than referenced in the config by
# its checkout path: if generateCommand named a checkout path, cloning to a
# different directory on another machine would break it — the same class of bug
# this setup exists to prevent.
#
# Exits 0 whenever the symlink work succeeded, even when the closing report
# lists missing tools: on a fresh machine jq/delta are legitimately absent until
# `make setup-deps` runs, and `make install setup` must not look like a failure.

set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BIN_DIR="$HOME/.local/bin"
CONFIG_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/lazygitrs"

link_into_place() { # target linkpath
    local target="$1" linkpath="$2"

    if [ -L "$linkpath" ]; then
        if [ "$(readlink "$linkpath")" = "$target" ]; then
            printf '  = already linked: %s\n' "$linkpath"
            return 0
        fi
        rm -f "$linkpath"
    elif [ -e "$linkpath" ]; then
        mv "$linkpath" "$linkpath.bak"
        printf '  ! existing file backed up: %s -> %s.bak\n' "$linkpath" "$linkpath"
    fi

    ln -s "$target" "$linkpath"
    printf '  + linked: %s -> %s\n' "$linkpath" "$target"
}

printf '\n  lazygitrs setup\n\n'

mkdir -p "$BIN_DIR" "$CONFIG_DIR"
chmod +x "$HERE/ai-commit" "$HERE/doctor.sh"

link_into_place "$HERE/ai-commit" "$BIN_DIR/ai-lazygitrs-commit"
link_into_place "$HERE/config.yml" "$CONFIG_DIR/config.yml"

case ":$PATH:" in
    *":$BIN_DIR:"*) ;;
    *) printf '  ! %s is not on PATH — add it so generateCommand resolves\n' "$BIN_DIR" ;;
esac

# The report is informational; a missing optional tool must not fail setup.
"$HERE/doctor.sh" || true

exit 0
