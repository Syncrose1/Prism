# Incident 2: second session loss, and the recovery that caused it

**Date:** 2026-09-04
**Host:** `c2` / <host>
**Session lost:** 21:14:00 BST
**Self-recovered:** 21:14:00 (greeter) → ~21:16 (autologin, session on VT2)
**Sunshine fully working:** 21:17:17, after manual restart
**Duration Moonlight unusable:** ~19 min (21:07:58 – 21:17:17)
**Diagnosed from:** `c1` (<laptop>), over Tailscale SSH
**Author:** Claude (Opus 5), c1 session

> [!NOTE]
> **Partially corrected by
> [`2026-09-06-tray-lock-contention.md`](./2026-09-06-tray-lock-contention.md)
> (2026-09-06).**
>
> §2 of this report recommended converting `sunshine.service` to a session-scoped
> **user unit**. That was implemented on 2026-09-04 23:43 and is *not* required:
> the 2026-09-06 streaming outage reproduced identically under both unit types,
> and reverting the migration did not fix it. The real cause there was Sunshine's
> broken GTK tray icon periodically holding a lock its HTTP threads needed.
>
> The `WAYLAND_DISPLAY` defect identified in §3.3 is real and still worth fixing —
> but setting `Environment=WAYLAND_DISPLAY=wayland-1` in the system unit solves it
> just as well, and is what runs on `c2` today.

**Companion document:** [`2026-09-04-session-loss.md`](./2026-09-04-session-loss.md) —
the 19:30 incident. **These are separate incidents with different root causes.**
Incident 1 was an upstream Hyprland segfault. This one was not a crash at all.

---

## Summary

The recovery actions taken at the end of Incident 1 **introduced this incident.**
Nothing crashed the compositor. The session ended because `sddm` deliberately
closed it, and Moonlight was unusable for 19 minutes primarily because Sunshine
was restarted in a way that could never have worked.

Three distinct faults compounded:

1. **Sunshine was started with no Wayland environment** and spent the entire
   session unable to capture — `WAYLAND_DISPLAY has not been defined`.
2. **`Restart=always`** (added during Incident 1 recovery) made Sunshine restart
   *eagerly* into a half-built session, producing a service that reported
   `active` while being functionally blind.
3. **The session itself ended cleanly at 21:14:00** — `sddm-helper` closed the
   PAM session and `exited successfully`, meaning `start-hyprland` returned of
   its own accord.

The honest headline: **the fix for Incident 1 was worse than the bug for 19
minutes**, and the operator's report of "can no longer connect" was caused by
the repair, not by a recurrence.

---

## Timeline

| Time (BST) | Event |
|---|---|
| 21:07:34 | `systemctl restart sddm` (Incident 1 recovery). Autologin → session 22, wayland, VT1 |
| 21:07:36 | Hyprland session comes up; portals start |
| 21:07:58 | `systemctl start sunshine` → **`Failed to create session: [wayland] WAYLAND_DISPLAY has not been defined`** |
| 21:07:58 | `Error: [av1_nvenc] Provided device doesn't support required NVENC features` |
| ~21:10 | `Restart=on-failure` → `Restart=always` applied; `daemon-reload` |
| 21:13:45 / 21:13:54 | Two diagnostic SSH sessions from `c1` (sessions 40, 41) — `type=tty`, open and close cleanly |
| **21:14:00** | **`sddm-helper[2677028]`: `[PAM] Closing session`; `Auth: sddm-helper exited successfully`** |
| 21:14:00 | `Activating special unit Exit the Session...`; session 22 removed |
| 21:14:00 | sddm starts greeter, `Jumping to VT 1`, `kwin_wayland` for greeter |
| 21:14:01 | **`kded6` segfaults** in `wl_proxy_get_version` |
| 21:14:11 | Sunshine SIGTRAP (core inaccessible) |
| ~21:16 | Autologin fires again unaided → new Hyprland, session 52, **VT2** |
| 21:16:08 | Sunshine (auto-restarted by `Restart=always`) floods `Couldn't get drm fb for plane [0]` |
| 21:17:17 | Manual `systemctl restart sunshine` → clean start, 0 DRM errors, Avahi established |

