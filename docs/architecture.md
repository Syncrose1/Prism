# Prism — Architecture

> A prism does not add light. It takes what is already there and makes the
> structure of it visible.

Prism is a resilience and remote-control layer for a single workstation (`c2`)
that runs heavy local AI workloads and is operated remotely over Tailscale for
days at a time.

Its job, in priority order:

1. **Never let the machine become unreachable.** Availability beats any single
   in-flight job.
2. **Recover automatically** from the two known failure modes without a human.
3. **Give the operator remote hands** — see everything, start/stop/retune
   anything, from a phone in London.
4. **Serve compute** — expose the workloads it manages as first-class network
   services.

---

## 1. The failure modes this exists to solve

### 1.1 The zram thrash spiral (root cause of the unrecoverable hangs)

The machine has 30 GiB RAM, **no disk swap**, and was configured with
`zram-size = ram * 1.5` → 46 GiB of zram.

zram is *compressed swap held in RAM*. Oversizing it produces a feedback loop:

```
memory fills
  → kernel swaps anon pages to zram
    → zram allocates RAM to store the compressed pages
      → less free RAM
        → swap harder
```

The OOM killer never fires, because from its perspective there is still tens of
gigabytes of free swap. So nothing is killed. Everything — compositor, sshd,
tailscaled — gets paged out. The box thrashes indefinitely.

This is why the observed symptom was *"hangs that don't resolve"* and *"I crash
far more often than I get warned and have the offending process terminated."*
The OOM killer was configured out of the game.

**Mitigated at the config layer** by sizing zram to `ram * 0.5`. **Mitigated at
the Prism layer** by `memory.swap.max` per workload — see §4.2.

#### The honest-headroom metric

`SwapFree` is actively misleading on a zram-only system. Swapping out `X` bytes
frees `X` but costs `X/ratio` to store, so the true net gain is `X · (1 − 1/ratio)`.

```
ratio           = zram DATA / zram COMPR        (observed, ~2.5 on zstd here)
honest_headroom = MemAvailable + SwapFree · (1 − 1/ratio)
```

This is the number no standard tool shows, and it is the **hero metric** on the
dashboard. Every threshold in the governor is expressed against it rather than
against `free`.

### 1.2 The ConflictKiller recursion

*Observed and diagnosed live on 2026-09-04, mid-incident.*

The visual symptom is stacked bars filling the screen top to bottom, which reads
as "quickshell is recursively spawning bars". That is not the mechanism.

`~/.config/quickshell/ii/services/ConflictKiller.qml` checks for a conflicting
system tray (`kded6`) or notification daemon (`mako`/`dunst`). If one is found
and the corresponding `autoKill*` option is **false**, it opens a dialog:

```qml
Quickshell.execDetached(["qs", "-p", root.killDialogQmlPath])
```

But `qs -p` starts a **new quickshell instance loading the full `ii` config** —
including the `ConflictKiller` singleton. That instance reaches `Config.ready`,
runs the same check, finds the same conflict, and spawns another. The recursion
is self-referential and gated only on a conflicting process continuing to exist.

Each generation is a ~300 MB process that paints its own `bar`, `dock`,
`background` and `screenCorners` layers — hence the stacked bars. Measured at
peak during the incident: **36 processes, 11.2 GiB, one new generation every
~3 seconds**, with 33 duplicate bars and 132 screenCorner surfaces.

**The trigger was `kded6`**, D-Bus activated and therefore able to reappear at
any time, with `autoKillTrays: false` in `~/.config/illogical-impulse/config.json`.

#### Why this is the same bug as §1.1

These are not two independent faults. They compose into a doom loop:

```
memory pressure / system stall
  → quickshell restarts (or Config re-readies)
    → ConflictKiller runs, finds kded6
      → recursive dialog storm, ~300 MB every 3s
        → memory exhaustion
          → deeper stall  ─────────────┐
             ▲                          │
             └──────────────────────────┘
```

A recoverable memory squeeze bootstraps the recursion; the recursion then
manufactures a far worse memory event than the original workload ever would.
On the old 46 GiB zram config there was no OOM kill to break the cycle, so it
ran until the machine was unreachable. This is the mechanism behind all three
lost weekends.

