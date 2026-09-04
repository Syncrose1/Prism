# Incident 3: `kill(-1, SIGTERM)` — Prism's own test suite was ending the session

**Date:** 2026-09-04
**Host:** `c2` / <host>
**Sessions lost to this cause:** 21:14:00, 21:22:21, 21:32:41, 21:40:55 (at least)
**Root cause identified:** 21:40:56, by kernel audit
**Author:** Claude (Opus 5), `c1` session

**Status:** **Confirmed by direct kernel evidence.** Not inference.

**Companions:**
[`2026-09-04-session-loss.md`](./2026-09-04-session-loss.md) (incident 1 — a genuine, unrelated Hyprland segfault),
[`2026-09-04-second-session-loss.md`](./2026-09-04-second-session-loss.md) (incidents 2–3 as understood at the time),
[`2026-09-04-drm-capture-root-cause.md`](./2026-09-04-drm-capture-root-cause.md) (**superseded — see §5**).

---

## 1. The finding

A unit test in `prismd` calls `kill(-1, SIGTERM)`, which sends `SIGTERM` to
**every process the user can signal**. Each `cargo test` run therefore terminated
Hyprland, quickshell, hypridle, kitty, Sunshine and syncthing — the entire
graphical session — as a side effect of running the test suite.

Kernel audit record, captured at the moment of death:

```
type=SYSCALL audit(1788554455.715:587): arch=c000003e syscall=62 success=yes exit=0
  a0=ffffffff  a1=f  ses=93  ppid=2708974  pid=2709027
  comm="action::tests::"
  exe="/home/raahats/Prism/target/debug/deps/prismd-ffc37b2774da3533"
  key="sigwatch"
```

- `syscall=62` → `kill()`
- `a0=ffffffff` → first argument **`-1`**
- `a1=f` → signal **15** (`SIGTERM`)
- `ppid` → `cargo`
- `exe` → prismd's own test binary

## 2. The bug

`crates/prismd/src/action.rs`:

```rust
fn signal(pid: u32, sig: i32) {
    let rc = unsafe { libc::kill(pid as libc::pid_t, sig) };   // ← here
    ...
}

#[test]
fn terminate_reports_already_dead_pids_as_gone() {
    let gone = terminate(&[u32::MAX], Duration::from_millis(50));
    assert_eq!(gone, vec![u32::MAX]);
}
```

The test passes `u32::MAX` intending "a pid that cannot exist". But `pid_t` is
`i32`, and the cast wraps:

```
u32::MAX        = 4294967295 = 0xFFFFFFFF
as libc::pid_t  = -1
kill(-1, SIGTERM)
```

Per `kill(2)`, `pid == -1` means *"send sig to every process for which the
calling process has permission to send signals, except process 1"*.

**The asymmetry that hid it.** The sibling test `impossible_pid_is_not_alive`
uses the same `u32::MAX` and is completely harmless, because `alive()` only reads
`/proc/4294967295/stat` and fails cleanly. The same constant is safe in one
function and catastrophic in the other. A reader checking "is `u32::MAX` used
safely here?" would find a reassuring precedent two functions away.

## 3. Why this was so hard to see

Every property of the failure was explained by SIGTERM, and every property
pointed away from a crash:

| Observation | Why `kill(-1, SIGTERM)` explains it |
|---|---|
| No Hyprland coredump | SIGTERM is a clean, catchable termination — nothing to dump |
| **Compositor log ends mid-operation, no shutdown message** | Hyprland was told to quit; it did not fail, and its remaining writes never flushed |
| `sddm-helper exited successfully` | `start-hyprland`'s child exited normally, so the wrapper returned 0 |
| Everything in the session died together | The signal went to *every* process owned by uid 1000 |
| No OOM, no GPU fault, no memory pressure | There was no resource problem at any point |
| ~6–9 minute intervals | The cadence of a developer agent running `cargo test` |
| Silent journal beforehand | Nothing was wrong until the instant everything was signalled |

**The investigation's central error was treating the interval as a property of
the machine.** Idle timers, GPU faults and memory pressure were each eliminated
on the evidence — correctly — but the search space never included *another agent's
ordinary development loop*. The variable was the workload, not the host.

## 4. How it was caught

`auditctl` rule on the signal-sending syscalls, plus a system-scoped watcher that
dumps forensics the moment the compositor PID disappears:

```
-a always,exit -F arch=b64 -S kill -S tkill -S tgkill -k sigwatch
```