---

## Root cause analysis

### 3.1 The session end was not a crash

**No Hyprland coredump exists for this event.** `coredumpctl` for the window
lists only `hyprsunset` (SIGABRT), `kded6` (SIGSEGV) and `sunshine` (SIGTRAP) —
no `Hyprland`. Compare Incident 1, where a 9.4 MB `Hyprland` core was written.

The initiating line is:

```
21:14:00 sddm-helper[2677028]: [PAM] Closing session
21:14:00 sddm[2676984]:        Auth: sddm-helper exited successfully
```

`sddm-helper` is the process supervising the autologin session. It closed the
PAM session and **exited successfully** — which happens when the session command
(`/usr/bin/start-hyprland`) *returns*. sddm then did exactly the correct thing:
started a fresh greeter and jumped to VT 1.

So Hyprland **exited normally**, roughly 6.5 minutes after starting. Why it chose
to exit is not recoverable from the evidence (see §5), but it did not fault.

### 3.2 `kded6` is a symptom, not a cause

`kded6` segfaulted one second *after* the session teardown began:

```
#4  wl_proxy_get_version (libwayland-client.so.0)
#5  devicenotifications.so + 0x10e85
```

It called into a Wayland proxy whose compositor had already gone away. This is
ordinary shutdown-ordering breakage, not an initiator.

**This matters for `architecture.md` §1.2.** `kded6` is named there as *the*
ConflictKiller trigger, so its appearance in a crash log invites the conclusion
that the recursion fired again. **It did not.** `autoKillTrays: true` and
`autoKillNotificationDaemons: true` remain set, no `qs -p .*killDialog.qml`
process existed, no process storm occurred, and memory never moved (3.7 GiB of
30 GiB throughout). A `kded6` crash is **not** by itself evidence of §1.2.

### 3.3 Sunshine never worked in this session

This is the fault that actually cost the operator 19 minutes, and it was
introduced by the Incident 1 recovery.

```
21:07:58 Error: Failed to create session:
21:07:58 Error: [wayland] Environment variable WAYLAND_DISPLAY has not been defined
```

`sunshine.service` is a **system** unit (`/etc/systemd/system/sunshine.service`,
`WantedBy=multi-user.target`) running as `User=raahats` with only
`Environment=XDG_RUNTIME_DIR=/run/user/1000`. It carries **no `WAYLAND_DISPLAY`**,
so when started independently of the graphical session it cannot attach to the
compositor.

During normal boot this works by luck of timing and inherited state. Started
by hand mid-session — as done at 21:07:58 — it does not. `systemctl is-active`
reported **`active`** the entire time. The service was running, listening on all
three ports, and completely unable to capture. Moonlight's "connection refused"
was the honest downstream symptom.

### 3.4 `Restart=always` restarts *too early*

The Incident 1 fix was correct in intent — a clean SIGTERM caused by another
process's death is not a success, and `Restart=on-failure` had left Sunshine dead
for 95 minutes. But `always` with `RestartSec=5` and no readiness gate means
Sunshine races the compositor on every session restart:

```
21:16:08 Warning: Couldn't get drm fb for plane [0]: No such file or directory   ×20+
```

Sunshine grabbed KMS capture before Hyprland owned the display. Again: `active`,
listening, blind. A manual restart at 21:17:17 — once the session had settled —
produced **zero** DRM errors and a working service.

**`Restart=always` is necessary but not sufficient.** Without an ordering or
readiness condition it converts "dead and obviously broken" into "running and
silently broken", which is harder to diagnose, not easier.

### 3.5 What the diagnostic SSH sessions did — nothing

Recorded because it was actively investigated and wrongly suspected mid-incident:
two `c1` SSH sessions closed at 21:13:45 and 21:13:55, five seconds before the
teardown. They were **not** the cause.

