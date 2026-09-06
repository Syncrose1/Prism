# Incident 4: a broken tray icon stalled the streaming server

**Date:** 2026-09-06
**Host:** `c2` / DESKTOP-CACHY02
**Symptom onset:** overnight Sat→Sun; Moonlight unusable Sunday morning
**Root cause identified:** 16:36 BST
**Fixed:** `system_tray = disabled`
**Author:** Claude (Opus 5), `c1` session

**Status:** **Confirmed by controlled experiment** — 90/90 probes pass with the
tray disabled, versus a reproducible ~40 s block/release cycle with it enabled.

**Companions:**
[`2026-09-04-session-loss.md`](./2026-09-04-session-loss.md),
[`2026-09-04-second-session-loss.md`](./2026-09-04-second-session-loss.md),
[`2026-09-04-third-kill-minus-one.md`](./2026-09-04-third-kill-minus-one.md),
[`2026-09-04-drm-capture-root-cause.md`](./2026-09-04-drm-capture-root-cause.md).

---

## 1. The finding

Sunshine's system tray icon — `libayatana-appindicator` → GTK → pango, running
under Hyprland with no working StatusNotifier host — periodically acquired a lock
that Sunshine's HTTP server threads also needed. It held that lock while stalled
in GTK/font work, then released it.

The result: **HTTP port 11000 alternated between blocked and healthy on a ~40 s
cycle**, in a single long-lived process, with no crash and no restart.

Moonlight pairing runs over HTTPS on 10995. Streaming needs HTTP on 11000. So the
host was reliably *discoverable* and reliably *unstreamable* — the operator's
description was exact:

> "it becomes reachable only to give a pairing request and becomes unreachable
> halfway through the pair"

## 2. Evidence

### 2.1 The block/release cycle

One probe per second against `localhost:11000`, tracking pid and accept-queue depth:

```
t=1–17s   pid=1431924  q=13→36   TIMEOUT       ← blocked, queue climbing
t=18s     pid=1431924  q=0       SERVER_FREE   ← unblocks, queue drains instantly
t=18–57s  pid=1431924  q=0       SERVER_FREE   ← 40 s healthy
t=58–60s  pid=1431924  q=1→5     TIMEOUT       ← blocks again
```

**Same pid throughout** — not crash-and-restart. And the queue draining to zero in
a single second is the decisive detail: a genuine deadlock never recovers, whereas
a lock *released by another thread* frees every queued connection at once.

### 2.2 What was blocked, together

During a blocked phase, every server thread was in `futex_wait` simultaneously:

```
1431998  nvhttp            futex_wait
1431999  confighttp        futex_wait
1432000  rtsp              futex_wait
1432005  nvhttp::11000     futex_wait
1432002  [pango] fontcon   futex_wait      ← font rendering, in the request path
1431943  sunshine          hrtimer_nanosleep
```

`[pango] fontcon` blocking alongside the HTTP threads is the smoking gun. Font
rendering has no legitimate reason to share a lock with an HTTP server.

### 2.3 The tray was visibly broken at every startup

```
Info: Starting system tray
libayatana-appindicator is deprecated. Please use libayatana-appindicator-glib…
Info: System tray created
gtk_widget_get_scale_factor: assertion 'GTK_IS_WIDGET (widget)' failed
```

`GTK_IS_WIDGET` failing means the tray widget was never valid. Sunshine reported
"System tray created" anyway.

### 2.4 The controlled test

| Condition | Result |
|---|---|
| Tray enabled | 17 s blocked / 40 s healthy / 3 s blocked, in a 60 s window |
| **`system_tray = disabled`** | **90/90 probes OK, 0 timeouts** |
| Tray disabled, from `c1` over tailnet | 10/10, then 8/8 after firewall cleanup |

## 3. Why this took ~4 hours and ten wrong theories

Every one of these was proposed, tested, and disproven before the real cause:

