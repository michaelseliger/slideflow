# Slideflow — Design & Product Overview

> **Every slide you ever made, one keystroke away.**

**Slideflow** is a desktop app that turns the scattered pile of `.pptx` files on
your machine into a single, instantly searchable library of slides — and lets
you drag any of them into a tray to compose a brand-new deck, where every slide
keeps the *exact* look, layout, master, and theme it had in its source file.

- **Name:** Slideflow
- **Type:** Local-first desktop app for slide search & deck composition
- **Platforms:** macOS · Windows · Linux
- **License:** Open source (MIT)
- **Source:** <https://github.com/michaelseliger/slideflow>

---

## What it's about

If you present for a living, your best slides already exist — they're just
trapped inside dozens of old decks, in folders you'll never remember to open.
The one chart, the one architecture diagram, the one pricing table you nailed
six months ago is somewhere on disk, and finding it means opening file after
file.

Slideflow indexes every slide across the folders you choose and makes each
*individual slide* a first-class, searchable, previewable object. You search
the way you think ("pricing", "roadmap", "that Zürich map"), you see live
previews, you pick the ones you want, and you export a new deck — with the
original formatting fully intact.

The hard part isn't search; it's **fidelity on recombination**. Most tools that
merge slides flatten them, dropping the source layouts, masters, and themes so
your reassembled deck looks broken. Slideflow's engine copies each slide's
*complete relationship closure* — layout → master → theme → media → charts —
plus the presentation-level styling parts, deduplicating identical parts by
content hash. That's the whole point: composed decks look exactly like their
sources, every time.

---

## Features

### Search & browse
- **Full-text search across every slide** — title, deck name, body text, and
  speaker notes, powered by SQLite FTS5.
- **Smart matching** — prefix matching (`road` → `roadmap`), diacritic-
  insensitive (`zurich` finds `Zürich`), with `<mark>`-highlighted snippets.
- **Relevance ranking** — bm25, weighted title > deck > body > notes.
- **Live SVG previews** rendered straight from the slide XML — theme colors,
  placeholder inheritance, embedded images — no external renderer needed.
- **Peek modal, inspector, and grid/group-by-deck views** with adjustable
  thumbnail density.
- **Favorites**, keyed by file path so they survive re-indexing.

### Compose & export
- **Drag-to-tray composition** — pull slides from anywhere into a persistent
  tray, reorder them, and export a new `.pptx`.
- **Fidelity-preserving export** — each slide brings its full style chain;
  masters/themes/media shared across picks are deduplicated by content hash.
- **Undo/redo** on the tray; the tray is persisted and restored on relaunch.
- Moved or changed source decks get a warning badge instead of silently
  vanishing.

### Library management
- **Incremental indexing** — unchanged files (mtime + size) are skipped,
  deleted files fall out, and a filesystem watcher picks up changes live.
- **Background scanning** with a determinate progress bar and live counter.
- **Statistics view** — index runs, searches, and export activity.

### Feel
- Native window chrome, dark mode, a **command palette** (`⌘K`), and a
  keyboard-first layout: `⌘F` search · `space` peek · `return` add to tray ·
  `⌘E` export · `⌘R` re-index · `⌘Z`/`⌘⇧Z` undo/redo.

---

## Privacy

**Slideflow is local-first and offline by design. Your slides never leave your
machine.**

- **No cloud, no accounts, no telemetry.** There is no server, no sign-in, and
  nothing is uploaded. All parsing, search, preview rendering, and deck
  composition run natively on your computer.
- **No third-party runtimes handling your files.** No LibreOffice, no bundled
  Python, no paid conversion APIs — every `.pptx` is read and written by pure
  Rust code in this repository.
- **Everything stays on disk, under your control.** The search index lives in a
  local SQLite database (`~/Library/Application Support/com.slideflow.app/library.db`
  on macOS), slide previews are cached as SVGs in the app cache dir, and
  preferences live in the app's local store. Deleting the database fully resets
  the index; the thumbnail cache is safe to delete and re-renders on demand.
- **Least-privilege by default.** The desktop shell ships a tight capability set
  and a strict Content-Security-Policy (`default-src 'self'`) — the webview
  can't phone home even if it wanted to.

You point Slideflow at folders you already own, and that's the entire trust
boundary.

---

## Open source & cross-platform

Slideflow is **open source under the MIT license** — read it, fork it, build it,
ship it.

It runs on **macOS, Windows, and Linux**. The release pipeline builds installable
bundles for every platform:

- **macOS** — `.dmg` (Apple Silicon + Intel)
- **Windows** — `.msi` / NSIS `.exe`
- **Linux** — `.deb` / `.AppImage` / `.rpm`

> Repository: **<https://github.com/michaelseliger/slideflow>**

---

## How cool it is to work with

Slideflow is a genuinely nice codebase to hack on — deliberately structured so
the engine and the app never get in each other's way:

- **A pure-Rust engine you can test anywhere.** `crates/slideflow-core` — the
  PPTX parser, the FTS5 search index, the SVG renderer, and the style-preserving
  composer — has **no GUI and no GTK/WebKit dependency**. `cargo test -p
  slideflow-core` runs on plain Linux/macOS/Windows, so CI is fast and the
  interesting logic is unit-testable without a desktop.
- **A thin, honest app layer.** `apps/desktop` is a Tauri 2 + React 18 +
  TypeScript + Vite + Tailwind shell whose Rust host is just a typed IPC bridge
  to the engine. Every call goes through one wrapper module — no scattered
  `invoke`s to chase.
- **Instant browser dev mode.** Run `pnpm dev` and the whole UI — search, peek,
  tray, export, dark mode — is clickable in a plain browser against an in-memory
  mock library of ~40 slides. No native build required to iterate on the front
  end.
- **A reproducible test corpus.** `examples/pptx/` holds a curated set of decks
  (regenerable from a fixed-seed script) so parsing, search, and export fidelity
  are verified against real, deterministic input — and export correctness can be
  pixel-diffed against sources.
- **CI across all three OSes** on every push, plus a one-tag release pipeline.

Clean separation, fast tests, a mockable UI, and a single source of truth for
the frontend↔backend contract — it's the kind of project where a change is easy
to reason about and easy to verify.

---

*Slideflow — search your slides, compose new decks with original fidelity.*
