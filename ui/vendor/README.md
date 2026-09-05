# Vendored assets

Committed rather than fetched at build time: the shell must render with no
network access beyond the tailnet, and Prism has no npm toolchain.

| File | Source | Licence |
|---|---|---|
| `xterm.js`, `xterm.css` | [@xterm/xterm](https://github.com/xtermjs/xterm.js) 5.5.0 | MIT |
| `xterm-addon-fit.js` | [@xterm/addon-fit](https://github.com/xtermjs/xterm.js) 0.10.0 | MIT |
| `term-nerd.woff2` | [CaskaydiaCove Nerd Font Mono](https://github.com/ryanoasis/nerd-fonts), subset | SIL OFL 1.1 |

The font is subset to Latin, box drawing, Powerline and the Nerd Font
private-use blocks — 2.7 MB down to 789 KB — so terminal glyphs render on a
device that has no such font installed.
