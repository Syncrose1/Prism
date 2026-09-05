# ADR 0003: Terminal sessions

**Date:** 2026-09-05
**Status:** Accepted
**Relates to:** [ADR 0001](./0001-shell-and-file-architecture.md), [ADR 0002](./0002-critical-functions-mode.md)

---

## Context

*Operator requirement, 2026-09-05: "be able to forward the actual terminals.
Create a terminal, interact with it, killing it in PrismOS should kill it on the
actual machine… I could add my ComfyUI launch script as a Quick Launch, or just
open a terminal, `cd ComfyUI`, `./run_nvidia_gpu.sh` as I usually do. It's an
interactive script so I need to press Enter twice, and it runs. I can kill it
too."*

That last detail is the load-bearing one. `run_nvidia_gpu.sh` **prompts**. A
pipe cannot answer a prompt — this needs a real PTY, not captured stdout.

This is also the single highest-privilege surface in the product. A terminal is
unrestricted shell access to the machine, which deserves saying plainly rather
than treating as one more app.

## Decisions

### 1. Sessions detach; they do not die with the window

Closing a terminal window **detaches**. The process keeps running. Reopening
reattaches, with scrollback intact. Only an explicit *Kill* destroys it.

The naive reading of the requirement — "closing it in PrismOS kills it on the
machine" — is right for `Kill` and wrong for `Close`. The operator's own example
proves it: launching ComfyUI from a terminal and then closing the window must not
kill ComfyUI. A browser tab crash, a phone locking, or a train entering a tunnel
would otherwise take down the workload.

So: `Close` = detach, `Kill` = `cgroup.kill`. Two visibly different controls,
never merged into one.

### 2. Sessions run in cgroup scopes, exactly like facets

The PTY child is `systemd-run --user --scope --unit=prism-term-<id>`, wrapped in
a `forkpty`. That yields a real terminal *and* a real cgroup from one mechanism.

This matters more here than anywhere else in Prism. **A terminal is precisely how
somebody accidentally runs the thing that eats the machine** — it is the least
supervised, most arbitrary code path in the system. Putting sessions in scopes
means memory limits, per-session accounting, governor attribution and atomic
whole-tree kill all apply to interactive work for free, rather than terminals
being the one hole in an otherwise contained design.

### 3. Quick Launch is a facet with `pty = true`, not a parallel system

The operator wants to save `cd ComfyUI && ./run_nvidia_gpu.sh` as a one-click
launcher. That is a facet — a registered command Prism owns — that happens to
need a terminal because it prompts.

So `Facet` gains `pty: bool`. When set, starting the facet allocates a PTY and
its output is viewable in a Terminal window. Stopping it is the same
`cgroup.kill` as any other facet.

One concept rather than two. It also means an interactive workload gets the same
limits, attribution and storm protection as a headless one, which the alternative
design would have quietly lost.

### 4. Terminal requires the `Fresh` tier, and is disableable entirely

Files needing a recent authenticator code while a shell did not would be
incoherent. Terminal is `Sensitivity::Fresh`, and `HostConfig` carries
`terminal.enabled` so an operator deploying Prism to a machine where remote shell
access is unacceptable can turn the feature off at the config layer rather than
relying on nobody navigating to it.

**Honest note on the safety guard.** `safety::SafetyGuard` prevents *Prism* from
signalling init, sshd, tailscaled or the compositor. It cannot prevent an
operator typing `kill -9` in a shell, and should not try. The guard exists to
stop Prism making the 2026-09-04 mistake again — it was never a sandbox, and a
terminal makes that boundary explicit rather than changing it.

### 5. Vendor a terminal emulator; do not write one

Interactive scripts, `vim`, `htop` and progress bars need real VT100/xterm
handling: escape sequences, cursor addressing, alternate screen, colour, resize.
Hand-rolling that is a large project with a long tail of subtle breakage, and the
failure mode is a terminal that looks fine until the operator runs the one thing
they actually needed.

`xterm.js` is vendored as a pre-built file and embedded. This does not
reintroduce the npm toolchain ADR 0001 rejected — the artefact is committed, and
there is still no build step.

### 6. Scrollback is bounded

Each detached session retains a ring buffer for reattach. **Bounded, default
256 KiB.** A chatty process filling unbounded scrollback would be a memory
incident caused by the memory-incident daemon, which is a failure this project
has already had once and does not need again.

At Red tier, scrollback retention drops and new session creation is refused —
opening a shell on a machine that is already failing adds load at the worst
moment. Existing sessions stay attached; the rescue page remains the way to act.

## Shape

```
POST   /api/term            create a session      → { id }
GET    /api/term            list sessions
WS     /api/term/:id/attach bidirectional stream
POST   /api/term/:id/resize rows/cols
POST   /api/term/:id/kill   cgroup.kill
```

```rust
struct Session {
    id: String,
    unit: String,          // prism-term-<id>.scope
    master: OwnedFd,       // pty master
    child: Pid,
    scrollback: RingBuffer,
    created: SystemTime,
    title: String,         // last command, for the window title
}
```

## Consequences

- Prism becomes a remote workstation rather than a monitoring tool. This is the
  feature that makes it worth reaching for when nothing is wrong.
- It is also the largest attack surface added so far, and the reason `Fresh`
  exists as a tier at all.
- Session lifecycle is now independent of window lifecycle, which the shell must
  represent honestly: a detached session is visible in the dock and the palette,
  not invisible until rediscovered.
- Facets and terminals converge, which simplifies the model and means Quick
  Launch needs no new machinery.
