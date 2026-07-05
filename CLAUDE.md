# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

Slideflow: a macOS-first desktop app (Tauri 2 + React) that indexes every `.pptx` in chosen folders, makes individual slides full-text searchable/previewable, and composes new decks from picked slides while preserving each slide's original layout/master/theme. All PPTX handling is pure Rust — no LibreOffice, no Python, no cloud APIs.

**Note:** This app's UI copy is intentionally **English**, overriding the global "user-facing copy is German" default.

## Commands

```bash
# Engine tests — this is what CI gates on (runs on Linux/macOS/Windows):
cargo test -p slideflow-core

# Single test / single integration test file:
cargo test -p slideflow-core <test_name>
cargo test -p slideflow-core --test <file>       # files in crates/slideflow-core/tests/

# Optional large end-to-end run against a real corpus:
SLIDEFLOW_CORPUS=~/Documents/Decks cargo test -p slideflow-core --test e2e

# Desktop app (all from apps/desktop/):
pnpm install
pnpm tauri dev        # native app with hot-reloading frontend
pnpm dev              # frontend ONLY in a browser at :1420 — uses in-memory mock library
pnpm build            # tsc --noEmit && vite build (this is the CI "frontend" job)
pnpm tauri build      # production bundle → src-tauri/target/release/bundle/

# Engine example binaries (also useful as manual test harnesses):
cargo run --release -p slideflow-core --example corpus_check <folder>
cargo run --release -p slideflow-core --example render_demo deck.pptx 1 out.svg
cargo run --release -p slideflow-core --example compose_demo out.pptx deck1.pptx:3 deck2.pptx:1
cargo run --release -p slideflow-core --example seed_library library.db <folder>

# Regenerate the example corpus (deterministic; needs python-pptx ≥ 1.0):
python3 scripts/generate_examples.py
```

Clippy runs in CI but is **non-blocking**; the engine is lint-clean (as of 2026-07), and a pre-commit hook keeps it that way. `cargo fmt --check` is not gated. Releases: push a `v*` tag → `release.yml` builds unsigned bundles on all platforms and attaches them to a draft GitHub release.

**Git hooks:** versioned in `.githooks/`; enable once per clone with `git config core.hooksPath .githooks`. The pre-commit hook runs `cargo clippy -p slideflow-core --all-targets -- -D warnings` and `cargo test -p slideflow-core` whenever staged changes touch `crates/` or the workspace manifests (bypass with `git commit --no-verify`).

### Local environment gotchas

- pnpm 11 blocks postinstall builds by default; `apps/desktop/pnpm-workspace.yaml` approves esbuild via `allowBuilds` — keep it.
- If `pnpm build`/`pnpm install` fails locally on fresh transitive deps (pnpm `minimumReleaseAge` on this machine), run the tools directly: `./node_modules/.bin/tsc --noEmit && ./node_modules/.bin/vite build`.

## Architecture

Two layers, and — critically — **two separate Cargo workspaces**:

- **`crates/slideflow-core`** — the pure-Rust engine. No GUI, no GTK/WebKit dependency; builds and tests anywhere. Member of the root workspace.
- **`apps/desktop/src-tauri`** — the Tauri host. Its `Cargo.toml` declares an empty `[workspace]` table and is `exclude`d from the root workspace, so `cargo test` at the repo root (and the `core` CI job) never needs GTK/WebKit system libraries. It depends on `slideflow-core` by path. Shared dependency versions live in root `[workspace.dependencies]`.
- macOS window chrome (overlay titlebar, traffic-light inset) is configured entirely in `tauri.conf.json` — there is deliberately **no** `cfg`-gated Rust, so `cargo check` compiles cleanly on Linux.

### Engine modules (`crates/slideflow-core/src/`)

- `opc` — Open Packaging Conventions layer: zip parts, `[Content_Types].xml`, relationship (`.rels`) parsing/writing. Everything else builds on this.
- `pptx/parser` — slide order, text, notes, metadata extraction.
- `pptx/composer` — builds new decks from picked slides. The core invariant: each copied slide brings its **complete relationship closure** (layout → master → theme → media → charts) plus presentation-level parts (`defaultTextStyle`, `presProps`, `viewProps`, merged `tableStyles`, `app.xml`); identical parts across picks are deduplicated by content hash. This closure-copying is what preserves formatting — the reason the Rust engine replaced the old Swift/LibreOffice/Aspose stack.
- `render` — slide → self-contained SVG previews (theme colors, placeholder inheritance, images as data URIs).
- `index` — SQLite + FTS5 library: incremental scanning (mtime+size skip), full-text search (prefix matching, diacritic-insensitive, bm25 weighted title > deck > body > notes), filesystem watcher.
- `model` — serde domain types shared with the frontend.
- `fixtures` — programmatic minimal-but-valid PPTX builders for tests.

### Frontend ↔ backend contract

- **All IPC goes through `apps/desktop/src/lib/api.ts`** — typed wrappers that call the Tauri command or fall back to `lib/mock.ts` in browser mode (`isTauri()`). Components never call `invoke` directly.
- `lib/types.ts` mirrors `model.rs` **field-for-field in snake_case** — change them in lockstep.
- All `#[tauri::command]`s live in `apps/desktop/src-tauri/src/commands.rs`; app setup/state in `src-tauri/src/lib.rs`.
- `start_scan` runs on a background `std::thread` guarded by an `AtomicBool` and streams `ScanEvent`s to the UI over the `scan:event` channel.
- State is zustand: `useApp` (library/search/selection/layout), `useTray` (tray persisted to `localStorage`, undo/redo), `useToast`.
- `tauri.conf.json` sets `"dragDropEnabled": false` — required so HTML5 drag-and-drop works inside the webview (grid → tray uses a private MIME type, `lib/dnd.ts`). Re-enabling Tauri's native drag-drop breaks the tray.
- Decks are displayed by **file name, not docProps title** (titles are unreliable); the generated example corpus deliberately sets titles ≠ filenames to exercise this.
- Favorites are keyed by **path**, not row id, so they survive rescans.

### Test data

`examples/pptx/` is a curated corpus: generated decks (reproducible via `scripts/generate_examples.py`, fixed RNG seed) plus third-party decks in `real/`. `.gitignore` excludes all `*.pptx`/`*.pdf` **except** this corpus, so scratch outputs at the repo root never get committed.

To verify composer/export fidelity changes, pixel-diff exported decks against their sources: `soffice --convert-to pdf` → `pdftoppm` → ImageMagick `compare` per slide (LibreOffice is used only as an offline verification oracle, never at runtime).