**Mitigated at the config layer** by setting `autoKillTrays` and
`autoKillNotificationDaemons` to `true`, which converts the dangerous path
(spawn a recursive dialog) into a safe one (`killall kded6`). **Mitigated at the
Prism layer** by the watchdog below, which must treat this as a first-class
failure mode rather than a curiosity.

The upstream defect — `killDialog.qml` instantiating the singleton that spawns
`killDialog.qml` — remains. The config change closes both paths that reach it,
but Prism should not assume it is gone.

### 1.3 Prism itself

*Observed 2026-09-04. Full account in
[`docs/incidents/2026-09-04-third-kill-minus-one.md`](incidents/2026-09-04-third-kill-minus-one.md).*

The third failure mode this project has to defend against is **Prism**.

A unit test in `prismd` passed `u32::MAX` to `terminate()` as "a pid that cannot
exist". `pid_t` is `i32`, so the cast wrapped:

```
u32::MAX  =  4294967295  =  0xFFFFFFFF
as pid_t  =  -1
kill(-1, SIGTERM)         →  every process the caller may signal
```

Each `cargo test` run therefore terminated the operator's entire graphical
session — compositor, shell, stream, everything owned by uid 1000 — while the
machine was being operated remotely from another city. Four sessions were lost
this way. It was identified only by a kernel audit rule on `kill`; every
userspace signal pointed elsewhere, because SIGTERM leaves no coredump, no
shutdown message, and no resource anomaly.

Three properties of this are worth keeping in view, because they generalise:

**The dangerous constant looked safe two functions away.** `alive(u32::MAX)` is
completely harmless — it stats `/proc/4294967295` and fails cleanly. The same
value in `signal()` is catastrophic. A reader checking whether `u32::MAX` was
used safely in this file would have found a reassuring precedent.

**Both investigating agents built confident, well-evidenced, wrong theories.**
`c1` attributed a loss to its own SSH sessions; this session built a detailed
DRM-contention theory, complete with a falsification protocol, while its own test
suite was the trigger. Neither was careless. The common defect is *an observer
inside the system it measures* — and Prism will be permanently in that position,
acting on a machine and then reading the consequences of its own actions. Hence
the principle about logging its own interventions into the timeline.

**One analytical error is worth naming specifically.** The DRM theory read
*position in the log file* as *proximity in time*: the frozen compositor log
ended with DRM errors, which was taken to mean they preceded the death. In fact
the log stopped at startup and the compositor ran nine more minutes writing
nothing. "Last lines in the file" meant "last thing worth logging during
startup", not "last thing before dying". Prism's recorder must therefore carry
explicit timestamps into every artefact it captures, so that a future analysis
cannot make the same substitution.

**Mitigations, all in place:**

| Layer | Mitigation |
|---|---|
| Code | [`safety::SafetyGuard`](../crates/prism-core/src/safety.rs) is the sole source of a signalable pid; refuses groups, init, self, ancestors, and reachability-critical names |
| Tests | [`scripts/test-isolated.sh`](../scripts/test-isolated.sh) runs the suite in a PID namespace, so a broadcast signal cannot escape even if the guard is wrong |
| Kernel | audit rule on `kill`/`tkill`/`tgkill` (key `sigwatch`), plus `hypr-death-watch.service`, both installed on this host |

The kernel layer is the one that actually found it. §4.5 should treat a standing,
narrowly-scoped audit rule on signal delivery as part of the recorder rather than
as optional instrumentation: it answers "who killed my facet?" unambiguously, and
no amount of userspace polling can.

**The known-good remedy is already bound to `CTRL+SUPER+R`:**

```
killall ydotool qs quickshell; qs -c ii &
```

Note this restarts **quickshell only, not Hyprland**. The reason it succeeds
where a manual kill-and-restart fails is that *Hyprland* spawns the
replacement, so the new process is session-owned rather than terminal-owned and
does not die with the terminal. Prism reproduces this exactly via
`hyprctl dispatch exec`, which is cheap, safe, and loses no windows.

---

## 2. Design principles

