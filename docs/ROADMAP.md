# Slideflow — Improvement Roadmap

> Brainstormed improvements, grounded in the codebase as of v0.2.3.
> Reviewed 2026-07-05 against v0.3.0: stale assumptions corrected,
> mixed-slide-dimensions and renderer follow-ups added.
> Trimmed 2026-07-05 after roadmap wave 1 shipped: Clear & rebuild index,
> Settings sheet, scan diagnostics, mixed-dimensions detection, dropped-construct
> telemetry, export presets, per-folder excludes, and sort views are done (see
> CHANGELOG "Unreleased") and removed from this menu. Item numbers of the
> remaining entries are kept stable for cross-references.

This document collects candidate improvements from two angles — the everyday
user who searches, picks, and exports, and the power user who lives in the app
all day — plus a staged proposal for local AI. Each near-term item comes with a
short implementation sketch naming the real modules involved, so any of them
can be picked up without re-discovering the codebase. Nothing here is a
commitment; it's a prioritized menu.

---

## Near-term, high-value

### 7. Mixed slide dimensions — scale on export

**What:** Normalize slide sizes when composing: scale automatically when
aspect ratios match, ask the user when they don't.

**Why:** Wave 1 shipped the detection half — mismatched tray picks are badged
and the compose report warns — but the exported deck still takes its slide
size silently from the first source deck
(`crates/slideflow-core/src/pptx/composer.rs`). Shape coordinates are absolute
EMU, so nothing adapts: a 4:3 slide in a 16:9-first tray still leaves a dead
right gutter, and a 16:9 slide in a 4:3-first tray still runs off the right
edge. The user is warned now, but not helped.

**Implementation sketch:** a per-source-deck uniform scale factor applied to
the whole copied closure — slide *and* layout *and* master — covering shape
offsets/extents, font sizes, and line widths (this is what PowerPoint itself
does on resize). Same aspect ratio → scale automatically and note it in the
report. Different aspect ratio → the export sheet asks, in PowerPoint's own
vocabulary: **Ensure fit** (uniform scale, centered) or **Maximize** (fill,
crop). Scaling per source deck keeps content-hash deduplication intact, since
closures are copied per deck.

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
- **CLI companion.** `slideflow-core` is a pure-Rust crate with no GUI or
  GTK/WebKit dependency — a small `slideflow` binary exposing `index`, `search`,
  and `compose` would enable scripting ("build me a deck from these slide IDs in
  CI") at very low cost, and doubles as a debugging tool.

---

## Renderer & engine follow-ups

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

Index implications: adding an embeddings table is a schema and pipeline change
— it rides the `user_version` migration runner shipped in wave 1, plus an
`INDEX_VERSION` bump to force a full re-parse (or the user-triggered Clear &
rebuild, also shipped). Embedding happens during the scan, so incremental
indexing keeps working unchanged.

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
| 5 | Advanced search syntax | M | High | FTS5 does the heavy lifting |
| 6 | Semantic search + find-similar (AI Stage 1) | L | High | Fully offline; rides migrations + `INDEX_VERSION` |
| 7 | Mixed slide dimensions — scale on export | M | Medium | Detection shipped in wave 1; scale the closure, ask on aspect mismatch |
| 8 | PDF / image export | M | High | v0.3.0 fidelity makes SVG output presentation-grade |
| 9 | Duplicate slide detection | M | Medium | Cheap after #6; a hash-based version is possible sooner |
| 10 | Saved searches / smart folders | S | Medium | Pairs with #5 |
| 11 | Multiple named trays | M | Medium | Power-user compose workflow |
| 13 | Font fidelity: embedded fonts + metrics | M | Medium | `fntdata` extraction; WebKit fallback verification still open |
| 14 | Drag slide out of app | M | Medium | Platform drag-and-drop plumbing |
| 15 | Tags / collections | M | Medium | Schema + sidebar work |
| 19 | CLI companion | M | Low–Medium | Cheap thanks to the pure-Rust engine; niche audience |
| 20 | Generative assist via local model server (AI Stage 2) | L | Unknown | Opt-in experiment on top of #6 |

**Suggested order:** 5 (advanced search) while 6 (embeddings) is developed;
7/8 and the renderer follow-up (13) ride alongside as engine work; then the
rest by demand.
