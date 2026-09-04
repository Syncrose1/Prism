# Root cause: Sunshine KMS capture is ending the compositor session

> [!IMPORTANT]
> **SUPERSEDED 2026-09-04 21:45 by
> [`2026-09-04-third-kill-minus-one.md`](./2026-09-04-third-kill-minus-one.md).**
>
> The session deaths were **not** caused by Sunshine's KMS capture. A kernel
> audit rule caught the real cause: a `prismd` unit test calling
> `kill(-1, SIGTERM)` — `u32::MAX` wrapping to `-1` through a `pid_t` cast —
> which SIGTERMed every process owned by uid 1000 on each `cargo test` run.
>
> This document's §6 asked for exactly that falsification, and it failed the
> test. Its evidence was real and its reasoning sound; it could not know its own
> test suite was the trigger, nor that the DRM errors sat at the end of the *file*
> because logging stopped after startup, not because they preceded the death.
>
> **Still valid and worth doing:** the DRM/`WAYLAND_DISPLAY` defect is genuine —
> Sunshine really is falling back to `Screencasting with KMS` — and the §5 fix
> (user unit bound to `graphical-session.target`) remains the right change. The
> §7 lessons and the §8 critique of the log mirror also stand.

---


**Date:** 2026-09-04
**Host:** `c2` / <host>
**Author:** Claude (Opus 5), `c2` session (Claude remote control)
**For:** the `c1` sentinel, who is actively investigating over SSH

**Status:** strong evidence, single-sample causality. Falsifiable test in §6.

**Companions:** [`2026-09-04-session-loss.md`](./2026-09-04-session-loss.md) (incident 1),
[`2026-09-04-second-session-loss.md`](./2026-09-04-second-session-loss.md) (incidents 2–3).

---

## 1. The claim

**Connecting to `c2` with Moonlight is what kills the session.**

`sunshine.service` runs as a *system* unit with no `WAYLAND_DISPLAY`, so Sunshine
cannot use the Wayland capture path and falls back to **KMS/DRM capture** on
`/dev/dri/card1` — the same GPU Hyprland is driving. On client connect it takes a
DRM lease and grabs planes, the compositor loses its DRM commits, and the session
ends seconds later.

This accounts for incidents 2 and 3 and for the "clean exit with no coredump"
signature. It does **not** account for incident 1 (19:30), which was a genuine
Hyprland segfault with a 9.4 MB core — a different bug.

---

## 2. The chain

```
sunshine.service = SYSTEM unit, Environment=XDG_RUNTIME_DIR only
        │            (no WAYLAND_DISPLAY — incident 2 §3.3)
        ▼
Sunshine cannot use the Wayland/portal capture path
        ▼
falls back to  "Info: Screencasting with KMS"   ← confirmed in journal
        ▼
Moonlight client connects
        ▼
DRM lease taken on card1 + plane grab
   compositor: "drm: Got a lease event for /dev/dri/card1"
   sunshine:   "Couldn't get drm fb for plane [0]"  (flood)
        ▼
compositor DRM commits fail
   "ERR: drm: Cannot commit when a page-flip is awaiting"  ×5
        ▼
session ends  — no coredump, no shutdown message, log ends mid-line
        ▼
Restart=always respawns Sunshine; operator reconnects; loop repeats
```

The loop is self-sustaining and operator-driven: every attempt to check whether
the machine is back is itself the thing that takes it down again.

---

## 3. Evidence

### 3.1 Capture backend (journal, `sunshine`)

```
21:33:01  Info: Screencasting with KMS
21:33:01  Info: Found monitor for DRM screencasting
21:33:06  Warning: Couldn't get drm fb for plane [0]: No such file or directory   (×many)
```

### 3.2 Dead vs live compositor logs

From `/var/log/hypr-crash-logs/` — the mirror c1 installed, which made this
possible. First frozen log of the evening.

| signal | dead session (`*.ended-20260904-213253.log`) | live session |
|---|---|---|
| `drm: Got a lease event for /dev/dri/card1` | **1** | 0 |
| `ERR: drm: Cannot commit when a page-flip is awaiting` | **5** | 0 |
| shutdown / cleanup message | **none — log ends abruptly** | n/a |

The dead session's log terminates mid device-enumeration with no teardown
message. Consistent with the session being pulled out from under the compositor
rather than Hyprland choosing to exit.

Sunshine's virtual input devices are visible in the dying log immediately after
the lease event — vendor/product `48879:57005` = `0xBEEF:0xDEAD`:

```
drm: Got a lease event for /dev/dri/card1
ERR: drm: Cannot commit when a page-flip is awaiting
libinput: New device Mouse passthrough:    48879-57005
libinput: New device Keyboard passthrough: 48879-57005
libinput: New device Pen passthrough:      48879-57005
libinput: New device Touch passthrough:    48879-57005
```

These devices appear **only** when a Moonlight client attaches, which timestamps
the connection inside the compositor's own log.

### 3.3 Connect → death timing

| Moonlight `CLIENT CONNECTED` | next `Exit the Session` | gap |
|---|---|---|
| 21:16:03 | 21:16:05 | **2 s** |
| 21:33:01 | 21:33:14 | **13 s** |
| 21:08:06 | 21:14:00 | 5 m 54 s |
| 21:18:06 | 21:22:21 | 4 m 15 s |

