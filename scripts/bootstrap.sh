#!/usr/bin/env sh
# Prism bootstrap — clone, build and set up in one command.
#
#   curl -fsSL https://raw.githubusercontent.com/Syncrose1/Prism/main/scripts/bootstrap.sh | sh
#
# Deliberately POSIX sh with no dependencies beyond git and a Rust toolchain, so
# it runs on a machine that has nothing else set up yet.
#
# On piped input the interactive steps are skipped and reported, rather than
# silently doing nothing: setting a password needs a terminal, and pretending
# otherwise would leave the operator wondering why every sign-in wants a code.

set -eu

REPO="${PRISM_REPO:-https://github.com/Syncrose1/Prism.git}"
DEST="${PRISM_DIR:-$HOME/Prism}"
BRANCH="${PRISM_BRANCH:-main}"

say() { printf '\n\033[1m%s\033[0m\n' "$*"; }
die() { printf '\033[31merror:\033[0m %s\n' "$*" >&2; exit 1; }

say "Prism bootstrap"

command -v git >/dev/null || die "git is required"
if ! command -v cargo >/dev/null; then
    printf '  Rust is required. Install it with:\n'
    printf '    curl --proto =https --tlsv1.2 -sSf https://sh.rustup.rs | sh\n'
    die "cargo not found"
fi

if [ -d "$DEST/.git" ]; then
    printf '  updating %s\n' "$DEST"
    git -C "$DEST" fetch --quiet origin "$BRANCH"
    # Rebase rather than reset: local edits to config or scripts are the
    # operator's, and discarding them silently would be hostile.
    git -C "$DEST" rebase --quiet "origin/$BRANCH" \
        || die "local changes conflict with upstream; resolve them in $DEST"
else
    printf '  cloning into %s\n' "$DEST"
    git clone --quiet --branch "$BRANCH" "$REPO" "$DEST"
fi

cd "$DEST"

# The installer wants a terminal for the password prompt. When this script is
# itself piped, hand it the real terminal if there is one.
if [ ! -t 0 ] && [ -e /dev/tty ]; then
    exec sh scripts/install.sh < /dev/tty
else
    exec sh scripts/install.sh
fi
