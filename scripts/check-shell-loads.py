#!/usr/bin/env python3
"""Load the shell in a real browser and fail on any uncaught exception.

Syntax checking cannot catch the bug this exists for. The shell is one script
in source order, and a block that runs at load time while referencing a `const`
declared further down parses perfectly and throws `ReferenceError` the moment a
browser reaches it — leaving a page that renders and does nothing. That has
happened twice: once when a helper moved above its dependencies, and once when
a new panel was inserted before `$` was defined.

So the check is empirical. Serve the file, open it, wait, and report anything
that was thrown. The page's API calls all fail here, which is fine and in fact
useful: it exercises the paths that run before a session exists, which is also
what a browser hitting a stopped daemon would do.

Skipped, not failed, when Chromium is absent — a contributor without it should
still be able to run the suite.
"""
import base64
import http.server
import json
import os
import secrets
import shutil
import socket
import struct
import subprocess
import sys
import tempfile
import threading
import time
import urllib.request

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SHELL = os.path.join(ROOT, "ui")
CDP_PORT = 9412
HTTP_PORT = 8912


def find_chromium():
    for name in ("chromium", "chromium-browser", "google-chrome", "google-chrome-stable"):
        path = shutil.which(name)
        if path:
            return path
    return None


class Quiet(http.server.SimpleHTTPRequestHandler):
    def __init__(self, *a, **kw):
        super().__init__(*a, directory=SHELL, **kw)

    def log_message(self, *a):
        pass


class WS:
    """Just enough of RFC 6455 to speak CDP without adding a dependency."""

    def __init__(self, url):
        parts = url.split("/", 3)
        host, port = parts[2].split(":")
        self.sock = socket.create_connection((host, int(port)), timeout=30)
        key = base64.b64encode(secrets.token_bytes(16)).decode()
        self.sock.send(
            f"GET /{parts[3]} HTTP/1.1\r\nHost: {host}:{port}\r\n"
            f"Upgrade: websocket\r\nConnection: Upgrade\r\n"
            f"Sec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n".encode()
        )
        buf = b""
        while b"\r\n\r\n" not in buf:
            buf += self.sock.recv(4096)
        self.buf = buf.split(b"\r\n\r\n", 1)[1]

    def send(self, obj):
        data = json.dumps(obj).encode()
        head = bytearray([0x81])
        mask = secrets.token_bytes(4)
        n = len(data)
        if n < 126:
            head.append(0x80 | n)
        elif n < 65536:
            head.append(0x80 | 126)
            head += struct.pack(">H", n)
        else:
            head.append(0x80 | 127)
            head += struct.pack(">Q", n)
        head += mask
        head += bytes(c ^ mask[i % 4] for i, c in enumerate(data))
        self.sock.send(bytes(head))

    def _read(self, n):
        while len(self.buf) < n:
            self.buf += self.sock.recv(65536)
        out, self.buf = self.buf[:n], self.buf[n:]
        return out

    def recv(self):
        head = self._read(2)
        length = head[1] & 127
        if length == 126:
            length = struct.unpack(">H", self._read(2))[0]
        elif length == 127:
            length = struct.unpack(">Q", self._read(8))[0]
        return json.loads(self._read(length))


def main():
    chromium = find_chromium()
    if not chromium:
        print("note: chromium not found, skipping shell load check", file=sys.stderr)
        return 0
    if not os.path.exists(os.path.join(SHELL, "shell.html")):
        print("ui/shell.html not found", file=sys.stderr)
        return 1

    server = http.server.ThreadingHTTPServer(("127.0.0.1", HTTP_PORT), Quiet)
    threading.Thread(target=server.serve_forever, daemon=True).start()

    profile = tempfile.mkdtemp(prefix="prism-shellcheck-")
    browser = subprocess.Popen(
        [chromium, "--headless=new", f"--remote-debugging-port={CDP_PORT}",
         "--no-sandbox", "--disable-gpu", f"--user-data-dir={profile}", "about:blank"],
        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
    )

    try:
        tabs = None
        for _ in range(80):
            try:
                tabs = json.load(
                    urllib.request.urlopen(f"http://127.0.0.1:{CDP_PORT}/json/list", timeout=2)
                )
                break
            except Exception:
                time.sleep(0.25)
        if not tabs:
            print("note: chromium did not start, skipping shell load check", file=sys.stderr)
            return 0

        ws = WS(next(t for t in tabs if t["type"] == "page")["webSocketDebuggerUrl"])
        errors = []
        counter = [0]

        def command(method, params=None):
            counter[0] += 1
            ident = counter[0]
            ws.send({"id": ident, "method": method, "params": params or {}})
            while True:
                msg = ws.recv()
                if msg.get("method") == "Runtime.exceptionThrown":
                    d = msg["params"]["exceptionDetails"]
                    errors.append(
                        d.get("exception", {}).get("description") or d.get("text", "?")
                    )
                if msg.get("id") == ident:
                    return msg

        command("Runtime.enable")
        command("Page.enable")
        command("Page.navigate", {"url": f"http://127.0.0.1:{HTTP_PORT}/shell.html"})
        # Long enough for boot() to run to the point where it needs a session.
        deadline = time.time() + 6
        while time.time() < deadline:
            command("Runtime.evaluate", {"expression": "1", "returnByValue": True})
            time.sleep(0.4)

        # An element carrying `hidden` that is still displayed. A UA rule
        # hides it at specificity (0,1,0), which any id selector setting
        # `display` outranks — so the attribute silently stops working and
        # every close button that sets it appears dead. Cheap to check, and it
        # is invisible to both syntax checking and exception reporting.
        stuck = command("Runtime.evaluate", {
            "expression": (
                "JSON.stringify([...document.querySelectorAll('[hidden]')]"
                ".filter(e=>getComputedStyle(e).display!=='none')"
                ".map(e=>e.id||e.className||e.tagName))"
            ),
            "returnByValue": True,
        })["result"]["result"].get("value")
        if stuck and stuck != "[]":
            print(f"ui/shell.html: [hidden] elements are still displayed: {stuck}",
                  file=sys.stderr)
            print("  a `display` rule outranks the UA [hidden] rule; "
                  "restate it as `#id[hidden] { display: none }`", file=sys.stderr)
            return 1

        # Anything that failed to *fetch* is expected: there is no daemon here.
        real = [e for e in errors if "Failed to fetch" not in e and "NetworkError" not in e]
        if real:
            print("ui/shell.html threw during load:", file=sys.stderr)
            for e in real[:10]:
                print("  " + e.splitlines()[0], file=sys.stderr)
            return 1
        print("ui/shell.html: loads in a browser with no uncaught exceptions")
        return 0
    finally:
        browser.terminate()
        server.shutdown()
        shutil.rmtree(profile, ignore_errors=True)


if __name__ == "__main__":
    sys.exit(main())