- `KillUserProcesses=false` (verified via `busctl`)
- The SSH sessions were `class=user type=tty`; the desktop was `type=wayland`
- Dozens of identical SSH connections earlier the same day caused no harm
- The initiator is `sddm-helper`, on the session's own supervision path

`Linger=no` for `raahats` does mean `user@1000.service` is only kept alive while
a session exists — a real consideration for a headless-managed host — but it did
not fire here, because the graphical session's own end is what triggered teardown.

---

## What Prism should take from this

### 1. "Running" is not "working" — health must be capability-based

The single most transferable lesson. Across 19 minutes, `systemctl is-active
sunshine` returned **`active`** continuously while Sunshine was (a) unable to
create a Wayland session, then (b) unable to read DRM planes. Every
process-liveness check passed. The service was useless throughout.

`architecture.md` §4.3 defines facet health as an HTTP probe
(`ready_after`, `http = …/system_stats`). That would have passed here too —
Sunshine's web UI on :11001 was up the whole time.

**Recommendation.** Health probes for capture/streaming facets must assert the
*capability*, not the port: does the service hold a valid Wayland/DRM handle? For
Sunshine specifically, `Failed to create session`, `WAYLAND_DISPLAY has not been
defined` and `Couldn't get drm fb` are all high-signal log predicates that
directly contradict `is-active`. Prism should treat log-derived health as
first-class alongside HTTP probes.

### 2. Session-coupled services need a readiness gate, not just a restart policy

Incident 1 recommended `Restart=always`; this incident shows that alone is a
trap. What is actually required:

- `ExecStartPre` that waits for a compositor (poll for `WAYLAND_DISPLAY` /
  `/run/user/1000/wayland-*`, or `hyprctl monitors` succeeding), **and**
- the correct environment imported (`systemctl --user import-environment
  WAYLAND_DISPLAY XDG_CURRENT_DESKTOP`, or convert Sunshine to a **user** unit
  bound to `graphical-session.target`), **and**
- `Restart=always` as the backstop.

Converting `sunshine.service` from a system unit to a `--user` unit with
`PartOf=graphical-session.target` would fix §3.3 and §3.4 together: it would
start with the session, stop with it, restart with it, and always inherit a
valid `WAYLAND_DISPLAY`. **This is the recommended change, and it was not made —
it is a behavioural change to the operator's streaming host and needs their
decision.**

### 3. Recovery actions are themselves an incident risk

Prism will take automated recovery actions. This incident is a live example of a
recovery that *created* an outage: a hand-started service in the wrong
environment, plus a restart policy applied without a readiness condition,
produced 19 minutes of "connected but black" that looked to the operator exactly
like a fresh crash.

**Recommendation.** Every automated recovery must be followed by a *verification
step that asserts the capability it was trying to restore* — and if verification
fails, say so loudly rather than reporting the action as successful. §2's
principle *"Every intervention is reported"* should be strengthened to **"every
intervention is verified, and the verification is reported."** An unverified
rescue is how you get an outage that looks like the thing you just fixed.

### 4. The compositor log dies with the compositor

`/run/user/1000/hypr/*/hyprland.log` is on tmpfs under `/run/user/1000`. When
`user@1000.service` stopped, **the log for the session that just ended was
destroyed.** Only the new session's log survives. This is precisely the evidence
needed to explain why Hyprland exited, and it is deleted by the event under
investigation.

**Recommendation.** `recorder/` should tail the active Hyprland log and mirror it
to persistent storage continuously, or copy it on session-end detection. Without
this, every clean compositor exit is permanently unexplainable — as this one now
is. This is cheap and would have turned §5 below into a definitive answer.

### 5. Two losses, two mechanisms, ~1h45m apart

| | Incident 1 (19:30) | Incident 2 (21:14) |
|---|---|---|
| Mechanism | Hyprland SIGSEGV, dwindle layout null deref | Clean exit; sddm closed session |
| Coredump | `Hyprland` 9.4 MB present | **none for Hyprland** |
| Recovery | Manual `systemctl restart sddm` | **Self-recovered** via autologin |
| Sunshine | Stayed dead (`Restart=on-failure`) | Restarted but blind |
| Session VT | VT1 | VT1 → **VT2** |

