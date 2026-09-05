#!/usr/bin/env bash
# Install and set up Prism for the current user, in one command.
#
#   ./scripts/install.sh
#
# Builds, installs, detects what this machine has, enrols an authenticator,
# sets a password, and starts the service. Re-running is safe: it rebuilds and
# restarts, and never overwrites configuration or credentials.
#
# Everything runs as a systemd *user* service. No root is required for Prism
# itself — the memory controller is delegated to the user slice, so facets and
# terminals still get real cgroup containment. One optional step at the end
# needs root once; it is offered, not assumed.

set -euo pipefail
cd "$(dirname "$0")/.."

BIN_DIR="${PRISM_BIN_DIR:-$HOME/.local/bin}"
UNIT_DIR="$HOME/.config/systemd/user"
CONFIG_DIR="${PRISM_CONFIG_DIR:-${XDG_CONFIG_HOME:-$HOME/.config}/prism}"
STATE_DIR="${PRISM_STATE_DIR:-${XDG_STATE_HOME:-$HOME/.local/state}/prism}"

step()  { printf '\n\033[1m%s\033[0m\n' "$*"; }
note()  { printf '  %s\n' "$*"; }
ok()    { printf '  \033[32m%s\033[0m\n' "$*"; }
warn()  { printf '  \033[33m%s\033[0m\n' "$*"; }

# ── requirements ──────────────────────────────────────────────────────────
step "Checking requirements"
missing=0
if ! command -v cargo >/dev/null; then
    warn "cargo not found — install Rust from https://rustup.rs"
    missing=1
fi
if ! systemctl --user show-environment >/dev/null 2>&1; then
    warn "no systemd user manager — Prism needs one for service management"
    missing=1
fi
[ "$missing" -eq 0 ] || exit 1
ok "cargo and systemd present"

# Optional, and only worth mentioning because their absence degrades a feature
# rather than breaking anything.
for tool in vips ffmpeg pdftoppm qrencode; do
    command -v "$tool" >/dev/null || note "optional: $tool not found"
done

# ── build and install ─────────────────────────────────────────────────────
step "Building"
cargo build --release --quiet
ok "built"

step "Installing"
mkdir -p "$BIN_DIR" "$UNIT_DIR"
# Replacing a running binary in place fails with ETXTBSY; a rename is atomic.
install -m 755 target/release/prismd "$BIN_DIR/.prismd.new"
mv -f "$BIN_DIR/.prismd.new" "$BIN_DIR/prismd"
install -m 644 systemd/prismd.service "$UNIT_DIR/prismd.service"
ok "$BIN_DIR/prismd"

if ! printf '%s' "$PATH" | tr ':' '\n' | grep -qx "$BIN_DIR"; then
    warn "$BIN_DIR is not on your PATH"
    case "$(basename "${SHELL:-}")" in
        fish) note "fix: fish_add_path $BIN_DIR" ;;
        zsh)  note "fix: echo 'export PATH=\"$BIN_DIR:\$PATH\"' >> ~/.zshrc" ;;
        *)    note "fix: echo 'export PATH=\"$BIN_DIR:\$PATH\"' >> ~/.bashrc" ;;
    esac
fi

# ── configuration ─────────────────────────────────────────────────────────
step "Configuring"
if [ -f "$CONFIG_DIR/prism.toml" ]; then
    ok "existing configuration kept ($CONFIG_DIR)"
else
    "$BIN_DIR/prismd" setup | sed 's/^/  /'
fi

# ── credentials ───────────────────────────────────────────────────────────
# Enrolment happens on first daemon start, which prints a QR. Starting the
# service first means that lands in the journal rather than on screen, so it is
# done here where the operator is looking.
step "Authenticator"
if [ -f "$STATE_DIR/totp.secret" ]; then
    ok "already enrolled ($STATE_DIR/totp.secret)"
    note "to replace it: prismd enrol --reset"
else
    # A short foreground run performs enrolment and prints the QR.
    timeout 5 "$BIN_DIR/prismd" >/tmp/prism-enrol.$$ 2>&1 || true
    sed -n '/PRISM ENROLMENT/,/────────────────$/p' /tmp/prism-enrol.$$ \
        | sed 's/^[0-9T:.Z-]* *INFO *//' || true
    rm -f /tmp/prism-enrol.$$
fi

step "Password"
if [ -f "$STATE_DIR/password.hash" ]; then
    ok "already set"
    note "to change it: prismd passwd"
elif [ -t 0 ]; then
    note "Sets a quick unlock, so only the first sign-in on a device needs the code."
    note "Press Ctrl-C to skip; you can run 'prismd passwd' later."
    "$BIN_DIR/prismd" passwd || warn "skipped — every sign-in will need a code"
else
    note "not a terminal; run 'prismd passwd' when you can"
fi

# ── service ───────────────────────────────────────────────────────────────
step "Starting"
systemctl --user daemon-reload
systemctl --user enable prismd.service >/dev/null 2>&1

# Without lingering the user manager stops at logout, taking Prism with it —
# a watchdog that dies with the session is not a watchdog.
if ! loginctl show-user "$USER" -p Linger --value 2>/dev/null | grep -q yes; then
    loginctl enable-linger "$USER" 2>/dev/null \
        || note "run later so Prism survives logout: sudo loginctl enable-linger $USER"
fi

systemctl --user restart prismd.service
sleep 2

if ! systemctl --user is-active --quiet prismd.service; then
    warn "failed to start:"
    journalctl --user -u prismd -n 20 --no-pager | sed 's/^/    /'
    exit 1
fi

URL=$(journalctl --user -u prismd -n 200 --no-pager 2>/dev/null \
      | grep -o 'http://[0-9.]*:[0-9]*/' | tail -1)
ok "running${URL:+ at ${URL}}"

if ! journalctl --user -u prismd -n 50 --no-pager 2>/dev/null | grep -q "memory locked"; then
    warn "prismd cannot lock its memory, so it can be paged out under the"
    warn "pressure it exists to resolve. One root command fixes it:"
    note ""
    note "  sudo install -Dm644 systemd/50-prism-memlock.conf \\"
    note "       /etc/systemd/user.conf.d/50-prism-memlock.conf"
    note ""
    note "then log out and back in. Prism works without it."
fi

# ── done ──────────────────────────────────────────────────────────────────
cat <<EOF

$(printf '\033[1mReady\033[0m')

  Open ${URL:-http://localhost:9000/} and sign in with a code from your
  authenticator. After that this device only needs the password.

  prismd setup      show what was detected
  prismd passwd     change the password
  prismd enrol      enrolment status

  systemctl --user status prismd
  journalctl --user -u prismd -f
EOF
