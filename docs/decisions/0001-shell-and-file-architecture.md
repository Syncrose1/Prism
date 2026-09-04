# ADR 0001: Build the shell, borrow the media pipeline

**Date:** 2026-09-05
**Status:** Accepted
**Context:** before implementing Files (#6), the OS shell (#7) and Media (#10).

---

## Question

Prism needs a windowed cloud-OS shell, a file manager with previews, and media
streaming. All three have mature open-source prior art. Should we adopt, fork, or
build — and what makes the result fast?

## Prior art surveyed

| Project | Stack | Shape | Verdict |
|---|---|---|---|
| [Puter](https://openapps.pro/apps/puter) | Node.js | Full browser desktop, windows, app store, cloud FS | Closest to the vision; wrong runtime |
| [CasaOS](https://dev.co/devops/open-source/casaos) | Go + Vue | Home-server app dashboard | Not a desktop; different product |
| [FileBrowser](https://www.makeuseof.com/file-browser-lightweight-self-hosted-google-drive-alternative/) | Go, single binary | File manager, previews for image/video/text/PDF | Right shape — but **archived / maintenance-only** |
| [Filestash](https://www.xda-developers.com/filestash-self-hosted-replacement-of-file-browser/) | Go | Preview-first, mobile-friendly, multi-backend | Its live successor; strong pattern reference |

## Decision

**Build the shell and file layer inside `prismd`. Do not rebuild the media
pipeline — shell out to what is already installed.**

### Why not adopt a web OS

The decisive argument is a requirement already recorded in `architecture.md` §5a:

> the shell must stay usable at Red tier, when the host is already struggling;
> it is a rescue interface before it is a desktop

Red tier means under ~1.5 GiB of honest headroom. That is the exact moment the
operator needs the interface, and the worst possible moment to be starting a
Node.js runtime. Adopting Puter would make the rescue interface the second thing
to die in the emergency it exists to manage.

Two supporting reasons, neither sufficient alone:

- A second runtime is a second thing that can fail, on a host whose whole premise
  is that it must not become unreachable.
- Prism's actual differentiator — PSI-driven governance, cgroup-bounded facets,
  storm detection — does not exist in any of these projects. The desktop is the
  commodity half; adopting it would mean carrying a large dependency to obtain
  the part that is easy.

### Why no frontend build step

No Vite, no npm, no `node_modules`. The UI is hand-written and embedded via
`rust-embed`, served from the same binary in one request. Reasons:

- a build toolchain in a Rust project is a second toolchain to keep working, for
  a UI whose entire budget is "small enough to embed"
- `/home` on this host is **95% full** — adding a node_modules tree is actively
  unhelpful
- first paint must not depend on a CDN; the CSP forbids it and the tailnet may be
  the only route available

### What we borrow rather than build

Verified present on `c2` on 2026-09-05:

```
ffmpeg   cuda, vaapi, vulkan, qsv hwaccel + 3 nvenc encoders
vips     ~10x ImageMagick on thumbnails, far lower peak memory
pdftoppm PDF page rasterisation
magick, exiftool
```

Files and Media are therefore **orchestration**, not imaging code:

| Need | Tool | Note |
|---|---|---|
| Image thumbnails | `vips thumbnail` | Chosen over ImageMagick specifically for peak memory — this runs on a memory-constrained host |
| Video thumbnails | `ffmpeg -ss` single frame | Seek before decode, never decode the file |
| Video transcode to phone | `ffmpeg` + `h264_nvenc` | Hardware path on the RTX 3060; near-free CPU |
| PDF preview | `pdftoppm` | First page only |
| Metadata | `exiftool` | On demand, never during listing |

Interaction patterns are borrowed from Filestash's preview-first, mobile-first
design, which matches the operator's stated phone use case.

## Performance architecture

What actually determines perceived speed, in order:

**1. Directory listing must not be N+1.** A single `getdents` pass, `statx` only
for fields being displayed, and no per-entry metadata, MIME sniffing, or
thumbnail generation during listing. Results paginated server-side; the client
virtualises the list. Directories with 100k entries are normal in a models
folder.

**2. Thumbnails are generated on demand and cached on disk**, keyed by
`(path, mtime, size, target_dimension)`, served with a strong `ETag` so repeat
views are a 304. Generation is bounded by a small worker pool — an unbounded
thumbnail queue on a directory of 4k images is itself a memory incident.

**3. The cache is budgeted and evicted, LRU, default 2 GiB.** `/home` has 44 GiB
free. An unbounded cache on a 95%-full disk is a self-inflicted outage.

**4. Media streams via HTTP range requests**, never buffered server-side.
Transcoding only when the client cannot play the source, and only through NVENC.

**5. Telemetry over WebSocket, not polling.** One connection at 1 Hz rather than
every open app issuing its own interval fetch.

**6. The shell degrades by tier.** At Red, Prism suppresses thumbnail generation
and transcoding, and serves a minimal listing. The interface stops being pretty
before it stops being available. This is the inverse of the usual priority and is
the whole point.

## Consequence: Prism needs a disk sensor

Surfacing this because the survey exposed it. `/home` is at **95% (44 GiB free)**
while `§4.1` senses only memory, PSI and GPU.

The machine is considerably closer to exhausting disk than RAM, and a full
`/home` wedges a session as effectively as a thrash spiral — with ComfyUI writing
outputs and llama.cpp models measured in gigabytes, it is the more likely of the
two. The governor's tiers should incorporate free-space and inode floors, and
Files/Media must account for their own cache against that budget.

Tracked separately; blocks nothing here, but it is a gap in the resilience story
rather than a feature request.

## Alternatives rejected

- **Fork FileBrowser** — Go, so a second toolchain and no reuse of `prism-core`;
  and it is archived, so we would inherit maintenance immediately.
- **Embed Filestash behind a proxy** — solves Files, but leaves the shell
  unbuilt, adds a runtime, and splits auth across two systems.
- **Vite + Svelte, embedded at build time** — genuinely tempting for the window
  manager. Rejected on the runtime-and-disk grounds above; revisit only if the
  hand-written shell becomes unmaintainable.
