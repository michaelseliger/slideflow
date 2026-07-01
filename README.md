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

Prerequisites: Rust (rustup), Node 20+ with pnpm, and on macOS the Xcode
command-line tools.

```bash
# Engine: build + test (pure Rust, runs anywhere)
cargo test -p slideflow-core

# Optional: validate the parser against a folder of real decks
cargo run --release -p slideflow-core --example corpus_check ~/Documents/Decks

# Optional: run the large-file end-to-end test against a corpus
SLIDEFLOW_CORPUS=~/Documents/Decks cargo test -p slideflow-core --test e2e

# Desktop app (macOS)
cd apps/desktop
pnpm install
pnpm tauri dev       # run the app
pnpm tauri build     # produce Slideflow.app / .dmg
```

See `apps/desktop/README.md` for app-specific details (data locations,
frontend-only browser mode, packaging).

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
