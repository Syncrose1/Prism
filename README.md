# Prism

A resilience layer and remote desktop for a Linux workstation that runs heavy
local AI workloads and gets operated from somewhere else for days at a time.

Prism exists because a machine kept becoming unreachable. Its first job is that
this stops happening; everything else is what makes the machine worth reaching.

```sh
git clone https://github.com/Syncrose1/Prism.git ~/Prism
cd ~/Prism && ./scripts/install.sh
```

One command: builds, detects what the machine has, enrols an authenticator,
sets a password, and starts as a systemd user service. No root required.

---

## What it does

**Keeps the machine reachable.** A governor watches memory-stall pressure, real
memory headroom and free disk, and intervenes before the kernel has to — first
by warning, then by asking a workload to shed memory, finally by killing its
cgroup outright. Workloads run in transient systemd scopes, so termination is
atomic across the whole process tree and limits can be changed while they run.

**Notices runaway processes.** A generic storm detector matches command lines by
pattern and fires on either count or spawn *rate*. Rate is the useful signal: a
pool of forty workers is normal, forty processes that did not exist ninety
seconds ago is not.

**Degrades instead of dying.** Under pressure it suspends thumbnailing and
transcoding, shrinks terminal scrollback, and refuses to start new workloads.
A zero-JavaScript rescue page at `/rescue` is served unconditionally at every
tier, shares no code with the main interface, and renders in a text browser over
SSH.

**Is a workstation you can use.** A windowed desktop in the browser: real
terminals on real PTYs, a file manager over your actual filesystem, a gallery
that treats a directory as one group so arrow keys walk images *and* video, live
system vitals, and a timeline of everything Prism observed and did.

## The metric that matters

On a machine with compressed swap, the kernel's own accounting is misleading.
`SwapFree` counts capacity that consumes RAM when used, so swapping out `X` bytes
frees `X` but costs `X/ratio` to store:

```
honest_headroom = MemAvailable + SwapFree × (1 − 1/compression_ratio)
```

Sized at 1.5× RAM, zram advertises headroom that cannot exist. The OOM killer
never fires because free swap remains, so nothing is shed and the machine
thrashes indefinitely instead. Prism computes the corrected figure from the live
compression ratio and keys every threshold on it. No standard tool shows this
number, and on the machine Prism was written for it explained three unrecoverable
hangs.

## Design

Written down as decisions rather than folklore:

| | |
|---|---|
| [`docs/architecture.md`](docs/architecture.md) | The system, its failure modes, and the principles |
| [`docs/decisions/`](docs/decisions/) | Why the shell is built rather than adopted, why rescue is a separate artefact, how terminal sessions work |
| [`docs/incidents/`](docs/incidents/) | Four real outages, diagnosed, and what each changed |

The incident reports are the most instructive material here. One of them is Prism
killing the operator's session four times through a `kill(-1)` in its own test
suite — which is why `safety.rs` exists, why the suite runs in a PID namespace,
and why the design prefers `cgroup.kill` to signalling pids.

Principles that earned their place the hard way:

- **Bound the blast radius by construction.** Prism kills processes for a living,
  so it is one bad pid from being the outage. Every signal passes a guard that
  refuses process groups, init, itself, its ancestors, and anything reachability
  depends on.
- **Every intervention is verified, and the verification reported.** A rescue
  that reports success and checks nothing is worse than none: it produces an
  outage wearing the costume of a fix.
- **Log your own actions into the timeline you read.** An observer inside the
  system it measures cannot see its own contribution.

## Security

Bound to the Tailscale interface, never a wildcard, so the network boundary and
the auth boundary fail independently.

Authentication is two factors without a timer: an authenticator code enrols a
browser once, a password unlocks it thereafter. The device token authorises
nothing on its own, so a stolen laptop still needs the password. Codes are
single-use within their step; wrong passwords and wrong codes share one lockout.

File access is confined to configured roots, resolved through `canonicalize` and
then checked for containment — so a symlink pointing out of a root resolves to
its target and is refused. "Outside the root" and "does not exist" return the
same error, since distinguishing them would be a filesystem oracle.

## Requirements

Linux, with cgroup v2 delegated to the user slice, systemd, and a Rust
toolchain. `vips`, `ffmpeg`, `pdftoppm` and `qrencode` are optional and only
affect previews and enrolment display.

Prism is Linux-only and honest about it: its resilience layer rests on cgroups,
PSI and zram accounting, none of which have direct equivalents elsewhere.
[`platform/mod.rs`](crates/prism-core/src/platform/mod.rs) defines the seams a
port would implement, and states plainly what such a port would not get for free.

## Development

```sh
./scripts/test-isolated.sh
```

The suite runs inside a PID namespace. Prism terminates processes and its tests
exercise that code, so the blast radius is bounded by the kernel rather than by
the correctness of the code under test. The script also syntax-checks the shell
and verifies every registered app is defined — both after bugs that left the
page rendering and inert.

## Licence

MIT. See [`LICENSE`](LICENSE).
