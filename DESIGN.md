# Slideflow — Design Handover

> **Every slide you ever made, one keystroke away.**

Slideflow is a local-first desktop app (Tauri 2 + React, macOS-first) that indexes
every `.pptx` in folders you choose, makes each individual slide searchable and
previewable, and composes new decks from picked slides — preserving each slide's
original layout, master, and theme. This document is the design source of truth:
every screen, overlay, state, and token, with screenshots.

**Working on the design:** run `pnpm dev` in `apps/desktop/` and open
`http://localhost:1420` — the full UI runs in a plain browser against a mock
library (~35 slides, 6 decks) with working search, tray, export simulation,
fonts panel, and the complete AI-consent/download flow. No native build needed.
Screenshots below were captured in exactly this mode. See
[Mock-mode limits](#mock-mode-limits) for the few things that behave differently.

- **App icon:** three stacked slides (beige `#C9C7BD` → gray `#8B897E` → blue
  `#3056D6`) on a warm off-white squircle — shared with the website's favicon.
  Source of truth: `apps/desktop/src-tauri/icons/icon.png`.

---

## Design language

Tokens live in `apps/desktop/tailwind.config.js` + CSS variables in
`src/index.css`. Dark mode is a `.dark` class on `<html>` (System/Light/Dark in
Settings → Appearance; System tracks `prefers-color-scheme` live).

| Token | Light | Dark |
|---|---|---|
| canvas | `#F6F6F7` | `#141414` |
| surface | `#FFFFFF` | `#1E1E1E` |
| elevated | `#FFFFFF` | `#262628` |
| ink | `#1C1C1E` | `#ECECEE` |
| subtle | `#6E6E73` | `#98989E` |
| accent | `#0A84FF` | `#0A84FF` (`accent.soft` = 10 %) |

- Semantic colors: amber = warnings (moved/off-size), green = success, red =
  danger, purple = near-duplicate.
- **Vibrancy** (`material`): translucent tint + `backdrop-filter: saturate(180%)
  blur(20px)` — sidebar, tray, header. In the browser this is a CSS
  approximation; the native app gets real vibrancy.
- **Type:** system font stack. Sizes: caption 11/14 · body 13/18 · title 15/20 ·
  heading 17/22. `.tabnum` for tabular numerals in counts.
- **Radii:** controls 6 · cards 8 · sheets/modals 12 · pills full.
- **Selection is always an accent ring**, never a color wash. Hairlines are
  0.5 px inset box-shadows.
- Motion: framer-motion springs, all `prefers-reduced-motion`-aware.

---

## Main window anatomy

```
┌──────────┬────────────────────────────────────┬───────────┐
│          │  Header: toolbar (52px) + strip    │           │
│ Sidebar  ├────────────────────────────────────┤ Inspector │
│ 224px    │  Main content                      │ 300px     │
│ (60 col- │  (grid / stats / duplicates /      │ (toggle)  │
│ lapsed)  │   empty / zero-results)            │           │
├──────────┴────────────────────────────────────┴───────────┤
│  Tray — full-width filmstrip, 132px (34px collapsed)      │
└───────────────────────────────────────────────────────────┘
```

![Main grid, light](docs/screenshots/01-grid-light.png)
![Main grid, dark](docs/screenshots/02-grid-dark.png)

- **Sidebar** (`Sidebar.tsx`, vibrancy): 52 px drag region for traffic lights;
  Library rows (All Slides / Favorites / Duplicates / Statistics); folder roots
  with live counts; Saved Searches; Decks (favorited decks get a filled amber
  star); Tags with counts. Bottom: scan progress + AI-indexing bars, pinned
  "Add folder…" and "About". Right-click menus on roots/decks/saved/tags.
- **Header** (`Header.tsx`, draggable titlebar): search field with clear + `?`
  syntax help; bookmark (save search — only when a query is present);
  retrieval-mode segmented control (only when the AI model is ready);
  palette/sidebar/inspector toggles. Below, a 36 px strip: result count, filter
  chips (query/deck/folder/date — each removable), density − / +, Sort menu,
  flat vs. group-by-deck toggle.
- **Inspector** (`Inspector.tsx`, opaque surface): preview, title, match
  snippet, metadata list, tags editor (chips, autocomplete, Enter to create),
  speaker notes, AI "Similar slides" (model-gated), actions (Add to Tray /
  favorite / Reveal in Finder).
- **Tray** (`Tray.tsx`, vibrancy): tray switcher dropdown, collapse chevron,
  warning pills (**moved / off-size / duplicates**), Clear, **Export…**;
  horizontal drag-to-reorder filmstrip with index badges and hover-remove.

![Sidebar during a scan](docs/screenshots/15-sidebar-scan.png)

---

## Screens

### Slide grid (default)
Virtualized grid, 3–10 columns (density stepper or ⌘+/⌘−). Cards show the SVG
preview (rendered from the real slide XML), title, deck, and on hover: peek,
favorite, add-to-tray. Selection = accent ring; ⌘-click toggles, ⇧-click
ranges, double-click adds to tray. An **"Approximate" badge** appears on tiles
whose preview skipped constructs the renderer can't do faithfully.

![Grouped by deck](docs/screenshots/03-grid-grouped.png)

Grouped mode (`2`, flat is `1`): collapsible per-deck headers.

### Search
Live as-you-type, FTS5-backed. Syntax: `title:` `deck:` `notes:` `body:`
prefixes, `"exact phrase"`, `OR`, `-term`/`NOT`, `after:`/`before:` dates; AND
by default; prefix matching; diacritic-insensitive. Matches highlight in
snippets; active criteria become removable chips in the header strip.

![Search results](docs/screenshots/04-search-results.png)
![Search syntax help](docs/screenshots/05-search-help.png)
![Save search popover](docs/screenshots/06-save-search-popover.png)
![Zero results](docs/screenshots/07-zero-results.png)

Zero-results offers one-click removal of the most restrictive filter, plus
re-index. With the AI model ready, the header shows a segmented **retrieval
mode** control — lexical (Aa) / semantic (✦) / hybrid:

![Retrieval mode toggle](docs/screenshots/30-retrieval-toggle.png)

### Inspector & Peek

![Inspector](docs/screenshots/08-inspector.png)
![Peek modal](docs/screenshots/09-peek-modal.png)
![Similar slides (AI)](docs/screenshots/31-similar-slides.png)

Peek (Space) is the Quick-Look analog: large preview + notes, ← → to walk
results, Space/Esc closes.

### Tray & composition

![Tray populated with warnings](docs/screenshots/13-tray-populated.png)
![Tray switcher](docs/screenshots/14-tray-switcher.png)
![Tray, dark](docs/screenshots/39-tray-dark.png)

Multiple **named trays** (create/rename/delete in the switcher; persisted, with
v1→v2 migration). Undo/redo (⌘Z/⌘⇧Z). Warning pills: **moved** (source file
gone/relocated), **off-size** (aspect ratio differs from the tray majority),
**duplicates** (identical content hash). Tray items are keyed by *deck path +
slide index*, so they survive re-indexing.

### Export

![Export — PowerPoint](docs/screenshots/16-export-pptx.png)
![Export — PDF](docs/screenshots/17-export-pdf.png)
![Export — PNG](docs/screenshots/18-export-png.png)
![Export success](docs/screenshots/19-export-success.png)
![Export, dark](docs/screenshots/36-export-dark.png)

Three formats:
- **PowerPoint** — fidelity-preserving compose (full layout/master/theme
  closure per slide, deduplicated by content hash). Options: include speaker
  notes; for mixed-aspect trays a **fit mode**: *Ensure fit* (letterbox) vs
  *Maximize* (crop).
- **PDF** — real per-slide progress, selectable text.
- **PNG** — one image per slide, width presets 960–2560.
Last-used preset is remembered. Success state gets confetti. Single slides can
also be saved as standalone `.pptx` from the card context menu, or **⌥-dragged
out** of the app as a real `.pptx` file (native only).

### Duplicates

![Duplicates view](docs/screenshots/32-duplicates.png)
![Duplicates, dark](docs/screenshots/37-duplicates-dark.png)

Exact groups (content hash, always available) and near-duplicate clusters
(embedding cosine, model-gated, purple accent). Newest copy is badged.

### Statistics

![Statistics](docs/screenshots/33-stats.png)
![Statistics, dark](docs/screenshots/38-stats-dark.png)

Tiles + last index run, AI-index coverage, problems (skipped files), recent
searches, recent exports, largest decks, approximate-preview count.

### Settings

![Appearance](docs/screenshots/20-settings-appearance.png)
![Library](docs/screenshots/21-settings-library.png)
![Fonts](docs/screenshots/22-settings-fonts.png)
![Updates](docs/screenshots/23-settings-updates.png)
![AI — off](docs/screenshots/24-settings-ai.png)
![AI — consent](docs/screenshots/25-ai-consent.png)
![AI — downloading](docs/screenshots/26-ai-downloading.png)
![AI — ready](docs/screenshots/27-settings-ai-ready.png)
![Settings, dark](docs/screenshots/35-settings-dark.png)

- **Appearance:** System / Light / Dark.
- **Library:** roots (add/remove, per-root exclude patterns editor), clear &
  rebuild index (confirm-gated).
- **Fonts:** every family the library references, with status dot —
  *available* / *downloadable* (curated set, consent per download) / *missing*
  / *embedded*. Add `.ttf/.otf`, remove, reveal folder. Fonts are stored
  app-local, never installed system-wide, and feed both previews and PNG/PDF
  rasterization.
- **Updates:** auto-update toggle (boot + daily check), manual check, phase UI
  (checking → downloading % → installing → ready-to-restart / error).
- **AI (semantic search):** opt-in toggle → explicit consent dialog (≈490 MB
  multilingual-e5-small download from Hugging Face, sha256-pinned) →
  progress → ready (model id, "X of Y slides indexed", re-run indexing,
  delete model). Everything stays on-device. Cross-lingual: German queries
  find English slides and vice versa.

### Other overlays

![Command palette](docs/screenshots/10-command-palette.png)
![Context menu](docs/screenshots/11-context-menu.png)
![Sort menu](docs/screenshots/12-sort-menu.png)
![Confirm dialog](docs/screenshots/28-confirm-dialog.png)
![About](docs/screenshots/29-about.png)
![Peek, dark](docs/screenshots/34-peek-dark.png)

| Overlay | Trigger | Notes |
|---|---|---|
| Command palette | ⌘K | ~14 actions + "Jump to: `<deck>`" per deck |
| Peek modal | Space | arrows navigate within results |
| Export sheet | ⌘E / tray button | format tabs → progress → success/error |
| Settings sheet | ⌘, | five sections, see above |
| About sheet | sidebar / palette | version, update status, links |
| Confirm dialog | destructive actions | z-above sheets; cancel can revert a toggle |
| Context menus | right-click cards & sidebar rows | portal, auto-flipping |
| Tray switcher | tray header dropdown | opens upward |
| Sort menu | header strip | Name / Added / Modified / Most exported; inert during search |
| Save-search / syntax-help popovers | header | anchored to search field |
| Toasts | bottom-center | success/info/error + optional action (e.g. Undo) |

All overlays share one primitive (`components/OverlaySheet.tsx` +
`lib/useDismiss.ts`): backdrop + spring card, Escape/outside-click dismissal
that never leaks into the global shortcut layer, reduced-motion aware.

### Empty states
- **First-run onboarding** (`EmptyState.tsx`): no folders yet — hero + "Add
  folder…" CTA. (Not reachable in mock mode — mock data always populates the
  library — so no screenshot; design changes here must be verified in the
  native app.)