| Principle | Consequence |
|---|---|
| **The kernel is the floor, userspace is the finesse.** | cgroup limits enforce hard boundaries that cannot fail. The daemon exists to act *earlier* and *more gracefully* than the kernel would — not to be the only line of defence. |
| **Prism must work when nothing else does.** | Rust, no GC, small static binary, `mlockall(MCL_CURRENT|MCL_FUTURE)` so the daemon can never be paged out during the exact thrash it needs to resolve. |
| **Generous by default, tunable live.** | The operator routinely runs workloads that consume nearly the whole machine. Prism must not pre-emptively cap that. Limits ship permissive; the UI makes them adjustable in real time. |
| **Restrict swap, not RAM.** | The spiral is a swap runaway. `memory.swap.max` stops it without constraining legitimate RAM use. |
| **Attribute before acting.** | Never kill blind. Always know which facet caused the stall. |
| **Bound the blast radius by construction.** | Prism kills processes for a living, so it is one bad pid from being the outage. Every signal passes [`safety::SafetyGuard`](../crates/prism-core/src/safety.rs), which is the *only* source of a signalable pid: it refuses process groups, init, self, ancestors, and anything reachability depends on. Prefer `cgroup.kill` over raw pids — a cgroup cannot escape to the session. See §1.3. |
| **Every intervention is verified, and the verification is reported.** | A rescue that reports success and checks nothing is worse than no rescue: it produces an outage wearing the costume of a fix. Assert the *capability* that was being restored, not the exit status of the repair. |
| **Log Prism's own actions into the timeline it reads.** | An observer inside the system it measures cannot see its own contribution. Two agents built confident, evidence-led, wrong theories on 2026-09-04 for exactly this reason. The recorder must let a future analysis subtract Prism. |

---

## 3. Component map

```
┌─ prismd ──────────────── Rust · systemd --user · mlockall'd ───┐
│                                                                │
│  sensors/     PSI · meminfo · zram mm_stat · cgroup stats      │
│               NVML · hyprctl · sunshine state                  │
│                                                                │
│  governor/    tiered policy engine · attribution · cooldowns   │
│                                                                │
│  supervisor/  facet registry · transient scope lifecycle       │
│               live limit adjustment · health probes            │
│                                                                │
│  watchdogs/   quickshell recursion · hyprland liveness         │
│               hardware /dev/watchdog pet (opt-in)              │
│                                                                │
│  recorder/    fsync'd ring buffer · survives hard reset        │
│                                                                │
│  beacon/      push notifications on every intervention         │
│                                                                │
│  api/         axum · REST + WebSocket · tailnet-bound          │
│  ui/          embedded SPA (rust-embed, no runtime deps)       │
└────────────────────────────────────────────────────────────────┘
```

`prism` — a thin CLI client against the same API, for local/SSH use.

---

## 4. Subsystems

### 4.1 Sensors

Base sample rate 1 Hz, escalating to 10 Hz above Amber.

- `/proc/pressure/memory` — `full` and `some`; the **`total` counter delta** is
  used rather than `avg10`, since avg10 has a ~10 s lag that matters when the
  spiral is exponential.
- `/proc/meminfo` — `MemAvailable`, `SwapFree`.
- `/sys/block/zram0/mm_stat` — live compression ratio and real RAM cost of swap.
- Per-facet cgroup: `memory.current`, `memory.swap.current`, `memory.pressure`,
  `memory.events`.
- NVML — VRAM per process, utilisation, temperature.
- Hyprland — `hyprctl layers -j` surface counts, `qs` process count and RSS.

### 4.2 Governor

Tiered, driven by PSI stall and honest-headroom. All thresholds are defaults and
are per-facet overridable at runtime.

| Tier | Trigger | Action |
|---|---|---|
| **Green** | — | Normal sampling. |
| **Amber** | `full` stall > 5 % sustained 10 s, or headroom < 4 GiB | Escalate sampling, begin high-fidelity recording, compute attribution, notify. |
| **Red** | `full` stall > 20 % sustained 15 s, or headroom < 1.5 GiB | Graceful intervention on the attributed facet — facet-defined hook (ComfyUI: unload models + free memory + drain queue; llama.cpp: unload). Notify. |
| **Black** | `full` stall > 50 % sustained 10 s, or headroom < 500 MiB | `SIGTERM` the facet scope → 5 s grace → `cgroup.kill` (atomic, whole tree, no orphaned CUDA workers). Notify. |
| **Terminal** | prismd itself cannot be scheduled | Hardware watchdog goes unpetted; board resets. (Opt-in, §4.4.) |

