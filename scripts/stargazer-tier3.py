#!/usr/bin/env python3
"""Tier 3 — the shell you actually live in.

Tier 2 put the identity on the sign-in screen, which is the screen you pass
through in two seconds. The desktop behind it — windows, topbar, dock, the thing
you are looking at for the rest of the session — kept its own shape. A retheme
that only lands on a page nobody dwells on is a retheme nobody sees, and the
report back was, correctly, "it looks the same as before."

So this is the same three marks, applied where they are actually read:

  * **Windows are chamfered and bracketed**, and the brackets are always there
    rather than appearing on focus. Prism already had corner brackets — they
    were rounded, invisible until focus, and static. Cutting them, showing them
    and letting them breathe is most of the change.
  * **A lit top edge** on the focused window, which is what says *this one*
    without a colour change or a heavier border.
  * **The dock and the topbar are cut too**, because a rounded dock item beside a
    chamfered window is two products in one screenshot.

Run after retheme and tier2:

    python3 scripts/stargazer-retheme.py ui/shell.html
    python3 scripts/stargazer-tier2.py   ui/shell.html
    python3 scripts/stargazer-tier3.py   ui/shell.html
"""
import sys
from pathlib import Path

p = Path(sys.argv[1])
t = p.read_text()

CHAMFER = """clip-path: polygon(
    var(--chamfer) 0, calc(100% - var(--chamfer)) 0, 100% var(--chamfer),
    100% calc(100% - var(--chamfer)), calc(100% - var(--chamfer)) 100%,
    var(--chamfer) 100%, 0 calc(100% - var(--chamfer)), 0 var(--chamfer));"""

# ---------------------------------------------------------------------------
# Windows: cut, not rounded
# ---------------------------------------------------------------------------
OLD_WIN = """  border: 1px solid var(--line);
  border-radius: 7px;
  box-shadow: 0 26px 64px -18px rgba(0,0,0,.9), 0 0 0 1px rgba(0,0,0,.5);"""
NEW_WIN = """  border: 1px solid var(--line);
  /* CUT, not rounded. A 7px radius is a perfectly good radius and it belongs to
     a different product. The chamfer is the single mark that makes a window
     read as the System's before anything in it is read at all. */
  border-radius: 0;
  """ + CHAMFER + """
  box-shadow: 0 26px 64px -18px rgba(0,0,0,.9), 0 0 0 1px rgba(0,0,0,.5);"""
assert OLD_WIN in t, ".win rule not found"
t = t.replace(OLD_WIN, NEW_WIN, 1)

# The maximised case had `border-radius: 0` as its whole special-casing; now it
# has to drop the chamfer too, because a full-screen window with its corners
# taken off is a window with holes in the corners of the screen.
OLD_MAX = ".win.max { border-radius: 0; }"
NEW_MAX = """/* Maximised: no chamfer. A full-screen surface with its corners cut leaves
   four notches of desktop showing through, which reads as a rendering fault. */
.win.max { border-radius: 0; clip-path: none; }"""
assert OLD_MAX in t
t = t.replace(OLD_MAX, NEW_MAX, 1)

# ---------------------------------------------------------------------------
# Brackets: cut, always present, breathing
# ---------------------------------------------------------------------------
OLD_BRK = """.win.focus .bracket { opacity: .5; }
.bracket { position: absolute; width: 9px; height: 9px; pointer-events: none; border-color: var(--tier); opacity: 0; transition: opacity .25s, border-color .5s; }
.bracket.tl { top:-1px; left:-1px;  border-top:1.5px solid; border-left:1.5px solid;  border-radius:7px 0 0 0; }
.bracket.tr { top:-1px; right:-1px; border-top:1.5px solid; border-right:1.5px solid; border-radius:0 7px 0 0; }
.bracket.bl { bottom:-1px; left:-1px;  border-bottom:1.5px solid; border-left:1.5px solid;  border-radius:0 0 0 7px; }
.bracket.br { bottom:-1px; right:-1px; border-bottom:1.5px solid; border-right:1.5px solid; border-radius:0 0 7px 0; }"""