```
/usr/local/bin/hypr-death-watch.sh        polls for the Hyprland pid; on loss,
/etc/systemd/system/hypr-death-watch.service   writes audit + journal + dmesg + memory
                                               state to /var/log/hypr-crash-logs/death-*.txt
```

It caught the cause on the **first real death after installation** — 21:40:55,
roughly four minutes after the rule went in.

One implementation note worth carrying: `ausearch -k sigwatch` did **not** match
these records even though the rule was loaded and firing correctly. The records
were only visible by reading `/var/log/audit/audit.log` directly and grepping
`syscall=62|200|234`. A tool that silently returns `<no matches>` while the data
sits in the log is a trap; the watcher was rewritten to read the log directly and
to decode `a0`/`a1` into "signal N sent to PID by PID".

## 5. This supersedes the DRM-capture analysis

`2026-09-04-drm-capture-root-cause.md`, written on `c2` between session deaths,
concluded that **Moonlight connections were killing the session** via Sunshine's
KMS capture contending with the compositor for DRM. That document should be read
as superseded, and it deserves a fair account of why it was reasonable:

- Its evidence was **real**. The dead session's log genuinely contains
  `drm: Got a lease event for /dev/dri/card1` and five
  `Cannot commit when a page-flip is awaiting` errors, at lines 253–292 of 300 —
  genuinely near the end of the file.
- Its causal story was coherent, mechanistically plausible, and it was explicit
  about resting on single-sample correlation.
- **It included a falsification protocol (§6) and invited the test.** The audit
  record is precisely that test, and the theory fails it.

Two things it could not have known:

1. **Its own test suite was the trigger.** The `cargo test` runs that killed the
   sessions were being launched from that same session. It was investigating a
   fire it was starting.
2. **The log's position is misleading.** The frozen log covers only the session's
   *startup* — device enumeration, modesetting, input-device attachment. The DRM
   errors sit near the end of the *file* because the file stops there: the
   compositor then ran ~9 more minutes writing nothing before being signalled.
   "Near the end of the log" was read as "just before death"; it actually meant
   "the last thing worth logging during startup".

**What survives from it.** The DRM contention is real and worth fixing on its own
merits — Sunshine on a Wayland host genuinely is falling back to
`Screencasting with KMS` because the system unit has no `WAYLAND_DISPLAY`, and
that genuinely does produce lease events and page-flip errors. Its §5 fix (user
unit bound to `graphical-session.target` with the session environment imported)
remains the right change. It fixes a real defect. It was not, however, causing
the session deaths.

Its §7 lessons also stand, particularly:
- a capability probe should name the mechanism (`Screencasting with KMS` is a red
  state even when every port is listening);
- the recorder should capture operator actions, not just system metrics;
- instrumentation paid for itself within ten minutes.

Its §8 critique of the log mirror is accepted: 3-second polling can lose the final
interval, and a `tail -F` stream would be strictly better.

## 6. What Prism should take from this

### 6.1 `signal()` needs a guard, in production code and not only in tests

This is the highest-value item in any of the three reports. Prism is a daemon
whose *purpose* is terminating processes. A pid that reaches `kill()` as `<= 0`
does not kill a process — it kills a process **group**, or at `-1`, the whole
user session. That is the exact catastrophe Prism exists to prevent, reachable
from a single arithmetic slip anywhere upstream: a config parse, an attribution
result, a cgroup scan returning an empty set, a `u32`/`i32` boundary.

```rust
fn signal(pid: u32, sig: i32) {
    // A pid that lands on <= 0 after the cast does not target one process:
    // 0 signals our own process group, -1 signals every process we own, and
    // < -1 signals a process group. Prism must only ever signal one specific
    // pid, so refuse anything else rather than trusting the caller.
    let Ok(pid) = i32::try_from(pid) else {
        warn!(pid, "refusing to signal: pid does not fit in pid_t");
        return;
    };
    if pid <= 0 {
        warn!(pid, "refusing to signal: non-positive pid would target a group");
        return;
    }
    let rc = unsafe { libc::kill(pid, sig) };
    ...
}
```

The test should use a pid that is merely *unused* rather than one that wraps —
e.g. a value above `/proc/sys/kernel/pid_max`, or spawn-and-reap a real child and
signal the reaped pid.

Consider also `assert!(pid > 0)` at the boundary of every function that accepts a
pid from outside, and a fuzz/property test asserting that no input to `terminate()`
can produce a non-positive argument to `kill()`.

### 6.2 Prism's blast radius must be bounded by construction

`architecture.md` §4.3 already launches facets into `systemd-run --user --scope`
units, and §4.2 escalates to `cgroup.kill` — *"atomic, whole tree, no orphaned
CUDA workers"*. That design is sound and would have contained this: killing a
cgroup cannot escape to the session.

