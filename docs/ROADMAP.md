# Slideflow — Improvement Roadmap

> Brainstormed improvements, grounded in the codebase as of v0.2.3.
> Reviewed 2026-07-05 against v0.3.0: stale assumptions corrected,
> mixed-slide-dimensions and renderer follow-ups added.
> Trimmed 2026-07-05 after roadmap wave 1 shipped: Clear & rebuild index,
> Settings sheet, scan diagnostics, mixed-dimensions detection, dropped-construct
> telemetry, export presets, per-folder excludes, and sort views are done (see
> CHANGELOG "Unreleased") and removed from this menu. Item numbers of the
> remaining entries are kept stable for cross-references.
> Extended 2026-07-05 (wave-2 brainstorm): items 21–30 added after a UX audit —
> multi-select, grid keyboard nav, card context menus, export history, and
> onboarding/empty states already exist and were dropped from the brainstorm.
> Trimmed again 2026-07-05 after roadmap wave 2 shipped: advanced search (#5),
> semantic search + find-similar (#6), mixed-dims scaling (#7), PDF/PNG export
> (#8), duplicate detection (#9), saved searches (#10), named trays (#11),
> embedded fonts (#13), drag-out (#14), tags (#15), and the CLI (#19) are done
> (see CHANGELOG "Unreleased") and removed from this menu. Item numbers stay
> stable; #31 added (tray ⌥-drag, deferred from #14).

This document collects candidate improvements from two angles — the everyday
user who searches, picks, and exports, and the power user who lives in the app
all day — plus a staged proposal for local AI. Each near-term item comes with a
short implementation sketch naming the real modules involved, so any of them
can be picked up without re-discovering the codebase. Nothing here is a
commitment; it's a prioritized menu.

---

## Everyday-user improvements

- **Copy slide as image (21).** The card context menu can copy only the source
  *path* today (`SlideCard.tsx`). With the wave-2 rasterizer
  (`export.rs::render_slide_png`) in place, ⌘C / "Copy as image" can put a PNG
  on the clipboard (Tauri clipboard plugin or `arboard`) — the fastest possible
  share path for a single slide.
- **Library metadata backup/restore (22).** Favorites, tags, saved searches,
  and named trays live in the SQLite library and `localStorage`; a machine move
  or a corrupted DB loses all curation even though decks are just files.
  Export/import the user-authored metadata as one JSON file (keyed by deck path
  + slide index, matching the rescan-safe favorites convention).
- **Slide-version awareness (23).** When a rescan re-indexes a changed deck,
  tray items and favorites silently keep pointing at the new content. With the
  per-slide `content_hash` shipped by wave 2 (#9), `useTray.reconcile()` can
  flag picks whose hash changed — "a newer version of this slide exists" — and
  offer a one-click refresh of the pick.

---

## Power-user improvements

- **Tray templates.** Named trays shipped in wave 2; templates ("standard
  intro + closing" as a reusable starting point) are the natural follow-up.
- **Theme / brand browser (26).** The index already knows each slide's
  layout/master/theme lineage; grouping the library by master or theme (a
  sidebar dimension next to folders) lets users spot off-brand decks and jump
  to "all slides still on the 2023 template".
- **Hide / exclude slides (27).** A per-slide "hide from search" flag (schema
  column + `push_filters` clause + context-menu toggle) removes boilerplate
  (dividers, legal pages) from results without touching the source files.
- **Menu-bar quick search (28).** A global hotkey opening a floating
  Spotlight-style search palette (reusing `CommandPalette.tsx` + the search
  API) makes the library reachable from inside PowerPoint without switching
  apps. macOS first (Tauri global-shortcut plugin).

---

## Renderer & engine follow-ups

- **Obfuscated embedded fonts (ODTTF).** Wave 2 extracts plainly-embedded
  fonts; the obfuscated ODTTF form some exporters produce is detected and
  skipped with a telemetry note. Deobfuscation (GUID-XOR header) is small and
  well-documented if such decks show up in practice.
- **OCR for image-only slides (30, research).** Slides whose text is baked
  into screenshots/exports are invisible to FTS and embeddings. Local OCR
  during scan would fix that, but every option has a real cost (tesseract:
  heavy C dep; macOS Vision: not cross-platform; pure-Rust OCR: immature).
  Investigate-only until a dependency fits the pure-Rust/no-system-libs rule.

---

## UX & platform polish

- **Peek modal, next stage (24).** `PeekModal.tsx` is a `max-w-5xl` modal with
  arrow navigation. A true fullscreen mode plus zoom/pan (the SVG previews are
  resolution-independent) would make it a real review surface.
- **Shift+arrow range selection (25).** The grid has an anchor-based
  multi-select (click modifiers) and arrow-key navigation, but no
  keyboard range extension; `useApp.rangeSelect` already exists — wire ⇧+arrows
  to it in the `App.tsx` key handler.
- **Tray ⌥-drag-out (31).** Wave 2's drag-out works from grid cards; tray
  items are framer-motion `Reorder.Item`s (pointer-gesture drag, not HTML5
  draggable), so ⌥-dragging out of the tray needs a gesture-integration pass
  rather than the grid's dragstart hook.
- **Accessibility pass (29).** VoiceOver labels on cards/tray/sheets, focus
  order after modal open/close, and a reduced-motion audit. Nothing is
  actively hostile today, but none of it has been verified.

---

## AI, Stage 2 — generative assist (later, opt-in, speculative)

Stage 1 (local semantic search, find-similar, near-duplicates — candle +
multilingual-e5-small, download-on-first-enable) shipped in wave 2. With those
embeddings in place, a generative layer becomes plausible: natural-language
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
| 20 | Generative assist via local model server (AI Stage 2) | L | Unknown | Opt-in experiment on top of shipped Stage 1 |
| 21 | Copy slide as image | S | Medium | Rides the wave-2 rasterizer |
| 22 | Library metadata backup/restore | S | Medium | One JSON file, path+index keyed |
| 23 | Slide-version awareness | M | Medium | Needs #9's content_hash (wave 2) |
| 24 | Peek modal fullscreen + zoom | S | Medium | SVG previews scale for free |
| 25 | Shift+arrow range selection | S | Low | `rangeSelect` exists, wire the keys |
| 26 | Theme / brand browser | M | Medium | Lineage data already indexed |
| 27 | Hide / exclude slides | S–M | Medium | Schema flag + filter + toggle |
| 28 | Menu-bar quick search | M | Medium | Global shortcut + palette reuse |
| 29 | Accessibility pass | M | Medium | VoiceOver, focus order, reduced motion |
| 30 | OCR for image-only slides | L | Unknown | Research: no dependency fits yet |
| 31 | Tray ⌥-drag-out | S–M | Low–Medium | Deferred from #14; framer-motion gesture integration |

**Suggested order:** the S-effort quick wins first (21, 22, 25, 24), then 23
(slide-version awareness — the content hashes it needs shipped in wave 2), 26/27
by demand; 20 (AI Stage 2) stays an experiment until Stage 1 usage proves the
appetite.
