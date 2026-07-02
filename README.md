# Slideflow

**Every slide you ever made, one keystroke away.**

Slideflow is a macOS desktop app that indexes every PowerPoint file in the
folders you choose, makes each individual slide full-text searchable and
previewable, and lets you drag slides from anywhere into a tray to compose a
new presentation — where every slide keeps the exact look, layout, master and
theme it had in its source deck.

Built as a Tauri 2 app with a pure-Rust engine. No LibreOffice, no bundled
Python, no cloud APIs: parsing, search, preview rendering and deck composition
are all native.

```
┌────────────────────────────────────────────────────────────┐
│  apps/desktop        Tauri 2 shell + React/TS frontend     │
│    src/              search UI, grid, tray, command palette│
│    src-tauri/        thin IPC layer over slideflow-core    │
├────────────────────────────────────────────────────────────┤
│  crates/slideflow-core                                     │
│    opc        zip parts, content types, relationships      │
│    pptx       parser (text/notes/metadata) + composer      │
│    render     slide → SVG previews (theme-aware)           │
│    index      SQLite FTS5 search, scanning, file watching  │
└────────────────────────────────────────────────────────────┘
```

## Why the rewrite

The previous Swift prototype depended on LibreOffice for thumbnails, a
bundled PyInstaller Python for merging, and the paid Aspose Cloud API for
export — and its merger dropped source layouts/masters/themes, so merged
slides lost their formatting. The Rust engine replaces all three legs and
copies each slide's complete relationship closure (layout → master → theme →
media → charts) with content-hash deduplication, which is what actually
preserves formatting.

## Development

### Two layers

Slideflow is two pieces (see the diagram above):

- **`crates/slideflow-core`** — the pure-Rust engine: PPTX parsing, the
  SQLite/FTS5 search index + file watcher, the slide→SVG renderer, and the
  style-preserving deck composer. No GUI, no GTK/WebKit, no OS dependency — it
  builds and tests anywhere.
- **`apps/desktop`** — the Tauri 2 shell (React 18 + TypeScript + Vite +
  Tailwind frontend, thin Rust IPC host) that drives the engine.

The Tauri host at `apps/desktop/src-tauri` is a **separate Cargo workspace**
(its `Cargo.toml` has an empty `[workspace]` table) and is intentionally
`exclude`d from the root workspace. That is what lets `cargo test` at the repo
root — and the `core` CI job — run on plain Linux/macOS/Windows without any
GTK/WebKit system libraries.

### Prerequisites

- **Rust** stable, via [rustup](https://rustup.rs).
- **Node 22** and **pnpm 11** (`npm i -g pnpm@11`, or Corepack: `corepack enable`).
- **macOS:** Xcode Command Line Tools (`xcode-select --install`). Tauri uses the
  system WebKit (`WKWebView`) — no bundled Chromium.
- **Linux (to build/run the desktop app):** the Tauri 2 system libraries —
  `libwebkit2gtk-4.1-dev`, `libappindicator3-dev` (or
  `libayatana-appindicator3-dev`), `librsvg2-dev`, `patchelf`, plus the usual
  `build-essential`, `libxdo-dev`, `libssl-dev`. (Not needed just to test the
  engine crate.)

### First-time setup

```bash
git clone <repo> && cd slideflow
cargo test -p slideflow-core        # verify the engine builds + passes
cd apps/desktop && pnpm install     # install frontend deps
```

### Dev loop

```bash
cd apps/desktop

# Full native app with hot-reloading frontend (recommended):
pnpm tauri dev

# Frontend only, in a plain browser — no native shell. Falls back to an
# in-memory mock library (~40 slides), so the whole UI (search, peek, tray,
# export, dark mode) is clickable at http://localhost:1420:
pnpm dev
```

### Tests

```bash
# Engine unit + integration tests (this is what CI gates on):
cargo test -p slideflow-core

# Optional: run the large-file end-to-end test against a real corpus:
SLIDEFLOW_CORPUS=~/Documents/Decks cargo test -p slideflow-core --test e2e
```

### Engine examples

Runnable tools under `crates/slideflow-core/examples/`:

```bash
# Parse every .pptx in a folder and print per-file stats + timings:
cargo run --release -p slideflow-core --example corpus_check ~/Documents/Decks

# Render one slide (1-based index) to a standalone SVG:
cargo run --release -p slideflow-core --example render_demo deck.pptx 1 slide.svg

# Compose a new deck from picked slides (deck.pptx:INDEX, 1-based):
cargo run --release -p slideflow-core --example compose_demo out.pptx deck1.pptx:3 deck2.pptx:1

# Create a library DB from a root folder and scan it:
cargo run --release -p slideflow-core --example seed_library library.db ~/Documents/Decks
```

### Production build

```bash
cd apps/desktop
pnpm tauri build     # native bundle for the current OS
```

Artifacts land in `apps/desktop/src-tauri/target/release/bundle/` (e.g.
`macos/Slideflow.app`, `dmg/Slideflow_<version>_aarch64.dmg`).

See `apps/desktop/README.md` for app-specific details (data locations,
keyboard map, frontend-only browser mode, packaging).

### CI & releases

Two GitHub Actions workflows live in `.github/workflows/`:

- **`ci.yml`** (every push / PR): builds + tests `slideflow-core` on Linux,
  macOS and Windows (`core`), type-checks and builds the frontend
  (`frontend`), and runs `clippy`. The clippy job is currently
  **non-blocking** (the engine still trips a few lints); `cargo fmt --check`
  is not gated for the same reason.
- **`release.yml`** (push a `v*` tag, or run manually via *workflow_dispatch*):
  builds installable bundles on every platform and, for a tag push, attaches
  them to a **draft** GitHub release — macOS `.dmg` (Apple Silicon + Intel),
  Linux `.deb` / `.AppImage` / `.rpm`, and Windows `.msi` / `.exe` (NSIS).
  Artifacts are **unsigned** (no code signing / notarization configured yet).
  A manual run builds the bundles without creating a release.

To cut a release: `git tag v0.2.0 && git push origin v0.2.0`, then publish the
drafted release once the bundles are attached.

## Engine guarantees

- **Indexing** is incremental: unchanged files (mtime+size) are skipped;
  deleted files fall out of the index; a filesystem watcher picks up changes
  while the app runs.
- **Search** is SQLite FTS5 with prefix matching, diacritic-insensitive
  tokenization (`zurich` finds `Zürich`), bm25 ranking weighted
  title > deck > body > notes, and `<mark>`-highlighted snippets.
- **Previews** are self-contained SVGs rendered straight from the slide XML
  (theme colors, placeholder inheritance, images as data URIs) — no external
  renderer, safe to embed, cached on disk.
- **Composition** produces a valid PPTX where every copied slide brings its
  full style chain along; identical masters/themes/media across picks are
  deduplicated by content hash.
