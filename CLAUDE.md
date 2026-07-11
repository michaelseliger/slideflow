# CLAUDE.md

## What this is

Slideflow: macOS-first desktop app (Tauri 2 + React) that indexes every `.pptx` in chosen folders, makes individual slides full-text searchable/previewable, and composes new decks from picked slides while preserving each slide's original layout/master/theme. All PPTX handling is pure Rust — no LibreOffice, Python, or cloud.

**UI copy is English** — this overrides the global "user-facing copy is German" default.

## Commands

CI's `core` job (Linux/macOS/Windows) runs `cargo test -p slideflow-core -p slideflow-cli --all-targets` and release-builds the CLI. Day-to-day:

```bash
cargo test -p slideflow-core                     # engine suite — the primary gate
cargo test -p slideflow-core <name>              # single test
cargo test -p slideflow-core --test <file>       # one integration file (tests/)
SLIDEFLOW_CORPUS=~/Documents/Decks cargo test -p slideflow-core --test e2e   # optional real-corpus run
```

Desktop app (from `apps/desktop/`):

```bash
pnpm tauri dev     # native app, hot-reload frontend
pnpm dev           # frontend only at :1420 (in-memory mock library)
pnpm build         # tsc --noEmit && vite build  — CI "frontend" job
pnpm test:tray     # pure-logic tray-model tests (Node type-strip, needs ≥22.6); CI runs these too
pnpm tauri build   # production bundle → src-tauri/target/release/bundle/
```

Engine examples double as manual harnesses: `cargo run --release -p slideflow-core --example <corpus_check|render_demo|compose_demo|seed_library> …`. Regenerate the corpus: `python3 scripts/generate_examples.py` (deterministic, needs python-pptx ≥ 1.0).

**CI/release:** clippy runs but is non-blocking; `cargo fmt --check` isn't gated. The optional `embeddings` feature (candle-backed E5) is a non-blocking `cargo check` only. Push a `v*` tag → `release.yml` builds unsigned bundles on all platforms into a draft release.

**Git hooks** (`.githooks/`, enable per clone: `git config core.hooksPath .githooks`; bypass: `--no-verify`). Pre-commit gates by what's staged: clippy `-D warnings` + `core`/`cli` tests for `crates/`/manifests; `tsc --noEmit` + `pnpm test:tray` for `apps/desktop/src/` (skips the slow vite build).

**pnpm gotchas:** pnpm 11 blocks postinstall builds — `apps/desktop/pnpm-workspace.yaml` approves esbuild via `allowBuilds` (keep it). If `pnpm install`/`build` fails on fresh transitive deps (`minimumReleaseAge` cooldown), run tools directly: `./node_modules/.bin/tsc --noEmit && ./node_modules/.bin/vite build`.

## Architecture

**Two separate Cargo workspaces:**

- `crates/slideflow-core` — pure-Rust engine, no GUI/GTK/WebKit; builds and tests anywhere. Root-workspace member.
- `crates/slideflow-cli` — thin `slideflow` CLI over the engine; CI release-builds it on all three OSes. Also a root member.
- `apps/desktop/src-tauri` — the Tauri host. Empty `[workspace]` table + `exclude`d from root, so root `cargo test` (the `core` CI job) needs no GTK/WebKit. Path-depends on `slideflow-core`; shared versions in root `[workspace.dependencies]`.
- macOS window chrome lives entirely in `tauri.conf.json` — no `cfg`-gated Rust, so `cargo check` stays clean on Linux.

### Engine modules (`crates/slideflow-core/src/`)

- `opc` — OPC layer: zip parts, `[Content_Types].xml`, `.rels` parse/write. Everything builds on this.
- `pptx/parser` — slide order, text, notes, metadata.
- `pptx/composer` — builds decks from picked slides. **Core invariant:** each slide brings its full relationship closure (layout→master→theme→media→charts) plus presentation-level parts (`defaultTextStyle`, `presProps`, `viewProps`, merged `tableStyles`, `app.xml`); identical parts deduped by content hash. This closure-copying is what preserves formatting — the reason Rust replaced the old Swift/LibreOffice/Aspose stack.
- `render` — slide → self-contained SVG (theme colors, placeholder inheritance, images as data URIs).
- `index` — SQLite + FTS5: incremental scan (mtime+size skip), full-text search (prefix, diacritic-insensitive, bm25 weighted title>deck>body>notes), fs watcher.
- `model` — serde domain types shared with the frontend.
- `fixtures` — minimal-but-valid PPTX builders for tests.

### Frontend ↔ backend contract

- **All IPC goes through `apps/desktop/src/lib/api.ts`** — typed wrappers that call the Tauri command or fall back to `lib/mock.ts` in browser mode (`isTauri()`). Components never call `invoke` directly.
- `lib/types.ts` mirrors `model.rs` **field-for-field in snake_case** — change them in lockstep.
- `#[tauri::command]`s live in `src-tauri/src/commands.rs`; setup/state in `src-tauri/src/lib.rs`.
- `start_scan` runs on a background `std::thread` guarded by an `AtomicBool`, streaming `ScanEvent`s over the `scan:event` channel.
- zustand stores: `useApp` (library/search/selection/layout), `useTray` (localStorage-persisted, undo/redo), `useToast`.
- `tauri.conf.json` sets `"dragDropEnabled": false` so HTML5 drag-and-drop works in the webview (grid→tray uses a private MIME type, `lib/dnd.ts`). Re-enabling native drag-drop breaks the tray.
- Decks shown by **file name, not docProps title** (titles unreliable; the corpus sets titles ≠ filenames to exercise this).
- Favorites keyed by **path**, not row id — so they survive rescans.

### Test data

`examples/pptx/` — curated corpus: generated decks (reproducible via `scripts/generate_examples.py`, fixed RNG seed) + third-party decks in `real/`. `.gitignore` excludes all `*.pptx`/`*.pdf` **except** this corpus, so scratch outputs never get committed.

Verify composer/export fidelity by pixel-diffing exports vs sources: `soffice --convert-to pdf` → `pdftoppm` → ImageMagick `compare` per slide (LibreOffice is an offline oracle only, never at runtime).
