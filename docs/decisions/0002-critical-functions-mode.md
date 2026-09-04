# ADR 0002: Critical Functions Mode — de-escalation instead of permanent restraint

**Date:** 2026-09-05
**Status:** Accepted
**Supersedes:** the "shell must stay usable at Red tier" constraint in
`architecture.md` §5a, and part of the reasoning in [ADR 0001](./0001-shell-and-file-architecture.md).

---

## Context

`architecture.md` §5a required that the rich shell itself remain usable at Red
tier. That is a permanent tax: every decision in the desktop is hostage to the
worst moment the machine will ever have, so nothing can afford to be good.

**Operator proposal, 2026-09-05:** *"a slightly heavier (but still lightweight)
cloud OS, that detects impending doom and de-escalates itself into 'Critical
Functions Mode' — the most stripped back Prism core that can handle the
situation and let you do it remotely (particularly useful if the display goes
down). Once resolved, it can gracefully self-escalate to Prism OS."*

This is the better structure and is adopted.

## Decision

Ship **two independent interfaces**, not two modes of one application.

| | Prism OS | Critical Functions Mode |
|---|---|---|
| Served at | `/` | `/rescue` |
| Purpose | the desktop | keep the machine recoverable |
| JavaScript | yes | **none** |
| Depends on | API, WebSocket, shell bundle | one HTML response |
| Available | Green / Amber | **always, at every tier** |

The governor's tier drives which is served at `/`. But `/rescue` is served
unconditionally, forever, regardless of tier or of whether the rich shell exists.

### 1. Two artefacts, not two modes

If Critical mode were a state inside the rich shell's bundle, a shell that fails
to load leaves the operator with nothing. They must share no code path. The
rescue page is generated server-side and served as a single self-contained HTML
response with no fetches of any kind.

### 2. Manual access must always work

Automatic de-escalation is a convenience. **The failure that cannot be
anticipated is the one where the detector itself is wrong** — a wedged
compositor with healthy memory, a hung facet with green tiers, a bug in the
governor. `/rescue` therefore has no tier gate: it is reachable when Prism
believes everything is fine, because Prism's belief is exactly what is in
question.

This is the same lesson as the 2026-09-04 incidents, where every conventional
health check reported success throughout a 19-minute outage.

### 3. No JavaScript in rescue

Plain HTML forms performing `POST`s. Rationale:

- it renders on a browser that is itself struggling for memory
- it works in `w3m`/`lynx` over SSH, which was the *only* surviving access path
  during the first incident
- it works on an old phone, on a bad connection, with a broken bundle
- there is no client-side state to get wrong at the moment it matters most

Zero JavaScript is the only way to be confident the page renders at all.

### 4. The server de-escalates too

De-escalation is not merely cosmetic. At Red and above, `prismd`:

- refuses thumbnail generation and transcoding
- stops WebSocket telemetry fan-out, falling back to on-demand polling
- prioritises rescue routes
- keeps the storm detector and governor running at full rate

Otherwise the interface de-escalates while the backend is still transcoding a
film, which is the wrong half.

### 5. Self-escalation is automatic but never automatic *downward* in capability mid-action

Returning to Green restores Prism OS. Escalation should not interrupt an
in-flight rescue action — if the operator is mid-kill, the page does not
reload underneath them.

## What Critical Functions Mode contains

Nothing that is not needed to recover a machine:

- current tier, and **which signal drove it** (`Driver` from the governor)
- honest headroom, disk free, load — as text
- top memory consumers, with a kill button each
- registered facets, with stop buttons (via `cgroup.kill`)
- the known-good compositor remedy (`hyprctl dispatch exec …`)
- last N lines of the recorder
- reboot, behind a confirmation

All of it as forms. No charts, no fonts, no images.

## Consequence for ADR 0001

ADR 0001 rejected adopting Puter primarily because *"the shell must work at Red
tier"*. **That argument is weakened by this ADR** — it is now the *rescue* that
must work at Red, not the shell, and a rescue page is small enough to build
regardless.

The decision to build does not change, but the primary reason does, and the
honest one is better: **Puter stores files in its own storage abstraction**,
whereas the requirement here is a view onto the host's real filesystem — *"the
cloud is another PC"*. Adopting it would mean fighting it for the core use case,
which is a stronger objection than the runtime one originally led with.

The runtime and disk arguments still stand, but as supporting reasons.

## Consequence for the rich shell

The permanent constraint is lifted. Prism OS may now use whatever it needs to be
good — a real window manager, transitions, thumbnails, live charts — because it
is no longer the thing that has to survive an emergency. "Lightweight" remains a
goal; it stops being a hard safety requirement.

## Build order

Rescue first. It is the floor, it is small, and everything else layers above it.
Building the desktop first would mean the safety-critical artefact is the one
written last and least carefully.