**Attribution.** The target is the facet with the highest
`memory.current + memory.swap.current`, weighted by growth rate over the
preceding window. Non-facet processes are never touched by default — a browser
sitting at 500 MB is not the problem, and killing it would be user-hostile.

**Flap protection.** Every action has a cooldown. Three interventions on the
same facet inside 10 minutes stops automatic action and escalates to a
notification instead — an intervention loop is worse than the original fault.

### 4.3 Supervisor and facets

A **facet** is a registered, Prism-owned workload. Registry is
`~/.config/prism/facets/*.toml`, hot-reloaded on change. Adding one is dropping
a file — or using the UI's form, or `prism add`, which both write the same TOML.
Templates ship for llama.cpp, ComfyUI, Ollama, vLLM.

```toml
id   = "comfyui"
name = "ComfyUI"

command = "/home/raahats/ComfyUI-Easy-Install/ComfyUI-Easy-Install/run.sh"
cwd     = "/home/raahats/ComfyUI-Easy-Install/ComfyUI-Easy-Install"

[health]
http        = "http://127.0.0.1:8188/system_stats"
ready_after = "60s"

[limits]                  # generous defaults; live-adjustable from the UI
memory_high = "22G"       # soft throttle point
memory_max  = "26G"       # hard backstop, leaves headroom for the session
swap_max    = "6G"        # the surgical lever — caps zram, not RAM
vram_soft   = "10G"

[graceful]                # attempted before SIGTERM at Red
http_post = { url = "http://127.0.0.1:8188/free",
              body = '{"unload_models":true,"free_memory":true}' }
timeout   = "10s"

[expose]
port    = 8188
tailnet = true            # reverse-proxied at /facet/comfyui/
```

Launched via `systemd-run --user --scope` into `prism-<id>.scope`, which gives
delegation, accounting, and atomic kill for free. Limits are written directly to
the cgroup on change, so UI adjustments apply **without restarting the
workload**.

### 4.4 Watchdogs

**ConflictKiller recursion.** Detection, informed by the live incident:

- **any** process matching `qs -p .*killDialog.qml` is already anomalous — the
  dialog is meant to be a singleton the user dismisses, so ≥2 is conclusive
- more than one process matching `qs -c ii`
- layer-shell surfaces per `quickshell:*` namespace exceeding the learned
  baseline (monitors × expected surfaces — 3 monitors here, so >3 `quickshell:bar`
  is definitionally wrong)
- monotonic growth in any of the above across a 10 s window

Because generations arrive every ~3 s at ~300 MB, detection must fire within one
or two generations. This argues for a dedicated fast path polled at 1 Hz
independent of the governor's tier escalation — by the time PSI registers, 3 GiB
is already gone.

The response is cheap and safe enough to run without waiting for confirmation:
reap the `killDialog` processes directly (`pkill -f killDialog.qml`), which
preserves the legitimate `qs -c ii` shell and loses no windows. Only if the main
shell is also unhealthy does Prism escalate to the full keybind remedy.

Prism should additionally assert the *invariant* that `autoKillTrays` and
`autoKillNotificationDaemons` remain `true`, and warn if a dotfiles update
reverts them — that config is the thing standing between a stray `kded6` and a
lost weekend.

Remedy is the known-good keybind, issued through Hyprland so the replacement is
session-owned:

```
hyprctl dispatch exec 'killall ydotool qs quickshell; qs -c ii'
```

Recovery is verified after 15 s. Escalation path is `hyprctl reload`, then a
full Hyprland restart, each gated behind verification. Before acting, the
recorder snapshots `hyprctl layers -j` and quickshell logs — the goal is to
eventually **fix** the recursion, not paper over it forever.

**Hyprland liveness.** IPC socket ping; non-response beyond 30 s is a distinct
failure class with a distinct response.

**Hardware watchdog** *(opt-in, off until verified)*. `sp5100_tco` is not
currently loaded and `/dev/watchdog` does not exist. Once enabled, prismd pets
it from a dedicated `SCHED_FIFO`, `mlockall`'d thread on a generous timeout.
If prismd cannot be scheduled, the board resets itself.