Two very tight couplings. The looser pairs are consistent with a session
surviving until the *next* capture re-initialisation rather than the initial one.

---

## 4. What this corrects

**Incident 2 §3.3 identified the missing `WAYLAND_DISPLAY` as the reason Sunshine
could not capture.** That is right, but it undersells it: the missing environment
does not merely make Sunshine blind, it makes Sunshine *destructive*, because the
fallback path it selects fights the compositor for DRM. The symptom "connected
but black" and the symptom "session keeps dying" are the same defect.

**Incident 2 §3.4 blamed `Restart=always` for restarting too early.** Also right,
but the DRM errors it noted (`Couldn't get drm fb for plane [0]`) are not a
symptom of racing the compositor at startup — they are the ongoing signature of
KMS capture contending with a compositor that owns the device. They recur on
every client connect, not only at boot.

**The 6-minute session lifetime is probably not a timer.** It is the interval
between operator reconnection attempts.

---

## 5. Recommended fix

Convert `sunshine.service` from a system unit to a **user** unit bound to the
graphical session, exactly as incident 2 §2 proposed:

```ini
# ~/.config/systemd/user/sunshine.service
[Unit]
Description=Sunshine
PartOf=graphical-session.target
After=graphical-session.target

[Service]
ExecStart=/usr/bin/sunshine
Restart=always
RestartSec=5

[Install]
WantedBy=graphical-session.target
```

with the session environment imported so the Wayland capture path is available:

```
systemctl --user import-environment WAYLAND_DISPLAY XDG_CURRENT_DESKTOP XDG_SESSION_TYPE
```

This fixes the environment problem and the DRM problem together: with a valid
`WAYLAND_DISPLAY`, Sunshine uses the compositor's own screencopy protocol and
never touches KMS. It also starts, stops and restarts with the session, which
removes the readiness-race entirely.

**Verify the fix by asserting the capability, not the unit state** — incident 2
§1's lesson. The success criterion is the journal reading anything *other* than
`Screencasting with KMS`, plus zero `Couldn't get drm fb` lines across a full
connect. `systemctl is-active` will say `active` either way and means nothing here.

### Immediate mitigation

Until that lands, **stopping Sunshine stabilises the machine**. Moonlight is
already unusable — connecting kills the session — so stopping it costs nothing
that currently works and breaks the loop. `c2` remains reachable over SSH and
Claude remote control.

```
sudo systemctl stop sunshine     # breaks the loop
sudo systemctl disable sunshine  # optional, survives reboot
```

---

## 6. How to falsify this

Single-sample causality on the tight couplings, so it deserves a real test:

1. Stop Sunshine. Leave the session alone for 30 minutes.
   - **Predicts:** zero `Exit the Session` events. If the session still dies,
     this analysis is wrong and the mechanism is elsewhere.
2. Start Sunshine, connect Moonlight once, do nothing else.
   - **Predicts:** `Screencasting with KMS`, a lease event in the compositor log,
     `Cannot commit when a page-flip is awaiting`, and session death within
     seconds to minutes.
3. Apply the user-unit fix, reconnect.
   - **Predicts:** no KMS line, no lease event, no page-flip errors, session
     survives.

Step 1 is the important one and costs only patience.

---

## 7. What Prism takes from this

Adds to the lessons already recorded in the companion documents:

1. **A "capability" health probe must name the mechanism, not just the outcome.**
   Incident 2 established that `is-active` is worthless here. This incident
   sharpens it: the *correct* probe for Sunshine is a log predicate on which
   capture backend it selected. `Screencasting with KMS` on a Wayland host is a
   red state even though every port is listening and every process is alive.

2. **Prism must model destructive interactions between facets, not just
   resource contention.** The architecture treats facets as competing for memory
   and VRAM. Here one facet destroys the *session* another depends on, at zero
   memory cost. Every governor tier read Green throughout. A dependency graph —
   "Sunshine requires the compositor and can harm it" — is a different axis from
   resource pressure and is not currently modelled anywhere.

3. **The recorder must capture the operator's own actions.** The trigger was a
   human reconnecting. Without connection events in the timeline, the pattern
   reads as a mysterious ~6-minute timer. With them it is obvious. Prism should
   record client connects/disconnects alongside system metrics.

4. **Instrumentation paid for itself within ten minutes.** The frozen log existed
   only because c1 installed the mirror at 21:26; the first session to die
   afterwards produced the artefact that resolved a question three prior
   incidents could not. This is the strongest possible argument for building
   `recorder/` early rather than late.

---

## 8. Note on the mirror itself

`/usr/local/bin/hypr-log-mirror.sh` polls and copies every 3 s. That risks losing
the final ~3 seconds — which is exactly the interval that explains a session
death. It happened to be sufficient here.

Suggest switching the copy to a streaming `tail -F` per instance while keeping
the system-service scoping, which is the part that matters and which c1 correctly
identified. A user-slice mirror dies with the session it is observing; that
reasoning is right and should be preserved in whatever Prism ships.

---

— **Claude (Opus 5)**, on `c2`
  2026-09-04, written between session deaths
