# Slideflow — Improvement Roadmap

> Brainstormed improvements, grounded in the codebase as of v0.2.3.
> Reviewed 2026-07-05 against v0.3.0: no item below has shipped yet (v0.3.0's
> auto-update and renderer-fidelity work was off-roadmap); stale assumptions
> corrected, mixed-slide-dimensions and renderer follow-ups added.

This document collects candidate improvements from two angles — the everyday
user who searches, picks, and exports, and the power user who lives in the app
all day — plus a staged proposal for local AI. Each near-term item comes with a
short implementation sketch naming the real modules involved, so any of them
can be picked up without re-discovering the codebase. Nothing here is a
commitment; it's a prioritized menu.

---

## Near-term, high-value

### 1. Clear & rebuild index

**What:** A user-facing action that wipes the search index and thumbnail cache
and kicks off a full re-scan from scratch, behind a confirmation dialog.

**Why:** Today the only re-index paths are incremental. "Re-index all folders"
(`⌘R` in the command palette) and the sidebar's per-folder "Re-index" both call
the same scan that skips any file whose mtime + size hash is unchanged — by
design. That's the right default, but it means a corrupted database, a stale or
wrong-looking preview, or an index written by an older parser version can't be
fixed from inside the app. The documented escape hatch ("deleting the database
fully resets the index") is a manual filesystem operation in a hidden
platform-specific directory — exactly the kind of thing an app should do for
you.

**Implementation sketch:**

- New method on `Library` in `crates/slideflow-core/src/index.rs` — e.g.
  `clear()` — that drops and recreates the schema (or deletes and re-opens
  `library.db`) and empties the thumbnail directory. (Since v0.3.0 thumbnails
  are content-addressed and `sweep_thumbs` GCs stale entries after every scan,
  so they self-heal against staleness — the wipe clears them anyway for a true
  reset.) Roots should survive the
  wipe (or be re-registered) so the follow-up scan knows what to index.
  Internally this is the user-facing cousin of the existing `INDEX_VERSION`
  bump, which already forces full re-parses at the code level.
- New Tauri command in `apps/desktop/src-tauri/src/commands.rs`, mirrored in
  `apps/desktop/src/lib/api.ts` (the single IPC wrapper module).
- UI entry points: a "Clear index & rebuild…" action in
  `components/CommandPalette.tsx` next to "Re-index all folders", and a button
  in the Settings sheet (below). Destructive → always confirm, and mention that
  favorites keyed by path survive but scan/search history may not.

### 2. Settings sheet

**What:** A proper preferences surface, modeled on the existing
`components/AboutSheet.tsx`.

**Why:** There is currently no settings UI at all — theme and grid density live
in `localStorage` and are only reachable through the command palette, and
library maintenance is scattered across sidebar context menus. A settings sheet
gives Clear-index a discoverable home and creates the anchor point every later
item needs (exclude patterns, export presets, AI toggles).

**Implementation sketch:** New `SettingsSheet.tsx` following the `AboutSheet`
pattern, opened from the command palette and a keyboard shortcut (`⌘,`).
Sections: Appearance (theme, grid columns — already in `stores/useApp.ts`),
Library (folder list, Clear & rebuild index), Updates (auto-update on/off,
check-now — v0.3.0 shipped auto-update always-on, with the only manual check
buried in the About sheet), and room to grow.

### 3. Index health & scan diagnostics

**What:** Surface what the scanner actually did — files indexed, skipped,
failed to parse — instead of silently swallowing errors.

**Why:** When a deck doesn't show up in search, the user has no way to learn
why (corrupt file? unsupported content? permission error?). More of this
already exists than it looks: the scanner emits per-file failures as
`ScanEvent::Skipped { reason }`, and aggregate counts land in the
`scan_history` table, displayed by `components/StatsView.tsx` — but the
frontend discards the reason (`stores/useApp.ts` only keeps the last path) and
failures are never persisted.

**Implementation sketch:** Persist per-file skip/parse-failure reasons during
`Library::scan()` in `index.rs`, expose them through `get_stats_overview` (or a
new command), and render a "problems" section in `StatsView` or the Settings
sheet. Smaller than it sounds — the reasons are already produced, they just
get dropped on the floor.

### 4. Mixed slide dimensions in the tray (detect & warn, then scale)

**What:** Detect when tray picks come from decks with different slide sizes
(`p:sldSz`), badge the mismatched picks in the tray, and warn in the compose
report. As a follow-up, normalize on export: scale automatically when aspect
ratios match, ask the user when they don't.

**Why:** A `.pptx` has exactly one slide size, and today the composer takes it
silently from the **first source deck**
(`crates/slideflow-core/src/pptx/composer.rs`). Shape coordinates are absolute
EMU, so nothing adapts: standard 4:3 (9144000×6858000) and 16:9
(12192000×6858000) share a height but not a width — a 4:3 slide in a
16:9-first tray leaves a dead right gutter, and a 16:9 slide in a 4:3-first
tray runs off the right edge. The user gets a wrong-looking deck with no hint
why.

**Implementation sketch:**

- Detection (near-term, S): the parser already reads `p:sldSz` per deck
  (`pptx/parser.rs`); store deck width/height in the index (`decks` table,
  `model.rs` + `lib/types.ts` in lockstep), badge tray cards whose deck size
  differs from the first pick's, and append a warning to `ComposeReport`.
- Normalization (follow-up, M): a per-source-deck uniform scale factor applied
  to the whole copied closure — slide *and* layout *and* master — covering
  shape offsets/extents, font sizes, and line widths (this is what PowerPoint
  itself does on resize). Same aspect ratio → scale automatically and note it
  in the report. Different aspect ratio → the export sheet asks, in
  PowerPoint's own vocabulary: **Ensure fit** (uniform scale, centered) or
  **Maximize** (fill, crop). Scaling per source deck keeps content-hash
  deduplication intact, since closures are copied per deck.

---

## Everyday-user improvements

- **PDF / image export of the tray.** The composer
  (`crates/slideflow-core/src/pptx/composer.rs`) outputs `.pptx` only, but the
  renderer already produces self-contained SVGs per slide — turning the tray
  into a PDF or a folder of PNGs is a natural extension for sharing and
  printing without PowerPoint. After the v0.3.0 fidelity overhaul
  (layout/master rendering, full text inheritance, tables, gradients, EMF),
  that SVG output is presentation-grade — this is now a credible feature
  rather than a rough proxy.
- **Drag a slide out of the app.** Dragging a slide card to Finder/Explorer or
  straight into an open PowerPoint window (as a single-slide `.pptx`) would
  make one-off reuse instant — no tray, no export sheet.
- **Duplicate slide detection.** The engine already hashes part content for
  export deduplication; the same signal can power a "this slide appears in 6
  decks — here's the newest copy" view, and warn when the tray contains
  near-identical picks.
- **Tags / named collections.** Favorites is a single flat list. Letting users
  tag slides ("intro", "pricing", "2026 kickoff") and browse tags in the
  sidebar turns the library into a curated asset store.
- **Sort options & smart views.** Recently added, recently modified, most
  exported (export history is already recorded) — cheap wins on top of existing
  data.

---

## Power-user improvements

- **Advanced search syntax.** Fielded queries (`title:roadmap`,
  `deck:kickoff`, `notes:todo`), boolean operators, and date filters. FTS5
  supports column filters and boolean queries natively, so this is mostly query
  building plus a small syntax layer in the search bar.
- **Saved searches / smart folders.** Persist a query as a sidebar entry —
  pairs naturally with advanced syntax and reuses the existing sidebar tree.
- **Multiple named trays.** The tray (`stores/useTray.ts`) is a single
  persisted list with undo/redo. Power users assembling several decks at once
  want named trays they can switch between, plus tray templates ("standard
  intro + closing").
- **Per-folder exclude patterns.** Glob ignores per root (e.g. `**/archive/**`,
  `~*`) configured in the Settings sheet, applied in the walk inside
  `Library::scan()`.
- **Export presets.** Remember title, include-notes, and target-folder choices
  in the export sheet; offer one-keystroke re-export with the last preset.
- **CLI companion.** `slideflow-core` is a pure-Rust crate with no GUI or
  GTK/WebKit dependency — a small `slideflow` binary exposing `index`, `search`,
  and `compose` would enable scripting ("build me a deck from these slide IDs in
  CI") at very low cost, and doubles as a debugging tool.

---

## Renderer & engine follow-ups

Both born out of the v0.3.0 fidelity rounds:

- **Dropped-construct telemetry.** The renderer silently skips constructs it
  doesn't support (`render/mod.rs`). Counting drops per slide and surfacing
  them — in the stats view, or as an "approximate preview" badge on affected
  slides — would show where the next fidelity round should go instead of
  waiting for user reports. (Planned in the original fidelity R0 scope, never
  built.)
- **Font fidelity, next stage.** v0.3.0 added narrow-font fallback chains and
  per-family width factors. Remaining: extract **embedded fonts** from PPTX
  (`fntdata` parts) and use them in previews for pixel-true text, and verify
  the in-app WebKit fallback path (Arial Narrow on macOS) actually kicks in.

---

## AI: local semantic search (staged)

The constraint that makes this interesting: Slideflow's core promise is
**local-first, offline, no telemetry**. Any AI feature has to keep that promise
— which rules out cloud APIs by default and points at small local models.

### Stage 1 — semantic search & "find similar" (recommended)

Embed each slide's extracted text (title, body, notes — already produced by
`pptx/parser.rs`) with a small local embedding model via an ONNX runtime (e.g.
`fastembed`-style MiniLM, tens of MB, no external runtime or user setup). Store
vectors in the existing SQLite database alongside the FTS5 table and rank
search results hybrid: bm25 for exact/lexical matches, cosine similarity for
meaning. This unlocks:

- **Search by meaning** — "customer churn" finds the slide that says
  "attrition", "org chart" finds "team structure".
- **"Find similar slides"** on any slide card — the best discovery feature a
  slide library can have.
- **Near-duplicate detection** — clusters of almost-identical slides across
  deck versions, feeding the duplicate-detection idea above.

Index implications: adding an embeddings table is a schema and pipeline change,
so it should ride the existing `INDEX_VERSION` mechanism in `index.rs` (bumping
it forces a full re-parse — or, once shipped, a user-triggered Clear & rebuild).
Embedding happens during the scan, so incremental indexing keeps working
unchanged.

### Stage 2 — generative assist (later, opt-in, speculative)

With embeddings in place, a generative layer becomes plausible: natural-language
deck assembly ("draft a 10-slide intro to our platform from my library" →
retrieve candidates by similarity, let a model pick and order them),
auto-tagging, and slide summarization. To stay local-first this would integrate
with a **user-run model server (e.g. Ollama)** rather than bundling a large
model: strictly opt-in, off by default, configured in the Settings sheet, and
degrading gracefully to Stage-1 retrieval when no server is present. Quality
will vary with the user's local model, so this stage should be treated as an
experiment layered on Stage 1 — never a dependency of it.

---

## Prioritization

| # | Item | Effort | Impact | Notes |
| --- | --- | --- | --- | --- |
| 1 | Clear & rebuild index | S | High | Unblocks recovery; prerequisite plumbing for schema changes |
| 2 | Settings sheet | S | High | Anchor point for almost everything below, incl. update preferences |
| 3 | Index health / diagnostics | S | Medium | Half-built: skip reasons already emitted, just unsurfaced |
| 4 | Mixed slide dimensions — detect & warn | S | High | Composer silently takes the first deck's size today |
| 5 | Advanced search syntax | M | High | FTS5 does the heavy lifting |
| 6 | Semantic search + find-similar (AI Stage 1) | L | High | Fully offline; rides `INDEX_VERSION` |
| 7 | Mixed slide dimensions — scale on export | M | Medium | Builds on #4; scale the closure, ask on aspect mismatch |
| 8 | PDF / image export | M | High | Upgraded: v0.3.0 fidelity makes SVG output presentation-grade |
| 9 | Duplicate slide detection | M | Medium | Cheap after #6; a hash-based version is possible sooner |
| 10 | Saved searches / smart folders | S | Medium | Pairs with #5 |
| 11 | Multiple named trays | M | Medium | Power-user compose workflow |
| 12 | Dropped-construct telemetry | S | Medium | Guides fidelity rounds; enables "approximate preview" badge |
| 13 | Font fidelity: embedded fonts + metrics | M | Medium | `fntdata` extraction; WebKit fallback verification still open |
| 14 | Drag slide out of app | M | Medium | Platform drag-and-drop plumbing |
| 15 | Tags / collections | M | Medium | Schema + sidebar work |
| 16 | Export presets | S | Low | Quality-of-life |
| 17 | Per-folder exclude patterns | S | Low | Needs Settings sheet |
| 18 | Sort & smart views | S | Low | Uses existing history data |
| 19 | CLI companion | M | Low–Medium | Cheap thanks to the pure-Rust engine; niche audience |
| 20 | Generative assist via local model server (AI Stage 2) | L | Unknown | Opt-in experiment on top of #6 |

**Suggested order:** 1 → 2 → 3 → 4 land as one "library maintenance & trust"
release; then 5 (advanced search) while 6 (embeddings) is developed; 7/8 and
the renderer follow-ups (12/13) ride alongside as engine work; then the rest
by demand.
