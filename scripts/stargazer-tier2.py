#!/usr/bin/env python3
"""Tier 2 — the chrome, and the body.

Tier 0 gave Prism the identity's colours. That was necessary and it is not
sufficient: a palette swap makes a thing *tinted*, not *recognisable*. What
makes a surface read as the System is geometry and motion — a cut corner rather
than a rounded one, brackets that breathe, a line of light that draws a thing
open, and the constellation itself.

So this pass adds:

  * **The chamfer.** The System's surfaces are cut, not rounded. `--r: 4px` was
    a perfectly good radius and it belongs to a different product.
  * **Corner brackets** on the sign-in card, on a period that does not divide
    into any of the others.
  * **The constellation**, as Prism's third renderer, on the sign-in screen —
    which is the first thing seen every time and the right place for the body
    to be waiting.
  * **The display face** for the few places Prism actually declaims. Everywhere
    else stays monospace, because Prism is a console and that is not a costume.

## Measuring this, if you ever need to

The sign-in card fades in (`@keyframes rise`, 0.5s). A headless screenshot lands
*mid-fade*, so everything on it — the body included — measures at roughly 40% of
its real brightness. That reads exactly like a renderer that is too dim, and it
cost three rounds of "fixing" a body that was never wrong.

Before measuring, neutralise the animation and probe with a known value:

    ctx.fillStyle = '#fff'; ctx.fillRect(2, 2, 20, 20);

If that swatch does not read (255,255,255), you are not measuring the renderer.
With the fade out of the way the body peaks at ~524 of 765 on its brightest
pixel, against 432 for the QML original and 503 for the Compose port — the same
body, at the same brightness, on all three.

Run after `stargazer-retheme.py`:

    python3 scripts/stargazer-retheme.py ui/shell.html
    python3 scripts/stargazer-tier2.py   ui/shell.html
"""
import re
import sys
from pathlib import Path

p = Path(sys.argv[1])
t = p.read_text()

# ---------------------------------------------------------------------------
# 1. Geometry: cut, not rounded
# ---------------------------------------------------------------------------
# The sign-in column is wider now that the card has padding of its own.
OLD_W = """.lg {
  width: min(340px, 88vw); text-align: center;"""
NEW_W = """.lg {
  width: min(380px, 90vw); text-align: center;"""

OLD_R = "  --r: 4px;"
NEW_R = """  /* The System's surfaces are CUT, not rounded. `--r` stays because a hundred
     rules reference it and a border-radius of 0 is the honest value for a
     chamfered language — the corner is taken off by clip-path where it matters
     and left square where it does not. A 4px radius is a perfectly good radius
     that belongs to a different product. */
  --r: 0px;
  --chamfer: 12px;"""
assert OLD_R in t, "--r not found"
t = t.replace(OLD_R, NEW_R, 1)
assert OLD_W in t, "login column rule not found"
t = t.replace(OLD_W, NEW_W, 1)