Two different failure modes in under two hours on a host with 8d 23h uptime is a
stability signal in itself. **Why Hyprland exited cleanly at 21:14:00 is
unresolved** and, per §4, the evidence is gone. If it recurs, the first action
should be preserving the Hyprland log before the session is restarted.

---

## Actions taken

| Action | Result |
|---|---|
| Diagnosed session-22 end via `sddm-helper` PAM trace | Confirmed clean exit, not a crash |
| Examined `kded6` core (PID 2679614) | Wayland-proxy use-after-compositor-death — symptom, not cause |
| Verified `KillUserProcesses=false`, `Linger=no` | Exonerated the diagnostic SSH sessions |
| `systemctl restart sunshine` at 21:17:17 | **0 DRM errors**, Avahi established, ports listening |

**Not changed, pending operator decision:** converting `sunshine.service` to a
user unit bound to `graphical-session.target` (§2). **Deferred:** enabling
Tailscale SSH — `tailscale set --ssh` warns it reroutes SSH and drops the current
session, and the tailnet ACL could not be verified from the node. Not a change to
make while the session is this unstable and SSH is the only reliable path in.

---

## Recurrence: 21:22:21 — the same failure, a second time

**Appended 21:30.** The failure documented above **repeated at 21:22:21**, roughly
six minutes after the session that autologin had restored. It is recorded here
rather than as a third report because it is the *same mechanism*, not a new one.

| | 19:30 (Incident 1) | 21:14 (this report) | 21:22 (recurrence) |
|---|---|---|---|
| Hyprland coredump | **yes**, 9.4 MB | none | none |
| Mechanism | SIGSEGV, dwindle layout | clean exit | clean exit |
| Session lifetime | 8 d 23 h | 6 min 26 s | ~6 min |
| Journal before exit | crash backtrace | silent | silent |
| Recovery | manual `restart sddm` | autologin (once) | **stuck at greeter** |

The 21:22 teardown is byte-for-byte the same shape: `sddm-helper[2682825]:
[PAM] Closing session` → `Auth: sddm-helper exited successfully`, no Hyprland
core, `hyprsunset` SIGABRT alongside, and **complete journal silence in the six
minutes preceding it**. Process parentage was confirmed live:
`/usr/bin/start-hyprland` (2687818) → `Hyprland --watchdog-fd 4` (2687844). When
Hyprland returns, `start-hyprland` exits, PAM closes, the session ends. That is
the observed signature exactly.

### `Relogin=false` — why every recovery needed a human

`/etc/sddm.conf.d/kde_settings.conf` contains:

```ini
[Autologin]
Relogin=false
```

Autologin therefore fires **once per sddm daemon start**. After a session ends,
sddm falls back to the greeter and waits for a keyboard. This explains the whole
evening's recovery pattern: `systemctl restart sddm` "fixed" things each time not
by repairing anything, but by **re-arming the one-shot autologin**.

Ten `Exit the Session` events were recorded this boot, clustered 21:04–21:22.

**On a headless host operated remotely, `Relogin=false` guarantees that the first
compositor exit becomes an indefinite outage.** It is arguably a larger
availability defect than either crash, because it converts a self-healing event
into one requiring intervention — and if SSH had not been available, intervention
would have been impossible.

Not changed: `Relogin=true` means anyone with physical access to `c2` is logged
straight in after a crash. That is the operator's security trade to make.

### Hypotheses eliminated

Recorded so the next investigation does not re-tread them:

- **Idle timers — ruled out.** `hypridle` is running, but its listeners are 1500 s
  (25 min) and 1800 s (30 min), neither near the ~6 min lifetime, and both
  `lock_cmd`/monitor-off actions that do not end a session.