This is the only true answer to a fully wedged machine — nothing running *on* a
wedged box can rescue it. It is also a footgun, and must not be enabled until
unattended boot (autologin → Hyprland → Sunshine → prismd) is verified
end-to-end, or a reboot loop is possible.

### 4.5 Recorder

A continuously-appended, periodically-fsync'd ring buffer: ~10 minutes at 1 Hz,
plus high-fidelity bursts during Amber and above. Survives hard reset. Renders
in the UI as a timeline: *here is the 90 seconds before your machine died.*

Without this, recurring faults get guessed at rather than diagnosed.

**Requirements learned the hard way on 2026-09-04**, when four session losses
produced no usable evidence between them:

1. **The recorder must not live inside what it observes.** Session logs sit on
   tmpfs under `/run/user/1000` and are destroyed when `user@1000.service` stops
   — by the very event under investigation. A mirror started from inside the
   session dies at the same instant. It must be system-scoped.
2. **Stream, do not poll.** The interim mirror copies every 3 s, which can lose
   the final interval — precisely the part that explains a death. `tail -F` per
   instance is strictly better and equally cheap.
3. **Timestamp every captured artefact.** The DRM misdiagnosis came from reading
   *position in a log file* as *proximity in time*. Without explicit timestamps
   carried into the capture, "the last lines in the file" silently becomes "the
   last lines before the failure", and those are not the same claim.
4. **Record Prism's own interventions in the same timeline**, so a later analysis
   can subtract them. See §2.
5. **Harvest kernel-level attribution.** `coredumpctl` metadata on crash, and a
   standing narrowly-scoped audit rule on `kill`/`tkill`/`tgkill` for supervised
   processes. The audit rule is what finally identified the cause on 2026-09-04,
   within four minutes of being installed, after three incidents of userspace
   evidence had pointed everywhere but the truth. It answers "who killed my
   facet?" unambiguously, and no amount of userspace polling can.

The interim tooling now running on `c2` — `auditd` with the `sigwatch` rule,
`hypr-death-watch.service`, `hypr-log-mirror.service` — is a crude prototype of
this section and should be superseded by it, not merely wrapped.

### 4.6 API and access

Bound to the Tailscale interface only. Preferred exposure is `tailscale serve`,
which provides TLS and identity via the `Tailscale-User-Login` header; fallback
is binding the tailnet IP with a bearer token.

- REST for actions and configuration
- WebSocket for the 1 Hz metrics stream
- SSE per-facet log streaming
- Reverse proxy at `/facet/<id>/` so facet UIs are reachable by name rather than
  by remembered port

---

## 5. Roadmap

**Phase 0 — Config hardening** *(done / in progress)*
zram resized to `ram * 0.5`.

**Phase 1 — Survive the weekend**
prismd skeleton, sensors, honest-headroom, governor tiers, facet supervisor with
cgroup scopes, quickshell watchdog, beacon notifications. Headless; CLI only.
This is the milestone that makes the machine safe to leave running.

**Phase 2 — Remote hands**
Tailnet API, WebSocket telemetry, the dashboard, live limit adjustment, log
streaming, facet start/stop. This is the milestone that makes it pleasant.

**Phase 3 — Compute server**
Reverse proxy, OpenAI-compatible inference gateway in front of llama.cpp with
on-demand model load / idle unload / model swap, job queue with completion
notifications.

**Phase 4 — Prevention over cure**
Admission control: estimate a workload's RAM/VRAM footprint before launch
(GGUF size + context + KV cache; ComfyUI graph weights) and refuse or warn when
it will not fit. Most crashes are predictable at launch time.

**Phase 5 — Hardware watchdog**

Blocked on two things, in order.

*First, a kernel parameter.* `c2` boots with **`nowatchdog`** on the cmdline;
there is no `/dev/watchdog`, and no `sp5100_tco` module is loaded. Removing it
requires a bootloader edit **and a reboot**, which is precisely the operation
this document argues against performing remotely.

*Second, verified unattended boot.* Enabling a hardware watchdog before the
autologin → compositor → Sunshine → prismd chain is proven end-to-end converts a
hang into a reboot loop — a strictly worse outcome. `Relogin=true` is now set in
`/etc/sddm.conf.d/kde_settings.conf` and was observed self-healing a session on
2026-09-04, which is real progress, but the full chain is still unproven.