| # | Theory | How it died |
|---|---|---|
| 1 | c2's IP changed | Tailnet IP `100.109.171.51` never changed; operator corrected this |
| 2 | c2's firewall | Added rules, then blanket `allow in on tailscale0`; symptom unchanged |
| 3 | Tailnet UDP broken | 36,460 packets at ~45 Mbps to :11009, zero loss |
| 4 | DERP relay degradation | Path was `direct`, 41 ms |
| 5 | c1's firewall | `allow in on tailscale0` confirmed applied; inbound UDP test succeeded |
| 6 | Stale Moonlight sockets | Killed and relaunched; identical failure |
| 7 | HDR / AV1 on an RTX 3060 | HDR already off; encoder probe lines appear in working sessions too |
| 8 | Missing file capabilities | `cap_sys_admin=p` correctly set |
| 9 | Beta vs stable Sunshine | Cycling reproduced on both builds |
| 10 | The user-unit migration | Reverted to system unit; cycling continued |

**The methodological failure was one-shot testing of an intermittent fault.**

With a ~40 s block and a ~40 s healthy window, any single probe is roughly a coin
flip. A `curl` that succeeds "proves" the service is fine; the next failure looks
like a *different* problem. That is precisely how theory #10 gained false support —
the revert happened to land in a healthy window, so the fix appeared to work, and
the next stall looked like a new fault.

**The operator broke the deadlock twice, both times by supplying a pattern rather
than a datapoint:**

1. *"the first fails and second succeeds… about 8 seconds passes. Not very long at
   all."* — killed the PIN-expiry theory. A strict fail/succeed alternation is not
   what latency looks like.
2. *"it's highly suspicious that c2 is online for a few moments then unresponsive…
   it smells like a race condition."* — prompted the continuous 1 Hz probe, which
   exposed the cycle in 60 seconds after hours of one-shot tests had hidden it.

## 4. What Prism should take from this

### 4.1 Intermittent faults require continuous sampling, not probes

This is the central lesson and it generalises directly to `prismd`.

A health check that samples once per interval cannot distinguish "healthy" from
"healthy 50 % of the time on a 40 s cycle." Worse, it will report **flapping**, and
a naive governor will treat each flap as an independent event — exactly the
`architecture.md` §4.2 flap-protection scenario, but arriving from a measurement
artefact rather than a real oscillation.

**Recommendations:**

- Health checks must record a **time series**, not a boolean. "9/10 in the last
  10 s" is actionable; "up" is not.
- Add **duty cycle** as a first-class health metric. A facet that is up 50 % of the
  time is not half-healthy, it is broken in a specific and diagnosable way.
- The `recorder/` should keep a rolling window of probe results so a later analysis
  can see the *shape* of a failure, not just its most recent sample.

### 4.2 Accept-queue depth is a cheap, high-signal liveness metric

`Recv-Q` on a listening socket climbing while the process is alive means the
service is accepting connections into the kernel backlog but not processing them.
It distinguishes:

- **process dead** → connection refused
- **process alive, thread wedged** → `Recv-Q` climbing ← *this incident*
- **process healthy** → `Recv-Q` at 0

One `ss -tln` gives this for free, and it is far more informative than
`systemctl is-active`. This sharpens incident 2's "running is not working" lesson
into a concrete metric.

### 4.3 Kernel thread stacks resolve what userspace logs cannot

`/proc/<tid>/stack` named the mechanism when every log line looked normal. Seeing
`nvhttp`, `confighttp`, `rtsp` **and `[pango] fontcon`** in `futex_wait` together
made the shared-lock hypothesis obvious; nothing in Sunshine's own output hinted
at it.

Worth adding to `recorder/`: on a facet health failure, capture per-thread
`comm` + `wchan` + stack top. It is a handful of file reads and it converts
"the service is hanging" into "these threads are blocked on this."

### 4.4 Decorative subsystems do not belong in a server's lock graph

