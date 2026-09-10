# Prism under the Stargazer identity

Prism is one surface of one system now. It should not have its own idea of what
grey is, so `ui/shell.html` draws from the shared palette in
`Contract/contract/identity/stargazer.tokens.json`.

## What changed, and what did not

**The neutral ramp** — void, base, surface, raised, line, ink — is the identity's,
value for value.

**The tier colours changed hue and kept their meaning**, which is the part worth
understanding. Prism already had the better rule here:

> As the machine degrades, decorative colour drains out of the desktop and the
> tier colour washes in. By Black the wallpaper is fully greyscale and the only
> chroma left on screen is the warning itself.

That rule is untouched. What changed is which three colours it uses, and they
turn out to be three the identity already names, meaning the same three things:

| was | is | because |
|---|---|---|
| green `#4ade80` | **cyan** `#56D2E4` | Prism's green tier means the machine is fine and running normally. Cyan means *the System operating*. Same statement, one vocabulary. |
| amber `#fbbf24` | **ember** `#E08A4B` | a warning, and the rail beside a law |
| red `#fb7185` | **alert** `#E2564D` | a breach |

So the one-colour rule is not compromised by the retheme — it is why the retheme
lands rather than being applied over the top of something.

**The mark** disperses into those three, because a beam entering a prism and
leaving as three colours is a legend for the tier key. Leaving it green/amber/red
would have made it a legend for a key that no longer exists.

## The one place the identity yields

**The terminal's ANSI palette is deliberately unchanged.** Its neutrals follow
the identity — background, foreground, cursor, selection — so a terminal sits
*in* the shell rather than on it. Its sixteen ANSI colours do not, because they
are not decoration: they are what `ls`, `git diff` and every TUI on this machine
render through, and a green that is actually cyan makes a diff unreadable.

## Applying it

    python3 scripts/stargazer-retheme.py ui/shell.html

Written as a script rather than committed as an edit because the working tree
often carries unrelated work in progress, and this needs to be applicable to a
clean checkout as easily as to a dirty one. It is idempotent in the sense that
it asserts on the shape it expects and fails loudly if the shell has moved.

## Rebuilding

The UI is embedded in the binary by `RustEmbed`, so a shell change needs a full
rebuild — this is not a static file you can edit in place.

    cargo build --release --bin prismd
    systemctl --user stop prismd      # the binary is busy while it runs
    cp target/release/prismd ~/.local/bin/prismd
    systemctl --user start prismd
    ss -ltnp | grep prismd            # CHECK THE ADDRESS, not just that it started

Keep a copy of the working binary first. Prism is the lifeline; a bad build is
the one thing that cannot be fixed remotely.


## Tier 2 — the chrome, and the body

A palette makes a thing *tinted*. Geometry and motion make it *recognisable*, so
this pass adds what actually reads as the System:

- **The chamfer.** `--r` is `0`, and surfaces that matter are cut by `clip-path`
  at 12px. A 4px radius is a perfectly good radius belonging to a different
  product.
- **Corner brackets** on the sign-in card, breathing on a 6.7s period — which
  divides into none of the others, so the composite never returns to where it
  started and nothing here is visibly a loop.
- **A lit top edge**, because the top of a System surface catches light.
- **The constellation**, as Prism's third renderer. QML draws it on the desktop,
  Compose on the phone, and now canvas here — no shared code between the three
  and every value the same, which is the whole bargain of a token source. It
  sits on the sign-in screen because that is the first thing seen every time,
  and a System that is waiting for you should look like it is waiting rather
  than like a form.
- **The display face**, for the few places Prism declaims. Everywhere else stays
  monospace: Prism is a console, and that is not a costume.

The body draws `idle` and nothing else. Prism has no quests to show and no voice
to speak with, and a formation with nothing behind it is decoration.

### A measurement trap, recorded because it cost three rounds

The card fades in over 0.5s and the body *turns*. A headless screenshot samples
one instant of both, so "brightest pixel" varies enormously between captures —
it depends on where the card is in its fade and which star happens to be nearest
the viewer. I read that as a renderer that was too dim and compensated for it
three times before checking.

The renderer measured in isolation, with a known white swatch as a probe, is
correct: white reads as white, and the body peaks in the same range as the QML
original and the Compose port. All three compensations were reverted.

If you need to measure this again, neutralise `@keyframes rise`, drop a
`ctx.fillRect` of pure white on the canvas, and confirm it reads 255 before
trusting anything else on the frame.