The ordering is therefore: prove the boot chain while physically present →
remove `nowatchdog` in the same visit → only then let prismd pet the device.
Attempting any part of this remotely inverts the risk the watchdog exists to
reduce.

---

## 5a. The shell: Prism as a cloud OS

*Operator direction, 2026-09-04: "more like DSM for Synology than a typical UI.
Maybe Prism OS makes more sense. I'm imagining a cloud OS… the OS layer is driven
by the compute and files of the PC. Literally imagine a cloud PC."*

This is a different product from a dashboard, and the distinction drives the
design: **a dashboard is a page; an OS is a shell.** The health view stops being
the product and becomes one app among several. What ships is a windowing
environment whose applications are backed by the host's real compute, processes
and filesystem.

### Apps, not pages

| App | Backed by |
|---|---|
| **Vitals** | §4.1 sensors — honest headroom, tiers, sparklines |
| **Facets** | §4.3 supervisor — start/stop, live limit sliders, logs |
| **Files** | §6 file manager — browse, preview, transfer |
| **Timeline** | §4.5 recorder — incident playback |
| **Settings** | host config, profiles, auth enrolment |

Registration mirrors facets deliberately: an app is a manifest entry, so adding
one is dropping a file rather than editing the shell. The operator has said they
intend to expand the OS's actions over time, and that only stays cheap if the
shell never needs to know what its apps are.

### Constraints, in tension

**Beautiful, lightweight, and windowed** do not naturally coexist — DSM-class
shells are typically heavy. This holds only under discipline:

- no UI framework; a hand-written window manager (drag, resize, z-order, snap,
  minimise/maximise) is a few hundred lines and has no runtime cost
- apps as inline DOM modules, not iframes — iframes buy isolation Prism does not
  need and cost memory and styling coherence it does
- everything embedded in the binary via `rust-embed`, no build step, no CDN
- the shell must stay usable at Red tier, when the host is already struggling;
  it is a rescue interface before it is a desktop

**The desktop metaphor is wrong on a phone**, but phones are not the target.

*Operator direction: "the main recipient of the best cloud OS experience [is] a
laptop or a keyboard-attached tablet… other PCs and laptops get the best full
experience. But phones will also benefit from file access or use c2 as a media
server to stream a video to the phone."*

So this is **desktop-first**, not responsive-equal. The windowed shell is the
real product and should be designed for a pointer and a keyboard without
compromise. Touch gets a deliberately reduced shell serving the two verticals
that are genuinely valuable on a small screen — **Files** and **Media** — rather
than a squeezed desktop nobody enjoys.

| Device | Shell | Scope |
|---|---|---|
| Laptop, desktop, keyboard-attached tablet | Full windowed OS | Everything. The design target. |
| Phone, bare tablet | Single-app fullscreen, app-switcher | Files, Media, and a read-only Vitals glance |

Apps must therefore declare whether they have a touch presentation at all, and
must not assume they own a window. An app with no touch mode simply does not
appear in the phone launcher — better than shipping one that technically renders
and is miserable to use.

**Media is a first-class app**, added on this direction: `c2` as a media server
streaming video to a phone. It is not part of the resilience story, but it is a
large part of what makes the machine worth reaching from London, and it shares
the file manager's plumbing — roots, previews, and HTTP range requests are the
same machinery.

### What makes it a *cloud* PC rather than a web page

- **Session state lives on the host.** Window positions, open apps and layout
  persist server-side, so the desktop follows the operator between phone,
  laptop and tablet.
- **Apps are views onto real resources**, not onto a database. Files is the
  filesystem; Facets is cgroups; Vitals is PSI.
- **It survives the machine it manages.** The shell must degrade gracefully when
  the compositor is dead, the GPU is wedged, or memory is exhausted — the states
  in which it matters most. A shell that needs a healthy host is decoration.

---

## 6. Visual language

Prism refracts state into spectrum. Hue is **semantic, never decorative** — it
encodes governor tier (green → amber → red) and nothing else. Dark near-black
substrate, one accent per state, generous negative space.

Numerals are the interface: tabular figures, no layout shift as values change.
Honest-headroom is the hero element. Sparklines carry history without chartjunk.
Facet cards show live memory and swap as paired bars, with the swap bar visually
distinct — it is the one that predicts death.

It should read as an instrument, not an admin panel.
