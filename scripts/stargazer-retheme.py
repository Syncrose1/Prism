"""Re-point Prism's shell at the Stargazer palette.

Written as a script rather than an edit so it can be applied to whichever
shell.html is being built — the working tree's, which currently carries
unrelated work in progress, or a clean checkout's. The change is the palette and
nothing else.
"""
import re, sys
from pathlib import Path

p = Path(sys.argv[1])
t = p.read_text()

OLD = '''  --void:      #050608;
  --base:      #0a0c10;
  --surface:   #0f1116;
  --raised:    #14171e;
  --line:      #1c2029;
  --line-hi:   #2c313d;

  --ink:       #e8ebf0;
  --ink-dim:   #99a1ae;
  --ink-faint: #5c6472;
  --ink-ghost: #363c47;

  --green: #4ade80;
  --amber: #fbbf24;
  --red:   #fb7185;'''

NEW = '''  /* ── The Stargazer palette ──────────────────────────────────────────────
     Generated values, from contract/identity/stargazer.tokens.json. Prism is
     one surface of one system now, and it should not have its own idea of what
     grey is.

     The tier colours are the interesting part, because Prism already had a
     better rule here than anything imposed from outside: as the machine
     degrades, decorative colour drains out of the desktop and the tier colour
     washes in, until at Black the only chroma left on screen IS the warning.
     That rule stays exactly as it is.

     What changes is which three colours it uses — and they turn out to be the
     same three the identity already names, meaning the same three things:

       green -> CYAN    the System operating. Prism's green tier means the
                        machine is fine and running normally, which is what
                        cyan means everywhere else in this system. Not a
                        substitution; the same statement in one vocabulary.
       amber -> EMBER   a warning, and the rail beside a law.
       red   -> ALERT   a breach.

     So the one-colour rule is not compromised by the retheme. It is the reason
     the retheme lands. */

  --void:      #070A11;   /* abyss  */
  --base:      #0B1018;   /* ground */
  --surface:   #111A28;   /* panel  */
  --raised:    #162233;   /* panel2 */
  --line:      #20304a;
  --line-hi:   #2c4260;

  --ink:       #DDE7F5;
  --ink-dim:   #9AACC6;
  --ink-faint: #61738f;
  --ink-ghost: #364159;

  --green: #56D2E4;   /* cyan  — the System operating */
  --amber: #E08A4B;   /* ember — a warning */
  --red:   #E2564D;   /* alert — a breach */

  /* The lit edge. Nothing in Prism used it before because Prism had no
     line-of-light to draw; it is here so that anything which grows one has the
     right colour to hand. */
  --lit:   #96ECFA;'''

assert OLD in t, "palette block not found — has the shell changed shape?"
t = t.replace(OLD, NEW)

# Glows and literal tints have to move with the colour they are a tint OF, or a
# fill and the border behind it end up different hues.
for pat, rep in [
    (r"rgba\(74,\s*222,\s*128,", "rgba(86,210,228,"),
    (r"rgba\(251,\s*113,\s*133,", "rgba(226,86,77,"),
    (r"rgba\(251,\s*191,\s*36,", "rgba(224,138,75,"),
]:
    t = re.sub(pat, rep, t)

# The mark: a beam entering a prism and leaving as three. The three it leaves as
# must be the three the tiers now use, or the logo is a legend for a dead key.
t = t.replace('stroke="#4ade80"', 'stroke="#56D2E4"')
t = t.replace('stroke="#fbbf24"', 'stroke="#E08A4B"')
t = t.replace('stroke="#fb7185"', 'stroke="#E2564D"')
t = t.replace('stroke="#e8ebf0"', 'stroke="#DDE7F5"')
t = t.replace('stroke="#5c6472"', 'stroke="#61738f"')
t = t.replace("btn.style.borderColor = '#4ade80';", "btn.style.borderColor = '#56D2E4';")

# The terminal: neutrals follow the identity, ANSI deliberately does not.
OLD_T = """    theme: {
      background: '#06070a', foreground: '#cfd6df', cursor: '#e8ebf0',
      selectionBackground: 'rgba(255,255,255,.16)',
      black: '#0a0c10', red: '#fb7185', green: '#4ade80', yellow: '#fbbf24',"""
NEW_T = """    // The terminal's own palette. The NEUTRALS follow the identity so a terminal
    // sits in the shell rather than on it; the ANSI COLOURS deliberately do not.
    // They are not decoration — they are what `ls`, `git diff` and every TUI on
    // this machine render through, and a green that is actually cyan makes a
    // diff unreadable. The one place in the product where the identity yields,
    // and it yields to something that matters more.
    theme: {
      background: '#070A11', foreground: '#DDE7F5', cursor: '#96ECFA',
      selectionBackground: 'rgba(86,210,228,.20)',
      black: '#0B1018', red: '#fb7185', green: '#4ade80', yellow: '#fbbf24',"""
assert OLD_T in t, "terminal theme block not found"
t = t.replace(OLD_T, NEW_T)
t = t.replace("brightBlack: '#5c6472',", "brightBlack: '#61738f',")
t = t.replace("brightWhite: '#e8ebf0'", "brightWhite: '#DDE7F5'")

p.write_text(t)
print(f"rethemed {p}")
