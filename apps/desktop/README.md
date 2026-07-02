# Slideflow — Desktop App

Search every slide across your presentation folders, then compose brand-new
decks that **keep each slide's original theme, master, and formatting**.

Built with **Tauri 2 + React 18 + TypeScript + Vite + Tailwind**, on top of the
native `slideflow-core` Rust engine (SQLite/FTS5 index, PPTX parser, SVG slide
renderer, and style-preserving composer). macOS-first, no LibreOffice, no
PowerPoint, no cloud.

---

## Prerequisites

- **Rust** — install via [rustup](https://rustup.rs). Stable toolchain.
- **pnpm** — `npm i -g pnpm` (or Corepack: `corepack enable`).
- **Xcode Command Line Tools** (macOS) — `xcode-select --install`.
  Tauri uses the system WebKit (`WKWebView`), so there's no bundled Chromium.
- On Linux (for `cargo check` / dev only): the usual WebKitGTK / GTK dev
  packages (`libwebkit2gtk-4.1-dev`, `libgtk-3-dev`, `libayatana-appindicator3-dev`,
  `librsvg2-dev`, `patchelf`).

## Install

```bash
cd apps/desktop
pnpm install
```

## Develop

Run the app in a native window with hot-reloading frontend:

```bash
pnpm tauri dev
```

You can also run **just the web frontend** in a browser — `pnpm dev` — and it
falls back to an in-memory mock library of ~40 slides across a handful of decks,
so the whole UI (search, peek, tray, export flow, dark mode) is clickable
without the native shell.

```bash
pnpm dev          # http://localhost:1420 — browser mock mode
```

## Build

Type-check + production frontend bundle:

```bash
pnpm build        # tsc --noEmit && vite build
```

Native app bundle (`.app` and `.dmg` on macOS):

```bash
pnpm tauri build
```

The artifacts land in `src-tauri/target/release/bundle/`
(`macos/Slideflow.app`, `dmg/Slideflow_<version>_aarch64.dmg`).

---

## Where your data lives (macOS)

| What | Path |
| --- | --- |
| Library database (SQLite/FTS5) | `~/Library/Application Support/com.slideflow.app/library.db` |
| Rendered slide thumbnails (SVG cache) | `~/Library/Caches/com.slideflow.app/thumbs/<slide_id>.svg` |
| Tray + preferences (theme, grid density) | Browser `localStorage` inside the app's WebKit data store |

Removing a folder or deleting `library.db` fully resets the index; the thumb
cache is safe to delete at any time (it re-renders on demand).

---

## Architecture

```
apps/desktop/
├── src/                     React frontend
│   ├── lib/
│   │   ├── api.ts           Typed invoke() wrappers + isTauri() mock fallback
│   │   ├── types.ts         EXACT mirror of crates/slideflow-core/src/model.rs
│   │   ├── mock.ts          ~40-slide fake library for browser dev
│   │   ├── dnd.ts           Drag-and-drop payloads + stacked drag ghost
│   │   ├── useSlideSvg.ts   Lazy, cached SVG thumbnail loader
│   │   └── utils.ts
│   ├── stores/              zustand: useApp (library/search/selection/layout),
│   │                        useTray (persistent tray + undo/redo), useToast
│   ├── components/          Header, Sidebar, SlideGrid, SlideCard, Inspector,
│   │                        Tray, PeekModal, CommandPalette, ExportSheet, …
│   └── App.tsx              Layout + global keyboard map + scan event wiring
└── src-tauri/               Rust / Tauri host
    ├── src/lib.rs           App setup: opens Library, manages state, plugins
    ├── src/commands.rs      All #[tauri::command]s (list_roots, add_root,
    │                        start_scan, search, get_slide_svg, compose_deck, …)
    ├── tauri.conf.json      Overlay titlebar, hidden title, traffic-light inset
    └── capabilities/        Least-privilege permission set for the main window
```

### How the pieces talk

- The frontend never calls `invoke` directly — everything goes through
  `lib/api.ts`, which either hits the Tauri command or the browser mock.
- `start_scan` runs on a background `std::thread`, guarded by an `AtomicBool`,
  and streams `ScanEvent`s to the UI over the `scan:event` channel. The sidebar
  shows a live, determinate progress bar with a slides-indexed counter.
- `get_slide_svg` renders a slide via `slideflow-core::render`, caches the SVG
  under the app cache dir, and persists the path with `set_thumb_path`, so
  repeat views are a plain file read.
- The composition tray is persisted continuously to `localStorage` and restored
  on relaunch; if a source deck moves/changes after a re-index, affected tray
  items get a subtle warning badge instead of being silently dropped.

### Keyboard

`⌘F` focus search · `esc` clear/close · arrows move selection · `space` peek ·
`return` add to tray · `⌘A` select all · `⌘T` tray · `⌘I` inspector ·
`⌘⌃S` sidebar · `⌘+/⌘-` thumbnail size · `⌘Z`/`⌘⇧Z` tray undo/redo ·
`⌘E` export · `⌘R` re-index · `⌘K` command palette · `1`/`2` flat / group-by-deck.

---

## Notes for integrators

- `src-tauri/Cargo.toml` declares an **empty `[workspace]`** so the desktop crate
  stays independent of the root Cargo workspace (root CI/Linux `cargo test`
  never needs GTK/WebKit). It depends on `slideflow-core` by path.
- macOS-only window chrome (Overlay titlebar, hidden title, traffic-light inset)
  lives entirely in `tauri.conf.json` — there is **no** `cfg`-gated Rust, so
  `cargo check` compiles cleanly on Linux.
- TypeScript model types in `lib/types.ts` mirror `model.rs` field-for-field in
  **snake_case** — keep them in lockstep if the Rust types change.