The lesson is that **the raw-pid path exists alongside the cgroup path**, and the
raw-pid path has no such bound. Where Prism can act via cgroups it should, and the
pid path should be treated as the dangerous fallback it is.

### 6.3 Test suites for a killer daemon need isolation

`cargo test` ran a process-killing library directly in the operator's live
graphical session. Even with §6.1 fixed, this is the wrong place to exercise that
code. Tests that signal anything should run in a container, a user namespace, or a
dedicated scope — something whose blast radius is bounded by the kernel rather
than by the correctness of the code under test.

### 6.4 Correlation under self-influence

Both agents investigating tonight reached confident, well-evidenced, wrong
conclusions before the audit rule existed:

- `c1` (this author) attributed the 21:14 loss to its own SSH sessions, then had
  to retract it on finding `KillUserProcesses=false`.
- `c2` built the DRM-capture theory while its own test suite was the trigger.

Neither error was careless; both followed the available evidence. The common
failure is **an observer inside the system it is measuring**, unable to see its own
contribution. For Prism this is not an abstract concern — it is a daemon that will
act on a machine and then observe the results of its own actions. The recorder must
therefore log *Prism's own interventions* into the same timeline as system events,
so that a future analysis can subtract them.

### 6.5 Kernel-level attribution is worth having before you need it

Three incidents produced no usable evidence. The audit rule produced a definitive
answer within four minutes of being installed, because it observes at a layer the
failure cannot erase. `recorder/` should consider a standing, narrowly-scoped audit
rule on signal delivery to supervised processes — the cost is negligible and it
answers "who killed my facet?" unambiguously, which no amount of userspace polling
can.

---

## 7. Immediate actions

| Action | Status |
|---|---|
| Root cause identified via kernel audit | **Done** — 21:40:56 |
| Session restored (`systemctl restart sddm`) | **Done** — Hyprland on tty1 |
| Sunshine restarted (`reset-failed` + `restart`) | **Done** — active, 0 DRM errors |
| Audit rule persisted (`/etc/audit/rules.d/sigwatch.rules`), auditd enabled | **Done** |
| `hypr-death-watch.service` installed and enabled | **Done** |
| `hypr-log-mirror.service` installed and enabled | **Done** |
| **Fix `signal()` guard + the test** | **Not done — code change, for the operator** |
| Convert `sunshine.service` to a user unit | **Not done — recommended, from DRM report §5** |

`Relogin=true` was set in `/etc/sddm.conf.d/kde_settings.conf` (backup:
`kde_settings.conf.bak-20260904`) and is confirmed working — session 93 autologged
in unattended on tty3 after the 21:32 death.

Note for whoever fixes the test: **do not run `cargo test` on `prismd` in the live
graphical session until §6.1 is applied.** It will end the session again.

---

## 8. Environment

```
Host          <host> (c2), 100.x.x.x
Audit         auditd 4.x, rule key "sigwatch", CONFIG_AUDITSYSCALL=y
              note: kernel cmdline contains `nowatchdog` — relevant to
              architecture.md §4.4's hardware-watchdog plans
Killer        /home/raahats/Prism/target/debug/deps/prismd-ffc37b2774da3533
              test module `action::tests`, launched by cargo (ppid 2708974)
Victims       Hyprland, quickshell(qs), hypridle, hyprsunset, kitty, syncthing,
              Sunshine, wl-paste, pipewire — everything owned by uid 1000
Sessions lost 21:14:00, 21:22:21, 21:32:41, 21:40:55
```

---

## Sign-off

Three incident reports, two agents, and four session losses, resolved by a
four-line audit rule and one hex value: `a0=ffffffff`.

The uncomfortable symmetry is worth stating plainly. Incident 1 found that Prism's
existing mitigations worked and the machine became unreachable anyway. Incident 2
found that the *repair* caused the next outage. This one closes the loop: **Prism's
own test suite was the thing taking the machine down**, while both agents
investigating built careful, evidence-led, incorrect theories about why — because
neither could see its own hand in the data.

For a project whose first stated goal is *"Never let the machine become
unreachable"*, there is a lesson in having spent an evening being the reason it
was. The code that ships should assume the same of itself: bound the blast radius
in the kernel, log your own actions into the timeline, and never trust a pid you
did not validate.

— **Claude (Opus 5)**, sentinel of `c1`
  2026-09-04, London — with credit to the `c2` session, which did good work with
  bad luck, and wrote down a falsifiable theory rather than a comfortable one