- **Zero results** (screenshot above): query/filters matched nothing.

---

## Keyboard model

Global (work while typing): ⌘F search · ⌘K palette · ⌘I inspector · ⌘,
settings · ⌘T collapse tray · ⌘E export · ⌘R re-index · ⌘Z/⌘⇧Z undo/redo ·
⌘+/⌘− density · ⌘A select all · ⌘⌫ remove selection from tray · ⌘⌃S sidebar.
Text-editing keys (⌘Z/⌘⇧Z/⌘⌫) are *not* hijacked while a text field is focused.

Grid (not while typing): arrows move selection · Space peek · Enter add to
tray · `1` flat / `2` grouped · Esc clears query, then closes inspector.

Cards: click select · ⌘-click toggle · ⇧-click range · double-click add to
tray · right-click menu · drag to tray · **⌥-drag out** = export as `.pptx`
(native only).

---

## Feature inventory (checklist)

Search: FTS5 full-text (title/deck/body/notes, bm25-weighted) · advanced syntax
(fields, phrases, OR/NOT, date ranges) · prefix + diacritic-insensitive ·
semantic & hybrid retrieval (opt-in model) · saved searches · zero-result
recovery.
Organize: favorites (slides + decks, path-keyed) · tags (per-slide, sidebar
filter) · duplicates (exact + near) · statistics.
Compose: multi-select → named trays · undo/redo · reorder · warnings
(moved/off-size/duplicate) · export PPTX (fidelity-preserving, notes, fit
modes) / PDF / PNG · single-slide save & ⌥-drag-out.
Previews: self-contained SVG per slide (theme colors, placeholder inheritance,
images, embedded fonts via @font-face) · approximate-badge honesty · peek.
Library: incremental scanning (mtime+size skip) · live progress · per-root
excludes · moved-deck badges · app-local font management · on-device AI with
explicit consent · auto-updater.

---

## Mock-mode limits

Everything above works in the browser except: native file dialogs (canned
paths), Reveal in Finder (no-op), ⌥-drag-out (hidden), real updater (mock
"up to date"; set `localStorage["slideflow:mockUpdate"]="1"` to see the
update-available flow), macOS menu bar, real vibrancy, and the first-run empty
state (mock library always has content).

---

## Architecture in one paragraph (for context)

`crates/slideflow-core` is a pure-Rust engine (OPC/zip layer → PPTX parser →
SQLite+FTS5 index → SVG renderer → fidelity-preserving composer → PDF/PNG
export via resvg/krilla → optional candle embeddings); it has no GUI
dependencies and is fully testable headless. `apps/desktop` is a thin Tauri 2 +
React shell; every IPC call goes through `src/lib/api.ts` (typed wrappers with
browser-mode mocks in `src/lib/mock.ts`), state lives in zustand stores, and
`src/lib/types.ts` mirrors the Rust `model.rs` field-for-field in snake_case.

*Slideflow — search your slides, compose new decks with original fidelity.*