# ---------------------------------------------------------------------------
# 2. The sign-in screen: the body, a chamfered card, and brackets
# ---------------------------------------------------------------------------
OLD_LG = """.lg .m { margin-bottom: 22px; display: flex; justify-content: center; }"""
NEW_LG = """.lg .m { display: none; }   /* the body is the mark now */

/* ── The body ────────────────────────────────────────────────────────────
   Prism's third renderer of the constellation. QML draws it on the desktop,
   Compose draws it on the phone, and this draws it here — no shared code
   between them and every value the same, which is the whole bargain of a token
   source.

   It sits on the sign-in screen because that is the first thing seen every
   time, and because a System that is waiting for you should look like it is
   waiting rather than like a form. */
.lg .body { display: block; margin: 0 auto -6px; width: 300px; height: 280px; }

/* The card is CHAMFERED and BRACKETED — the two marks that make a surface the
   System's rather than merely dark. The brackets breathe on a period that
   divides into none of the others, so the composite never returns to where it
   started and nothing here is ever visibly a loop. */
.lg .card {
  position: relative;
  padding: 22px 20px 18px;
  background: linear-gradient(160deg, var(--surface), rgba(11,16,24,.55));
  border: 1px solid var(--line-hi);
  clip-path: polygon(
    var(--chamfer) 0, calc(100% - var(--chamfer)) 0, 100% var(--chamfer),
    100% calc(100% - var(--chamfer)), calc(100% - var(--chamfer)) 100%,
    var(--chamfer) 100%, 0 calc(100% - var(--chamfer)), 0 var(--chamfer));
}
/* The lit edge: the top of a System surface catches light. */
.lg .card::before {
  content: ""; position: absolute; left: var(--chamfer); right: var(--chamfer); top: 0;
  height: 1.6px;
  background: linear-gradient(90deg, transparent, var(--lit) 30%, var(--lit) 72%, transparent);
  opacity: .9;
}
.lg .brk { position: absolute; inset: 0; pointer-events: none; }
.lg .brk i {
  position: absolute; display: block; background: var(--lit); opacity: .85;
  animation: brk 6.7s ease-in-out infinite;
}
.lg .brk i.h { height: 2px; width: 16px; }
.lg .brk i.v { width: 2px; height: 16px; }
.lg .brk .tl-h { top: 0; left: var(--chamfer); }
.lg .brk .tl-v { top: var(--chamfer); left: 0; }
.lg .brk .tr-h { top: 0; right: var(--chamfer); }
.lg .brk .tr-v { top: var(--chamfer); right: 0; }
.lg .brk .bl-h { bottom: 0; left: var(--chamfer); }
.lg .brk .bl-v { bottom: var(--chamfer); left: 0; }
.lg .brk .br-h { bottom: 0; right: var(--chamfer); }
.lg .brk .br-v { bottom: var(--chamfer); right: 0; }
@keyframes brk { 0%,100% { opacity: .45 } 50% { opacity: .95 } }

.lg h1 {
  font-family: var(--display);
  font-size: 26px; letter-spacing: .18em; font-weight: 700;
  color: var(--ink);
}
.lg p { color: var(--ink-dim); }
.lg .hint { color: var(--ink-faint); }"""
assert OLD_LG in t, "login .m rule not found"
t = t.replace(OLD_LG, NEW_LG, 1)

# The old h1 rule has to go, or it wins on source order.
OLD_H1 = """.lg h1 { font-size: 11.5px; letter-spacing: .34em; text-transform: uppercase; font-weight: 500; margin: 0 0 6px; }"""
NEW_H1 = """.lg h1 { text-transform: uppercase; margin: 0 0 6px; }"""
assert OLD_H1 in t
t = t.replace(OLD_H1, NEW_H1, 1)

# The input sits inside a chamfered card now, so square it and light its focus.
OLD_IN = """  background: var(--surface); border: 1px solid var(--line-hi); border-radius: var(--r);
  color: var(--ink); outline: none; transition: border-color .18s;
}
.lg input:focus { border-color: var(--ink-faint); }"""
NEW_IN = """  background: rgba(7,10,17,.55); border: 1px solid var(--line-hi); border-radius: 0;
  color: var(--ink); outline: none; transition: border-color .18s, box-shadow .18s;
}
.lg input:focus {
  border-color: var(--green);
  box-shadow: 0 0 0 1px rgba(86,210,228,.22), 0 0 22px -8px var(--green);
}"""
assert OLD_IN in t
t = t.replace(OLD_IN, NEW_IN, 1)

# The display face. Declared beside --mono; used only where Prism declaims.
OLD_FONT = '  --mono: ui-monospace, "SF Mono", "JetBrains Mono", "Cascadia Mono", Menlo, Consolas, monospace;'
NEW_FONT = OLD_FONT + """
  /* The System's face, for the few places Prism declaims. Everywhere else stays
     monospace: Prism is a console, and that is not a costume it is wearing. */
  --display: "Chakra Petch", ui-sans-serif, system-ui, sans-serif;"""
assert OLD_FONT in t
t = t.replace(OLD_FONT, NEW_FONT, 1)

# ---------------------------------------------------------------------------
# 3. Markup: wrap the card, add the canvas and the brackets
# ---------------------------------------------------------------------------
OLD_MK = """<div id="login">
  <div class="lg">
    <div class="m">"""
NEW_MK = """<div id="login">
  <div class="lg">
    <canvas class="body" id="sgbody" width="300" height="280"
            aria-label="A constellation of points, turning slowly."></canvas>
    <div class="card">
      <div class="brk" aria-hidden="true">
        <i class="h tl-h"></i><i class="v tl-v"></i><i class="h tr-h"></i><i class="v tr-v"></i>
        <i class="h bl-h"></i><i class="v bl-v"></i><i class="h br-h"></i><i class="v br-v"></i>
      </div>
    <div class="m">"""
