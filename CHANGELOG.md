# Changelog

All notable user-facing changes to Slideflow. Versions follow [semver](https://semver.org); dates are ISO.

## [Unreleased]

## [0.4.0] — 2026-07-06

### Added

- **Semantic search (optional, fully local).** Enable AI search in Settings to find slides by meaning — "customer churn" finds the deck that says "Kundenabwanderung". A one-time ≈490 MB model download (multilingual, English + German); everything runs and stays on your machine, off by default. Search modes: exact, semantic, or hybrid. Each slide card gains *Find similar (AI)*, and the Inspector shows similar slides.
- **Duplicate detection.** A new *Duplicates* view groups identical slides across decks (and, with AI enabled, near-identical ones), highlighting the newest copy. The tray warns when two picks are the same slide.
- **Advanced search syntax.** `title:`, `deck:`, `notes:`, `body:` field scoping, `"exact phrases"`, `OR`, `NOT`/`-term`, and `before:`/`after:` date bounds — see the `?` help in the search box.
- **Saved searches.** Bookmark the current query + filters from the search box; saved searches live in the sidebar with rename/delete.
- **Multiple named trays.** Build several compositions in parallel: create, rename, switch, and delete trays from the tray header; undo/redo is tracked per tray. Your existing tray carries over as "Tray 1".
- **Slide tags.** Tag slides from the Inspector (with autocomplete), browse tags in the sidebar, and filter search results by tag. Tags survive rescans and *Clear index & rebuild*.
- **PDF and image export.** The export sheet now offers PowerPoint, PDF (selectable text, one page per slide), or PNG images (1280/1920/3840 px wide) — with real progress. Decks' embedded fonts are used when rasterizing.
- **Mixed slide sizes, fixed on export.** Slides from differently-sized decks are now scaled onto the output canvas: same aspect ratio scales automatically; mixed aspect ratios ask — *Ensure fit* (letterbox) or *Maximize* (fill, may crop). Fonts, line widths, tables, and effects scale along, like PowerPoint itself does.
- **Drag a slide out of the app** (macOS-first). Hold ⌥ and drag any slide card to Finder or an open PowerPoint window — it lands as a single-slide .pptx with full formatting. Or right-click → *Save slide as .pptx…* on any platform.
- **Embedded fonts in previews.** Decks that embed their fonts now render previews with the real typefaces instead of fallbacks.
- **Font management.** A Fonts section in Settings lists the fonts Slideflow uses for previews and export, and lets you add your own (from file) or download common families on demand, and remove them again — so slides that reference fonts you don't have installed still render correctly.
- **`slideflow` CLI.** A companion command-line tool: `slideflow index/search/compose/render/stats` — scriptable library indexing, advanced-syntax search (`--json`), and deck composition without the app.
- **Settings.** A proper preferences sheet (`⌘,`, command palette, or *Slideflow → Settings…* in the macOS menu bar): theme, grid density, library folders, per-folder exclude patterns, and update preferences. *About* is also reachable from the menu bar now.
- **Clear index & rebuild.** One action (Settings or command palette) wipes the search index and preview cache and re-scans from scratch — the recovery tool for stale previews or a corrupted index. Starred slides and decks survive.
- **Scan problems.** Files that fail to index no longer disappear silently: skip reasons are stored and listed in Statistics, and the sidebar shows a live skip count while scanning.
- **Per-folder exclude patterns.** Glob ignores per library folder (e.g. `**/archive/**`, `~$*`), applied during scanning — excluded subtrees are not even walked.
- **Mixed slide sizes, surfaced.** Tray picks whose deck slide size differs from the first pick are badged (a 4:3 slide in a 16:9 composition would silently mis-fit before), with a header hint and the existing export-report warning.
- **Approximate-preview badge.** Slides whose preview silently skips unsupported constructs (charts, SmartArt, OLE, exotic images) are marked, and Statistics shows what the renderer dropped and where.
- **Sort options.** Browse by name, recently added, recently modified, or most exported (export counting starts with this release).
- **Export presets.** The export sheet remembers your last title, include-notes choice, and target folder.

### Improved

- **Real typefaces in grid thumbnails.** Fonts embedded into previews (a deck's own embedded fonts, fonts you added or downloaded in Settings, and the bundled Calibri/Cambria substitutes) are now subsetted per slide to just the characters that slide uses — a few KB instead of hundreds. That makes them cheap enough to include in every grid thumbnail, so the grid now shows the same real typefaces as the large preview, and large previews shrink substantially. Existing preview caches rebuild automatically.
- Library databases now migrate in place between app versions (schema versioning) — no more manual database deletion after upgrades.
- Update checks can be disabled in Settings; disabling also cancels an already-downloaded pending install.
- Renderer telemetry descends into `mc:AlternateContent`, so modern PowerPoint charts/media are classified correctly instead of flagged as unknown shapes.
- **Office fonts without embedding.** Slides that reference Calibri or Cambria but don't embed them now render with metric-compatible substitutes (bundled Carlito/Caladea), and other unembedded fonts fall back through sensible chains instead of a single generic face.
- **More image formats in previews.** TIFF images and WMF-embedded bitmaps now render instead of being dropped.

### Fixed

- PDF and PNG export could lose slide background photos whose deck declared the wrong image MIME type (e.g. a JPEG labelled `image/png`); the renderer now sniffs the actual bytes.
- Shapes marked hidden in PowerPoint are no longer drawn in previews.
- A round of 47 audited correctness fixes across the engine, host, and frontend — most notably: scanning while a library volume is unmounted no longer silently empties the library, and scan errors now surface in the UI.

## [0.3.0] — 2026-07-05

### Added

- **Automatic updates.** Slideflow checks GitHub releases quietly in the background (on launch and daily), downloads new versions silently, and prompts once to restart. Ignored updates install on quit (macOS/Linux). Manual check via *About → Check for Updates…*. Linux: AppImage self-updates; deb/rpm stay on the package manager.

### Improved

- **Slide preview fidelity, overhauled end to end:**
  - Slide layouts and masters render — backgrounds, logos, and template artwork appear in previews.
  - Text: per-run fonts/sizes/colors, full style inheritance, real bullets, autofit, highlights, bold-aware widths, slide-number fields; no more clipped or overflowing text.
  - Tables, drop shadows, gradients, pattern fills, theme color transforms, SVG vector images, EMF-embedded bitmaps, and image crops now render.
  - Geometry: custom shapes, rounded-corner accuracy, dashed connectors with line endings, and pictures clipped to angled/custom shapes.
- **Performance:** previews are served as downscaled files instead of multi-megabyte inline images (93 MB photo slide → <0.5 MB), scrolling large libraries is smoother, and render work is bounded.

### Fixed

- Previews could show the wrong slide after decks changed on disk (thumbnail cache is now content-addressed).
- Linux release builds failed on ubuntu-22.04 due to conflicting appindicator packages in CI.

## [0.2.3] — 2026-07-03

### Added

- About dialog with version, website link, and Buy Me a Coffee.
- MIT license; reworked README with badges, architecture overview, and screenshots.

## [0.2.2] — 2026-07-02

Initial public release: indexes `.pptx` folders into a searchable slide library (SQLite + FTS5), full-text search with previews, and composing new decks from picked slides with original layout/master/theme preserved. Favorites, statistics view, drag-and-drop tray, CI + multi-platform release pipeline.
