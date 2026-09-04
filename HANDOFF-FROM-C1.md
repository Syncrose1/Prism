# Handoff: c1 → c2

**From:** Claude (Opus 5), `c1` session — <laptop>
**To:** Claude (Opus 5), `c2` session
**Written:** 2026-09-04 21:50 BST
**Context:** the four session losses tonight, now root-caused and fixed.

Passed on verbatim by the operator, who is converging the two sessions. Some of
this is phrased to them rather than to you — read past that.

---

All green. Only the one death file, from before the fix.

## Safe to leave — and it's actually fixed, not just healing

Your assumption was reasonable but incomplete: `Relogin=true` heals the login,
but wouldn't have stopped the killing. Any `cargo test` run while you were out
would have SIGTERMed the session again, and you'd have come back to a machine
that had been cycling. So I fixed the cause rather than relying on the recovery.

**The guard is applied and verified.** `signal()` in
`crates/prismd/src/action.rs` now refuses any pid that isn't a single positive
value:

```rust
let pid: libc::pid_t = match i32::try_from(pid) {
    Ok(p) if p > 0 => p,
    _ => { warn!(pid, sig, "refusing to signal: pid is not a single positive pid"); return; }
};
```

The test now uses `0x7FFF_FFF0` (unused but valid) instead of `u32::MAX`, and
there's a new regression test asserting that `u32::MAX`, `0x8000_0000` and `0`
all get refused.

**Proven:** `cargo test -p prismd` — 5 passed, and Hyprland kept PID 2710314
through it. The exact command that killed your session four times is now
harmless. Backup at `action.rs.bak-20260904`.

## Reports

Four documents in `~/Prism/docs/incidents/`:

- `2026-09-04-session-loss.md` — the 19:30 Hyprland segfault (genuine, unrelated)
- `2026-09-04-second-session-loss.md` — the recovery-induced outage
- `2026-09-04-drm-capture-root-cause.md` — c2's report, now banner-marked SUPERSEDED
- `2026-09-04-third-kill-minus-one.md` — the real cause, from kernel audit

On c2's report: it was good work with bad luck. Real evidence, coherent
mechanism, honest about single-sample causality — and it wrote a falsification
protocol in §6 asking someone to test it. The audit record is that test, and it
failed. It couldn't have known its own test suite was the trigger, or that the
DRM errors sat at the end of the file because logging stopped after startup, not
because they preceded the death. I preserved what survives: the
`WAYLAND_DISPLAY`/KMS defect is real and its §5 user-unit fix is still the right
change.

The sharpest lesson for Prism, in §6.4: both agents reached confident,
well-evidenced, wrong conclusions — I blamed my own SSH sessions, c2 blamed
Moonlight — because each was inside the system it was measuring. For a daemon
that acts on a machine and then observes the results, that's a design
constraint: log your own interventions into the same timeline so a later
analysis can subtract them.

## Still running while you're out

`auditd` + sigwatch rule, `hypr-death-watch`, `hypr-log-mirror` — all enabled at
boot. If anything dies while you're away, forensics land in
`/var/log/hypr-crash-logs/death-*.txt` automatically.

## Two things left for you

- **Sunshine as a user unit** bound to `graphical-session.target` — fixes the
  real KMS/`WAYLAND_DISPLAY` defect c2 found. Not done; it changes streaming
  behaviour.
- **Tailscale SSH** — still deferred, still worth doing once you're confident
  things are stable.

Nothing is committed — the repo has no commits yet and everything is untracked,
so the reports and the fix are on disk only. Worth an initial commit when you're
back.

---

## Practical notes for picking this up

- **Do not revert `signal()`'s guard.** It is the fix. If a test fails against
  it, the test is wrong.
- The three instrumentation services (`auditd`, `hypr-death-watch`,
  `hypr-log-mirror`) are enabled at boot and are live investigation tooling.
  Leave them unless you have a reason; `hypr-log-mirror` is a crude prototype of
  what `recorder/` should do properly, and its §8 critique in c2's own report
  (3 s polling loses the final interval; prefer `tail -F`) is accepted and worth
  implementing for real.
- `Relogin=true` is set in `/etc/sddm.conf.d/kde_settings.conf`
  (backup `.bak-20260904`) so the session self-heals rather than parking at the
  greeter.
- `sunshine.service` was changed to `Restart=always`
  (backup `.bak-20260904`). Necessary but not sufficient — see incident 2 §3.4.
- Kernel cmdline carries `nowatchdog`, which matters for `architecture.md`
  §4.4's hardware-watchdog plans.
- If you need c1 again: the operator has SSH from `<laptop>` on key
  `SHA256:<redacted>`.

— Claude (Opus 5), `c1`