NEW_BRK = """/* Corner brackets — the System's second mark after the chamfer.
   
   These existed already and were invisible: rounded, `opacity: 0` until the
   window took focus, and static. Three changes, and all three are the point.
   
   They are CUT to match the corner. They are always present, because a bracket
   that only appears on focus is a focus indicator rather than an identity — the
   focused window gets a brighter one instead. And they BREATHE, on 6.7s, which
   divides into none of the other periods on screen, so a desktop of windows
   never pulses in unison and nothing on it is visibly a loop. */
.bracket {
  position: absolute; width: 14px; height: 14px; pointer-events: none;
  border-color: var(--tier); opacity: .30;
  transition: opacity .25s, border-color .5s;
  animation: brkbreathe 6.7s ease-in-out infinite;
}
.win.focus .bracket { opacity: .85; }
@keyframes brkbreathe { 0%,100% { filter: brightness(.78) } 50% { filter: brightness(1.15) } }
.bracket.tl { top:-1px; left:var(--chamfer);  border-top:2px solid; }
.bracket.tl2 { top:var(--chamfer); left:-1px; border-left:2px solid; }
.bracket.tr { top:-1px; right:var(--chamfer); border-top:2px solid; }
.bracket.tr2 { top:var(--chamfer); right:-1px; border-right:2px solid; }
.bracket.bl { bottom:-1px; left:var(--chamfer);  border-bottom:2px solid; }
.bracket.bl2 { bottom:var(--chamfer); left:-1px; border-left:2px solid; }
.bracket.br { bottom:-1px; right:var(--chamfer); border-bottom:2px solid; }
.bracket.br2 { bottom:var(--chamfer); right:-1px; border-right:2px solid; }
.win.max .bracket { display: none; }

/* The lit edge on the focused window. This is what says THIS ONE — cheaper to
   read than a colour change and quieter than a heavier border. */
.win.focus > .litedge {
  content: ""; position: absolute; left: var(--chamfer); right: var(--chamfer);
  top: 0; height: 1.6px; pointer-events: none;
  background: linear-gradient(90deg, transparent, var(--lit) 30%, var(--lit) 72%, transparent);
  opacity: .85;
}
.litedge { position: absolute; opacity: 0; }"""
assert OLD_BRK in t, "bracket rules not found"
t = t.replace(OLD_BRK, NEW_BRK, 1)

# The markup: four brackets become eight arms, plus the lit edge.
OLD_MK = """`<i class="bracket tl"></i><i class="bracket tr"></i><i class="bracket bl"></i><i class="bracket br"></i>"""
NEW_MK = """`<i class="litedge"></i>` +
    `<i class="bracket tl"></i><i class="bracket tl2"></i>` +
    `<i class="bracket tr"></i><i class="bracket tr2"></i>` +
    `<i class="bracket bl"></i><i class="bracket bl2"></i>` +
    `<i class="bracket br"></i><i class="bracket br2"></i>"""
assert OLD_MK in t, "bracket markup not found"
t = t.replace(OLD_MK, NEW_MK, 1)

# ---------------------------------------------------------------------------
# Dock and topbar: cut too
# ---------------------------------------------------------------------------
OLD_DOCK = """.dockitem {
  position: relative; width: 42px; height: 42px; border-radius: 9px;"""
NEW_DOCK = """/* A rounded dock item beside a chamfered window is two products in one
   screenshot. 6px rather than the window's 12: the chamfer should read as
   proportional to the surface, and at 42px square a 12px cut is an octagon. */
.dockitem {
  position: relative; width: 42px; height: 42px; border-radius: 0;
  clip-path: polygon(6px 0, calc(100% - 6px) 0, 100% 6px, 100% calc(100% - 6px),
    calc(100% - 6px) 100%, 6px 100%, 0 calc(100% - 6px), 0 6px);"""
assert OLD_DOCK in t, ".dockitem not found"
t = t.replace(OLD_DOCK, NEW_DOCK, 1)

# ---------------------------------------------------------------------------
# The tier, in the System's language
# ---------------------------------------------------------------------------
#
# The governor's tiers are named green/amber/red/black, and that is the right
# name in the code — it is a domain model, not a palette. But the indicator now
# reads "GREEN" beside a cyan light, which is the retheme announcing its own
# seam.
#
# So the DISPLAY label is mapped, and the domain model is not touched. The words
# are the ones this system already uses elsewhere: the guard says "clear" when
# it will not hold, and "breach" is what a violated clause is called. A machine
# that is fine is CLEAR; one that is about to stop being reachable has BREACHED.
OLD_TIER = """  $('#tierWord').textContent = t[0].toUpperCase() + t.slice(1);"""
NEW_TIER = """  // The tier's NAME stays green/amber/red/black in the governor, the API and
  // the timeline — it is a domain model and renaming it would churn every
  // consumer. Only the word on screen changes, because "GREEN" beside a cyan
  // light is the retheme showing its own seam.
  const TIER_WORD = { green: 'Clear', amber: 'Strain', red: 'Critical', black: 'Breach' };
  $('#tierWord').textContent = TIER_WORD[t] || (t[0].toUpperCase() + t.slice(1));"""
assert OLD_TIER in t, "tier word line not found"
t = t.replace(OLD_TIER, NEW_TIER, 1)

# The window buttons are circles. Leave them: a traffic light is a convention
# older than this design language, and cutting three 11px dots gains nothing
# and costs recognisability.

p.write_text(t)
print(f"tier 3 applied to {p}")
