# Changelog

All notable user-facing changes to Slideflow. Versions follow [semver](https://semver.org); dates are ISO.

## [Unreleased]

### Added

- **Settings.** A proper preferences sheet (`⌘,`, command palette, or *Slideflow → Settings…* in the macOS menu bar): theme, grid density, library folders, per-folder exclude patterns, and update preferences. *About* is also reachable from the menu bar now.
- **Clear index & rebuild.** One action (Settings or command palette) wipes the search index and preview cache and re-scans from scratch — the recovery tool for stale previews or a corrupted index. Starred slides and decks survive.
- **Scan problems.** Files that fail to index no longer disappear silently: skip reasons are stored and listed in Statistics, and the sidebar shows a live skip count while scanning.
- **Per-folder exclude patterns.** Glob ignores per library folder (e.g. `**/archive/**`, `~$*`), applied during scanning — excluded subtrees are not even walked.
- **Mixed slide sizes, surfaced.** Tray picks whose deck slide size differs from the first pick are badged (a 4:3 slide in a 16:9 composition would silently mis-fit before), with a header hint and the existing export-report warning.
- **Approximate-preview badge.** Slides whose preview silently skips unsupported constructs (charts, SmartArt, OLE, exotic images) are marked, and Statistics shows what the renderer dropped and where.
- **Sort options.** Browse by name, recently added, recently modified, or most exported (export counting starts with this release).
- **Export presets.** The export sheet remembers your last title, include-notes choice, and target folder.

### Improved

- Library databases now migrate in place between app versions (schema versioning) — no more manual database deletion after upgrades.
- Update checks can be disabled in Settings; disabling also cancels an already-downloaded pending install.
- Renderer telemetry descends into `mc:AlternateContent`, so modern PowerPoint charts/media are classified correctly instead of flagged as unknown shapes.

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
