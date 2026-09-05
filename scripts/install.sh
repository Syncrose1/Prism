#!/usr/bin/env bash
# Install Prism for the current user.
#
# Everything lives in the user's own directories and runs as a systemd *user*
# service: no root, no system unit, and nothing that needs a password. The
# memory controller is delegated to the user slice on a modern systemd, so
# facets and terminals still get real cgroup containment.
#
# Re-running is safe; it rebuilds, replaces the binary and restarts.

set -euo pipefail
cd "$(dirname "$0")/.."

BIN_DIR="${PRISM_BIN_DIR:-$HOME/.local/bin}"
UNIT_DIR="$HOME/.config/systemd/user"

say() { printf '\n\033[1m%s\033[0m\n' "$1"; }

say "Building (release)"
cargo build --release

say "Installing"
mkdir -p "$BIN_DIR" "$UNIT_DIR"
# Install to a temporary name and rename: replacing a running binary in place
# fails with ETXTBSY, and a rename is atomic.
install -m 755 target/release/prismd "$BIN_DIR/.prismd.new"
mv -f "$BIN_DIR/.prismd.new" "$BIN_DIR/prismd"
echo "  $BIN_DIR/prismd"

install -m 644 systemd/prismd.service "$UNIT_DIR/prismd.service"
echo "  $UNIT_DIR/prismd.service"

if ! printf '%s' "$PATH" | tr ':' '\n' | grep -qx "$BIN_DIR"; then
    echo
    echo "  note: $BIN_DIR is not on your PATH."
    echo "        fish:  fish_add_path $BIN_DIR"
    echo "        bash:  echo 'export PATH=\"$BIN_DIR:\$PATH\"' >> ~/.bashrc"
fi

say "Enabling the service"
systemctl --user daemon-reload
systemctl --user enable prismd.service >/dev/null

# Without lingering the user manager stops at logout, taking Prism with it —
# which would mean the watchdog dies exactly when the session does.
if ! loginctl show-user "$USER" -p Linger --value 2>/dev/null | grep -q yes; then
    echo "  requesting linger so Prism survives logout (may prompt)"
    loginctl enable-linger "$USER" 2>/dev/null \
        || echo "  could not enable linger; run: sudo loginctl enable-linger $USER"
fi

systemctl --user restart prismd.service
sleep 2

say "Status"
if systemctl --user is-active --quiet prismd.service; then
    ADDR=$(systemctl --user show prismd -p MainPID --value | xargs -I{} sh -c \
        'journalctl --user -u prismd -n 200 --no-pager 2>/dev/null | grep -o "http://[0-9.]*:[0-9]*/" | tail -1' || true)
    echo "  running${ADDR:+ at $ADDR}"
    if journalctl --user -u prismd -n 50 --no-pager 2>/dev/null | grep -q "memory locked"; then
        echo "  memory locked — prismd cannot be paged out"
    else
        # Not fatal, but worth being explicit about: the daemon runs, it can
        # just be swapped out at the moment it is most needed.
        cat <<'NOTE'
  note: prismd could not lock its memory, so it can be paged out under the
        pressure it exists to resolve. The default limit is 8 MiB and is checked
        against virtual size, which any threaded binary exceeds.

        One-off fix, needs root once:
          sudo install -Dm644 systemd/50-prism-memlock.conf \
               /etc/systemd/user.conf.d/50-prism-memlock.conf
        Then log out and back in, or reboot.
NOTE
    fi
else
    echo "  failed to start. Recent log:"
    journalctl --user -u prismd -n 20 --no-pager | sed 's/^/    /'
    exit 1
fi

cat <<EOF

Next steps
  prismd passwd            set the quick-unlock password
  prismd enrol             enrolment status
  prismd enrol --reset     revoke and replace the authenticator secret

  systemctl --user status prismd
  journalctl --user -u prismd -f
EOF
