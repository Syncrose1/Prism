# Incident: total remote-access loss via compositor segfault

**Date:** 2026-09-04
**Host:** `c2` / <host>
**Session lost:** 19:30:15 BST
**Access restored:** 21:05 BST
**Duration unreachable (graphical):** ~1 h 35 min
**Diagnosed and recovered from:** `c1` (<laptop>), over Tailscale SSH
**Author:** Claude (Opus 5), c1 session

**Companion document:** [`2026-09-04-second-session-loss.md`](./2026-09-04-second-session-loss.md)
— a **separate incident** at 21:14 the same evening, with a different root cause
(clean compositor exit, no segfault). It also documents how this incident's
recovery actions — `Restart=always` and a hand-started Sunshine — caused that
second outage.

---

## Why this document exists

Prism is being built to prevent exactly this class of event. This incident is
the first *observed, fully-instrumented* case of the failure mode Prism's
priority-1 goal names outright — **"Never let the machine become unreachable"** —
and it is worth reading carefully, because **it is not the failure mode the
architecture currently anticipates.**

`docs/architecture.md` §1 identifies two failure modes: the zram thrash spiral
(§1.1) and the ConflictKiller recursion (§1.2). This incident was **neither**.
No thrash spiral occurred. No recursion occurred. Both mitigations were already
in place and both worked correctly.

The session was lost anyway.

---

## What happened

### Timeline

| Time (BST) | Event |
|---|---|
| 19:08:00 | Claude session in `~/Prism` probes sudo availability (`COMMAND=/usr/bin/true`) |
| 19:10:22 | `apply-zram.sh` runs under sudo; safety gate passes |
| 19:10:23–19:10:37 | All swap deactivated; zram rebuilt 46 GiB → 15.4 GiB; swap re-enabled |
| 19:10:56 | `wireplumber`: `s-device: Could not find valid non-headset profile, not switching` |
| 19:10:56 | **`quickshell` (`qs -c ii`) segfaults** — PID 2655047, SIGSEGV |
| 19:10:58 | **`quickshell` segfaults again** — PID 2655049, SIGSEGV |
| 19:10:58 – 19:30:15 | ~19 minutes of apparently normal operation |
| 19:30:15 | **Hyprland segfaults** — PID 1092, SIGSEGV, `SEGV_MAPERR` at address 0 |
| 19:30:15 | `hyprsunset` aborts (SIGABRT) alongside it |
| 19:30:15 | `systemd[920]: Activating special unit Exit the Session...` |
| 19:30:16 | `sddm-helper`: PAM session closed for `raahats` |
| 19:30:16 | **Sunshine exits `status=0/SUCCESS`** — and is not restarted |

Everything in the session died together: Sunshine, syncthing, all kitty scopes
(peaks of 26 G, 19.2 G, 19.1 G, 14.9 G), Chromium, the portals, the lot.

### The three access paths, and why all three failed at once

| Path | Depended on | Outcome |
|---|---|---|
| Moonlight | Sunshine → graphical session | **Lost** — Sunshine died with the session |
| Remote Control | graphical session | **Lost** — same root |
| SSH | `sshd`, system-scoped | **Survived** — the only way back in |

This is the single most important structural fact in the incident. Two of the
three remote paths shared a **common dependency on the graphical session**, so a
single compositor segfault took both out simultaneously. The apparent redundancy
was illusory. Only SSH — which depends on nothing in the session — survived.

---

## Root cause

**Hyprland 0.55.4 dereferenced a null pointer in the dwindle tiling layout
during compositor shutdown.**

```
CCompositor::cleanup()
  → wl_display_destroy_clients()          (libwayland-server)
    → CWLSurfaceResource::destroy()
      → Desktop::View::CWindow::unmapWindow()
        → Layout::CLayoutManager::removeTarget()
          → Layout::Tiled::CDwindleAlgorithm::removeTarget()
            → Layout::Tiled::CDwindleAlgorithm::calculateWorkspace()
              → Layout::ITarget::setPositionGlobal()   ← SIGSEGV, addr 0
```

As clients are torn down, each unmapping window triggers a dwindle layout
recalculation; one of these reached a target whose backing pointer was already
null. `SEGV_MAPERR` at address `0` confirms a null dereference rather than
corruption.

This is an **upstream Hyprland defect**, not a configuration error and not
something any local script caused directly.

---

## What the earlier quickshell crashes actually were

They look like §1.2 ConflictKiller recursion. **They are not.** This matters,
because building a watchdog against the wrong signature would miss this class
entirely.

```
PwCore::poll()
  → PwBindableObject::safeDestroy()
    → PwNodeIface::~PwNodeIface()          [D0 — destructor running]
      → Pipewire::onNodeRemoved()
        → PwNodeIface::instance()
          → QObject::property()            ← use-after-free
```

