#!/usr/bin/env bash
# Run Prism's test suite inside a PID namespace.
#
# Prism is a daemon that terminates processes, and its tests exercise that code.
# On 2026-09-04 `cargo test` in the operator's live graphical session sent
# SIGTERM to every process owned by uid 1000 — four times — because a test passed
# `u32::MAX` as "an impossible pid" and it wrapped to -1 through a pid_t cast.
#
# `crates/prism-core/src/safety.rs` now makes that specific mistake impossible.
# This script exists because that is the wrong place to rely on: a test suite for
# a killer daemon should have its blast radius bounded by the kernel, not by the
# correctness of the code under test. Inside a PID namespace, even a literal
# kill(-1) reaches only the namespace's own processes.
#
# Usage:  ./scripts/test-isolated.sh [cargo test args...]

set -euo pipefail

cd "$(dirname "$0")/.."

if ! unshare --user --pid --fork --mount-proc true 2>/dev/null; then
    echo "ERROR: unprivileged PID namespaces are unavailable on this host." >&2
    echo "Refusing to run signal-sending tests unisolated. Check:" >&2
    echo "  sysctl kernel.unprivileged_userns_clone   (want 1)" >&2
    echo "  sysctl user.max_user_namespaces           (want > 0)" >&2
    exit 1
fi

# Syntax-check the shell before the Rust tests. A duplicate `const` in an
# inline script kills the entire IIFE silently — the page renders, nothing
# works, and no Rust test can see it. Caught exactly that on 2026-09-05.
if command -v node >/dev/null 2>&1; then
    python3 - <<'EOF' > /tmp/prism-shell-check.js
s = open("ui/shell.html").read()
i = s.rindex("<script>"); j = s.rindex("</script>")
print(s[i + 8:j])
EOF
    if node --check /tmp/prism-shell-check.js; then
        echo "ui/shell.html: syntax OK"
    else
        echo "ui/shell.html: SYNTAX ERROR — refusing to run tests" >&2
        exit 1
    fi
    rm -f /tmp/prism-shell-check.js

    # Syntax alone is not enough. Deleting a function while removing a
    # neighbouring one left the shell parsing cleanly and throwing
    # "appTerminal is not defined" at boot — page rendered, nothing worked.
    # Check that every app the registry names is actually defined.
    python3 - <<'EOF' || exit 1
import re, sys
src = open("ui/shell.html").read()
registry = re.search(r"const APPS = \{(.*?)\n\};", src, re.S)
if not registry:
    print("could not find the APPS registry", file=sys.stderr); sys.exit(1)
referenced = set(re.findall(r"\b(app[A-Z]\w*)\b", registry.group(1)))
defined = set(re.findall(r"function\s+(app[A-Z]\w*)\s*\(", src))
missing = sorted(referenced - defined)
if missing:
    print("ui/shell.html: APPS references undefined functions: %s" % ", ".join(missing), file=sys.stderr)
    sys.exit(1)
print("ui/shell.html: all %d registered apps are defined" % len(referenced))
EOF
else
    echo "note: node not found, skipping shell syntax check" >&2
fi

echo "running tests in an isolated PID namespace"
exec unshare --user --pid --fork --mount-proc cargo test "$@"