The proximate bug is upstream Sunshine's: a tray icon should never be able to
stall the HTTP server. But the general principle applies to Prism directly —
**a daemon whose job is availability must not link its serving path to optional
GUI/desktop-integration code.** `architecture.md` §2's "Prism must work when
nothing else does" argues for `mlockall` and a small static binary; this incident
adds: no GTK, no font rendering, no tray, nothing that reaches into a desktop
session, anywhere near the request path.

### 4.5 Correlation under self-influence, again

Incident 3 §6.4 recorded that both agents reached confident wrong conclusions
because each was inside the system it measured. This incident adds a variant:
**an intermittent fault manufactures false correlations for any change you make.**
Ten interventions each appeared to help or fail depending only on which phase of
the cycle the verification landed in.

The defence is the same as §4.1 — verify against a *window*, never a point. Prism
should never mark an intervention successful on a single post-action probe.

## 5. Changes made

| Change | Detail |
|---|---|
| **`system_tray = disabled`** | `~/.config/sunshine/sunshine.conf` — **the fix** (backup `/tmp/sunshine.conf.bak`) |
| Sunshine unit | Reverted user unit → system unit; `enabled` so it survives reboot |
| `WAYLAND_DISPLAY=wayland-1` | Added to the system unit — without it Sunshine falls back to KMS capture (incident 2 §3.3). Log now confirms `Screencasting with Wayland's protocol` |
| Package | `sunshine-beta-bin` (May) → stable `sunshine 2026.724.5619` (July). Not the fix, but newer and it restored `cap_sys_nice` |
| Firewall cleanup | Removed 19 redundant per-port rules from c2; kept `allow in on tailscale0` on both hosts |
| Pairing | `c1` paired via web API; device list went 5 → 6 |

**Preserved:** user unit at `/tmp/sunshine-user-unit.backup`; original system unit
at `/etc/systemd/system/sunshine.service.bak-20260904`.

**Outstanding:** the Sunshine web UI credentials set during this investigation
(`claudeadmin`) should be changed by the operator.

## 6. Corrections to earlier reports

**Incident 2 §2 recommended migrating `sunshine.service` to a session-scoped user
unit.** c2's Claude implemented that on 2026-09-04 23:43. This report supersedes
that recommendation *as a fix*: the migration was not the cause of this outage and
reverting it was not the cure — the tray bug reproduced identically under both
unit types. The `WAYLAND_DISPLAY` problem incident 2 identified is real, but it is
solved just as well by setting the variable in the system unit.

**On the API `status` field.** During this incident `{"status":true}` from
`POST /api/pin` was twice reported as confirmed pairing. It is not: a bogus PIN
`0000` with no pending request also returns `true`. The reliable check is the
device list in `sunshine_state.json`. A subsequent claim that *no* pairing had
succeeded was also wrong — a stale `md5sum` read; the device list showed `c1` had
in fact been added.

---

## Sign-off

Four incident reports across three days. A compositor segfault, a recovery that
caused its own outage, a test suite calling `kill(-1)`, and now a tray icon that
stalled a streaming server for four hours of investigation.

The through-line is measurement, not any of the individual bugs. Incident 2:
`systemctl is-active` said "active" while the service was blind. Incident 3: two
agents built careful theories while unable to see their own influence. This one:
ten disproven hypotheses, every one of them "tested," because a single probe
against a 50 %-duty-cycle fault is a coin flip dressed as evidence.

For a project whose first goal is *"Never let the machine become unreachable,"*
the operative lesson is that **you cannot know whether a machine is reachable by
asking once.**

Both breakthroughs here came from the operator noticing *shape* — an alternating
pattern, and a rhythm that "smells like a race condition" — while the agent was
still collecting points. Prism's dashboards should make shape visible by default,
because that is what a human recognises and what a threshold check discards.

— **Claude (Opus 5)**, sentinel of `c1`
  2026-09-06, London — ten wrong answers, one broken icon