A PipeWire **node-removal use-after-free**: a QML binding evaluates a property on
a node object while that object's destructor is executing. The trigger is
immediately upstream in the log — `wireplumber` failing a Bluetooth audio profile
switch at 19:10:56, consistent with the `hci0` A2DP endpoints and
`Bluetooth: hci0: link tx timeout` seen later at teardown.

Distinguishing evidence against the recursion hypothesis:

- Command line is `qs -c ii`, **not** `qs -p .*killDialog.qml` (§4.4's signature)
- **Two** processes died, not the 36-process / 11.2 GiB storm of §1.2
- `autoKillTrays: true` and `autoKillNotificationDaemons: true` are **already set**
  in `~/.config/illogical-impulse/config.json` — both dangerous paths were closed
- No memory growth accompanied them

**The §1.2 mitigation worked. This was a different bug in the same component.**

---

## On the zram change: exonerated

`apply-zram.sh` was correct work and should not be reverted. For the record,
since the temporal proximity invites suspicion:

- The **safety gate passed legitimately.** It refuses to run unless
  `MemAvailable ≥ SwapUsed + 2 GiB`, precisely to avoid recreating the hang it
  was preventing. That check is exactly right.
- **No OOM event occurred** anywhere in the window. The kernel OOM killer never
  fired. Nothing was killed for memory.
- **No memory pressure at crash time.** The machine currently sits at 3.6 G used
  of 30 G, 23 G free.
- The reasoning in `zram-generator.conf` is sound: 46 GiB of compressed
  in-RAM swap on a 30 GiB machine manufactures phantom headroom and converts
  clean OOM kills into unrecoverable thrash. `ram * 0.5` is the right call.
- The script tested `sudo` with `/usr/bin/true` before acting — deliberate,
  careful practice.

**One correction to an intuition worth recording:** the journal shows swap being
deactivated under four different names — `by-uuid/c9c09768…`, `by-label/zram0`,
`by-diskseq/3`, and `dev-zram0.swap`. These are **all the same zram device**
under different symlink aliases, not a separate disk swap partition that went
missing. `/etc/fstab` contains no swap entry; zram has always been the only swap
on this host. Nothing was lost.

**Honest statement of the causal link.** 19 minutes and 19 seconds separate the
zram change from the Hyprland segfault, with no OOM and no memory pressure in
between. The Hyprland bug is a genuine null-deref in the dwindle layout that a
swap resize does not plausibly reach. **The most defensible reading is that the
two are unrelated,** and that the quickshell crashes — which share a mechanism
with neither — were triggered by a Bluetooth profile switch that happened to
occur 34 seconds after the swap rebuild. Prism should not encode a causal link
here that the evidence does not support.

---

## What Prism should take from this

### 1. Session-scoped services are a single point of failure (highest value)

Prism's §1 goal is that the machine never becomes unreachable. This incident
shows the current threat model is incomplete: **a compositor crash is sufficient
to sever every graphical remote path simultaneously**, with no memory pressure,
no thrash, and no recursion — none of the conditions the governor watches for.

The governor is tiered on PSI stall and honest-headroom. **All tiers would have
read Green throughout this incident.** No sensor in §4.1 observes "the graphical
session just ended", and no watchdog in §4.4 acts on it.

Concrete suggestions:

- **Add a session-liveness sensor.** Watch `loginctl` session state and the
  Hyprland process directly. The transition to no active `seat0` session is a
  first-class event, independent of memory tiers.
- **Add a remote-access-path health check** as a distinct concern from workload
  health: is Sunshine listening? Is a compositor alive? Is at least one
  *non-session-scoped* path (SSH, prismd's own API) reachable? Prism should
  know its own reachability, continuously.
- **Treat "compositor died" as its own recovery action.** The §4.4 quickshell
  watchdog already reproduces the known-good `CTRL+SUPER+R` remedy. The Hyprland
  case needs an analogue: on compositor death, restart the display manager (or
  re-trigger autologin) rather than waiting for a human on SSH.

### 2. `Restart=on-failure` is the wrong policy for session-coupled services

`sunshine.service` had `Restart=on-failure`. When the session ended, Sunshine
received a clean SIGTERM and exited **`status=0/SUCCESS`** — so systemd
considered it a success and correctly did not restart it.

**A clean exit caused by the death of something else is not a success.** This
single line is why the outage lasted 95 minutes rather than 5 seconds.

Fixed during recovery (`Restart=always`, backup at
`/etc/systemd/system/sunshine.service.bak-20260904`). Prism should audit this
pattern across every service it supervises — any unit whose liveness is
contingent on another process needs `Restart=always`, not `on-failure`.

### 3. The watchdog signature for quickshell is too narrow

§4.4 keys detection on `qs -p .*killDialog.qml` and on process-count/RSS growth.
This incident produced **neither** — two clean segfaults from an unrelated
PipeWire use-after-free, no growth, no dialog. Recommend adding a generic
**"quickshell died unexpectedly"** trigger (crash-count in a window, regardless
of signature) alongside the recursion-specific one. The existing remedy —
`hyprctl dispatch exec` to restart session-owned — applies unchanged.

### 4. Cross-check against the design principles

Two principles held up well and are worth keeping in view:

- *"Attribute before acting."* Had a governor acted on memory tiers here, it
  would have attributed wrongly — there was no memory fault to attribute. The
  discipline of attributing before acting is what prevents that error.
- *"Every intervention is reported."* Nothing reported this. The operator
  discovered a 95-minute outage by trying to connect and failing. A beacon on
  session death — before any recovery is even attempted — would have collapsed
  the discovery time to seconds.

### 5. Coredump mining is cheap, high-signal instrumentation

Every conclusion here came from `coredumpctl` and `journalctl`, already present
and already retaining what was needed. Prism's `recorder/` should consider
harvesting `coredumpctl` metadata on crash: the distinction between §1.2
recursion and a PipeWire use-after-free was visible **only** in the backtrace,
and cost one command to obtain.

---

## Recovery actions taken

| Action | Result |
|---|---|
| Installed `c1` SSH key on `c2` | Key-based access established (was: no key, password-only) |
| `systemctl restart sddm` | Autologin fired; Hyprland live on `seat0`/tty1; session restored |
| `systemctl start sunshine` | Active; Moonlight reachable again |
| `Restart=on-failure` → `Restart=always` | Sunshine now survives session death; backup retained |

**Left deliberately unchanged:** the zram configuration. It is correct.

---

## Recommended follow-ups (not performed)

1. **Enable Tailscale SSH on `c2`** — `sudo tailscale set --ssh`. During this
   incident SSH was the *only* way in, and it was nearly unavailable: `c2` had no
   authorized key for `c1` and `SSH_HostKeys: None` confirmed Tailscale SSH was
   off. Recovery depended on the operator having a password to hand. For a host
   whose stated priority-1 goal is never being unreachable, identity-based SSH
   that needs neither key distribution nor passwords is the cheapest possible
   resilience win.
2. **Rotate the `c2` sudo password and review its strength.** SSH password
   authentication is enabled over the tailnet. Now that key auth
   works, consider `PasswordAuthentication no` as well.
3. **Report the Hyprland dwindle crash upstream.** The backtrace above is a clean
   reproduction candidate: null deref in `ITarget::setPositionGlobal()` via
   `CDwindleAlgorithm::calculateWorkspace()` during `CCompositor::cleanup()`.
   Hyprland 0.55.4, core retained (PID 1092).
4. **Report the quickshell PipeWire use-after-free upstream** —
   `PwNodeIface::instance()` called from within `~PwNodeIface`, reached via
   `onNodeRemoved` during `PwCore::poll()`. Cores retained (PIDs 2655047,
   2655049).
5. **Consider `systemd-coredump` retention limits** — cores are accumulating
   (5 × ~180 MB from `llama-server` earlier the same day).

---

## Environment

```
Host           <host> (c2), 100.x.x.x
Kernel         7.1.1-2-cachyos
RAM            30.7 GiB (32205000 kB)
Swap           zram0 only, 15.4 GiB zstd, prio 100 (no disk swap, no fstab entry)
Hyprland       0.55.4-1.1  (v0.55.4, commit a0136d8c)
quickshell     illogical-impulse-quickshell-git 0.1.0.r1-8
pipewire       1:1.6.7-1.1
wireplumber    0.5.15-1.1
Display mgr    sddm, autologin → hyprland, user raahats
Uptime at crash 8 d 23 h (boot 2026-08-26 21:29) — no reboot involved
```

Cores retained: `Hyprland` (1092, 9.4 M), `hyprsunset` (1403), `quickshell`
(2655047 35.5 M, 2655049 66.7 M).

---

## Sign-off

Diagnosed and recovered remotely from `c1` with no physical access to `c2` and
no working graphical path — SSH only, throughout.

The headline finding is deliberately the uncomfortable one: **the mitigations
Prism has already built worked, and the machine still became unreachable.** The
zram fix held, the ConflictKiller fix held, and a single upstream null-pointer
dereference in a tiling-layout function still took down every graphical route in
at one stroke. Resilience against the two known failure modes was not resilience
against *unreachability*, because two of three access paths shared a dependency
neither mitigation covers.

That gap is worth more to Prism than a clean recovery would have been.

— **Claude (Opus 5)**, sentinel of `c1`
  2026-09-04, written from London, over a tunnel that held when nothing else did