- **GPU fault — ruled out.** No Xid errors, no GPU resets, no DRM errors in the
  journal this boot. NVIDIA 610.43.02 on the RTX 3060 initialised cleanly each
  time (`aquamarine` reaching `Swapchain: Reconfigured … 1920x1080`).
- **Memory pressure — ruled out.** 3.7 GiB of 30 GiB used throughout; no OOM, no
  PSI stall.
- **Diagnostic SSH sessions — ruled out** (§3.5).

**Still unexplained:** what causes Hyprland to return cleanly. The evidence needed
is its own log, which tmpfs destroys with the session (§4) — twice now.

### Mitigation installed: durable log mirroring

Acting on §4's recommendation, a system service now mirrors the Hyprland log off
tmpfs:

```
/usr/local/bin/hypr-log-mirror.sh      copies /run/user/1000/hypr/*/hyprland.log
/etc/systemd/system/hypr-log-mirror.service   → /var/log/hypr-crash-logs/ every 3 s
```

Enabled at boot, `Restart=always`, `Nice=10`, `IOSchedulingClass=idle`. On session
end it freezes a timestamped `*.ended-<ts>.log`, emits a `logger` line, and keeps
the 20 most recent.

**Design note worth carrying into Prism.** The first version of this mirror was
started with `nohup` from an SSH session and therefore lived in the user slice —
it would have been killed by `user@1000.service` stopping, *at the exact moment
it needed to be writing*. A recorder for session-death events **must not live
inside the session it observes.** This is the same class of error as §1's
"running is not working": the naive implementation appears to function during
normal operation and fails precisely when it matters.

---

## Environment

Unchanged from Incident 1. Additional detail specific to this event:

```
sunshine.service  system unit, User=raahats, WantedBy=multi-user.target
                  Environment=XDG_RUNTIME_DIR=/run/user/1000  (no WAYLAND_DISPLAY)
                  Restart=always, RestartSec=5  (changed 21:10, was on-failure)
                  backup: /etc/systemd/system/sunshine.service.bak-20260904
GPU               NVIDIA RTX 3060, driver 610.43.02, av1_nvenc unsupported
Session 22        wayland, VT1, 21:07:34 → 21:14:00 (6 min 26 s)
Session 52        wayland, VT2, ~21:16 → current
logind            KillUserProcesses=false; Linger=no for raahats
Memory            3.7 GiB / 30 GiB used throughout — no pressure at any point
```

Cores retained: `kded6` (2679614, 4.3 M), `hyprsunset` (2677256).
`sunshine` (2677738, SIGTRAP) core is **inaccessible** — not captured.

---

## Sign-off

Incident 1's finding was that Prism's existing mitigations worked and the machine
became unreachable anyway. Incident 2's finding is sharper and less comfortable:
**the repair caused the next outage.**

`Restart=always` was the right diagnosis of the wrong altitude. It fixed "the
service stays dead" and created "the service runs blind", and because
`systemctl is-active` answered `active` throughout, every conventional health
check agreed the system was fine while the operator sat looking at a connection
error. For a project whose stated job is automated recovery, that failure mode —
**a rescue that reports success and verifies nothing** — is the one most worth
designing against.

The most valuable single change from this incident is also the cheapest:
persist the Hyprland log outside tmpfs. It costs almost nothing, and its absence
is the only reason the root cause of this session's exit is now unknowable.

**That change is now made** (see Recurrence, above) — prompted by the failure
repeating at 21:22 and destroying its own evidence a second time. Three session
losses in under two hours, two of them from a mechanism still unidentified,
because the one artefact that would identify it lives on tmpfs and dies with the
thing it describes.

The remaining open question is not *why did this crash* but *why does Hyprland
exit cleanly after roughly six minutes*. Idle timers, GPU faults, memory pressure
and the diagnostic SSH sessions are all eliminated. The next occurrence will
leave a frozen log in `/var/log/hypr-crash-logs/`, and that should end the
guessing — which, on the evidence of this evening, is worth more than any further
inference drawn without it.

— **Claude (Opus 5)**, sentinel of `c1`
  2026-09-04, London — second watch of the evening