assert OLD_MK in t
t = t.replace(OLD_MK, NEW_MK, 1)

OLD_CLOSE = """      <a id="loginSwap">use the other method</a> · no JavaScript? <b>/rescue</b>
    </div>
  </div>
</div>"""
NEW_CLOSE = """      <a id="loginSwap">use the other method</a> · no JavaScript? <b>/rescue</b>
    </div>
    </div>
  </div>
</div>"""
assert OLD_CLOSE in t
t = t.replace(OLD_CLOSE, NEW_CLOSE, 1)

# The mark is now redundant beside the body, and two marks read as clutter.
t = t.replace("""    <div class="m">
      <svg width="40" height="30" viewBox="0 0 24 18" fill="none">""",
"""    <div class="m" hidden>
      <svg width="40" height="30" viewBox="0 0 24 18" fill="none">""", 1)

# ---------------------------------------------------------------------------
# 4. The renderer
# ---------------------------------------------------------------------------
BODY_JS = r"""
<script>
/* Stargazer's body, third renderer.
 *
 * A port of body/Constellation.js — the same Fibonacci shell, the same spring
 * constants, the same emissive falloff, and the same incommensurate periods, so
 * this settles the way the other two settle. It draws idle and nothing else:
 * Prism has no quests to show and no voice to speak with, and a formation with
 * nothing behind it would be decoration.
 *
 * Deliberately self-contained and defensive. It runs on the sign-in screen,
 * which is the one screen that must render when everything else is broken — so
 * it cannot be the reason the page fails, and every entry point is guarded.
 */
(() => {
  const c = document.getElementById('sgbody');
  if (!c || !c.getContext) return;
  const ctx = c.getContext('2d');
  if (!ctx) return;
  const reduce = window.matchMedia && window.matchMedia('(prefers-reduced-motion: reduce)').matches;

  const N = 26, TAU = Math.PI * 2, GOLDEN = 2.399963229728653;
  /* Draw at device resolution and let CSS scale it down. A radial falloff at
     1x banding on a high-DPI screen is the one artefact that would make this
     look cheap next to the other two renderers. */
  const DPR = Math.min(2, window.devicePixelRatio || 1);
  const CSS_W = c.width, CSS_H = c.height;
  c.width = CSS_W * DPR; c.height = CSS_H * DPR;
  c.style.width = CSS_W + 'px'; c.style.height = CSS_H + 'px';
  ctx.scale(DPR, DPR);
  const W = CSS_W, H = CSS_H, MID_X = W / 2, MID_Y = H / 2;
  const RAD = Math.min(W, H) * 0.36, U = RAD / 94;
  const CY = [120, 224, 238], WH = [235, 252, 255];

  const rgba = (col, a) =>
    'rgba(' + (col[0] | 0) + ',' + (col[1] | 0) + ',' + (col[2] | 0) + ',' + Math.min(1, a).toFixed(3) + ')';

  const pts = [];
  for (let i = 0; i < N; i++) {
    pts.push({ i, ph: i * 1.7, sp: 0.6 + (i % 7) * 0.11,
               x: MID_X, y: MID_Y, vx: 0, vy: 0, sx: MID_X, sy: MID_Y, dep: 0.5 });
  }

  function fib(k, n) {
    const y = 1 - (k / Math.max(1, n - 1)) * 2;
    const r = Math.sqrt(Math.max(0, 1 - y * y)), th = k * GOLDEN;
    return [Math.cos(th) * r, y, Math.sin(th) * r];
  }
  function rot(x, y, z, yaw, pit) {
    const cy = Math.cos(yaw), sy = Math.sin(yaw);
    const x1 = x * cy - z * sy, z1 = x * sy + z * cy;
    const cp = Math.cos(pit), sp = Math.sin(pit);
    return [x1, y * cp - z1 * sp, y * sp + z1 * cp];
  }

  /* Fixed timestep. A variable one makes the level of detail flicker, because
     the same frame does different work depending on how long the last took. */
  const STEP = 0.016;
  let t = 0, acc = 0, last = 0;

  function step() {
    t += STEP;
    const swim = 3.2 * U;
    for (const q of pts) {
      let tx, ty;
      if (q.i === 0) { tx = MID_X; ty = MID_Y; q.dep = 0.98; }
      else {
        const v = rot(...fib(q.i - 1, N - 1), t * 0.20, 0.38);
        tx = MID_X + v[0] * RAD; ty = MID_Y + v[1] * RAD;
        q.dep = 0.5 + v[2] * 0.5;
      }
      tx += Math.sin(t * q.sp * 0.9 + q.ph) * swim;
      ty += Math.cos(t * q.sp * 0.7 + q.ph * 1.3) * swim;
      q.vx = (q.vx + (tx - q.x) * 0.062) * 0.84;
      q.vy = (q.vy + (ty - q.y) * 0.062) * 0.84;
      q.x += q.vx; q.y += q.vy;
      q.sx = q.x; q.sy = q.y;
    }
  }

  function draw() {
    ctx.clearRect(0, 0, W, H);
    ctx.globalCompositeOperation = 'lighter';

    /* Lines between genuine near neighbours, fading with distance — structure
       discovered rather than declared. The threshold is 0.62 of the radius; at
       roughly half that the shell comes apart into disconnected stubs. */
    const thr = RAD * 0.62;
    ctx.lineWidth = Math.max(1, U * 0.9);
    for (let i = 0; i < N; i++) {
      for (let j = i + 1; j < N; j++) {
        const dx = pts[i].sx - pts[j].sx, dy = pts[i].sy - pts[j].sy;
        const d = Math.sqrt(dx * dx + dy * dy);
        if (d >= thr) continue;
        ctx.strokeStyle = rgba(CY, 0.26 * (1 - d / thr));
        ctx.beginPath(); ctx.moveTo(pts[i].sx, pts[i].sy); ctx.lineTo(pts[j].sx, pts[j].sy); ctx.stroke();
      }
    }

    /* A star is EMITTED light, not a painted disc: a flat fill has a hard edge,
       and under additive blending two overlapping stars show the lens where
       they cross. Peak alpha stays low so the SUM does the brightening — past
       alpha 1 an additive source clips, and the clip boundary is itself an
       edge. */
    for (const q of pts) {
      const core = q.i === 0;
      const sz = (1.05 + q.dep * 1.05) * U * (core ? 1.20 : 1.0);
      const reach = sz * 4.4;
      const peak = (0.30 + q.dep * 0.16) * (core ? 1.30 : 1.05) * 0.88;
      const g = ctx.createRadialGradient(q.sx, q.sy, 0, q.sx, q.sy, reach);
      g.addColorStop(0.00, rgba(WH, peak));
      g.addColorStop(0.05, rgba(WH, peak * 0.66));
      g.addColorStop(0.14, rgba(CY, peak * 0.34));
      g.addColorStop(0.30, rgba(CY, peak * 0.13));
      g.addColorStop(1.00, rgba(CY, 0));
      ctx.fillStyle = g;
      ctx.beginPath(); ctx.arc(q.sx, q.sy, reach, 0, TAU); ctx.fill();
    }
    ctx.globalCompositeOperation = 'source-over';
  }

  function frame(now) {
    if (last) acc += Math.min(0.25, (now - last) / 1000);
    last = now;
    let steps = 0;
    while (acc >= STEP && steps < 4) { step(); acc -= STEP; steps++; }
    if (steps === 4) acc = 0;
    draw();
    requestAnimationFrame(frame);
  }

  /* Settle before the first paint, so the shell is a shell rather than a dot
     that explodes outward the moment anyone looks at it. */
  for (let i = 0; i < 200; i++) step();
  draw();
  if (!reduce) requestAnimationFrame(frame);
})();
</script>
"""

# Appended at the very end. The shell is one script in source order, and a block
# that runs at load time above its dependencies parses fine and throws the
# moment a browser reaches it — which is the bug scripts/check-shell-loads.py
# exists for. This has no dependencies and sits after everything regardless.
#
# There is no </body> to anchor to: the document ends at the last </script>,
# which the browser closes for it.
assert t.rstrip().endswith("</script>"), "the shell no longer ends with a script block"
t = t.rstrip() + "\n" + BODY_JS

p.write_text(t)
print(f"tier 2 applied to {p}")
