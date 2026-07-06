# Slideflow UI screenshot set

Captured 2026-07-06 from the frontend running in browser mock mode (`pnpm dev` @ :1420, `isTauri()===false`), viewport 1440×900. Mock library: 34 slides / 6 decks, with a seeded exact-duplicate pair, a near-duplicate pair, one 4:3 off-size deck (Brand Guidelines), and deterministic "Approximate" preview drops. Export, fonts, semantic-model, scan, and updater flows are all simulated in mock mode.

All shots are light theme unless the filename says `-dark`.

## Main window states
- `01-grid-light.png` — Default slide grid, flat layout, first card selected (accent ring). Light.
- `02-grid-dark.png` — Slide grid with a card selected and inspector open (Similar slides visible). Dark.
- `03-grid-grouped.png` — Group-by-deck layout with collapsible deck headers and folder paths. Light.
- `04-search-results.png` — Search "search": 3 results with highlighted matched-snippets and a removable "search" filter chip. Light.
- `05-search-help.png` — Search-syntax help popover open (field prefixes, phrases, OR/NOT, date ops). Light.
- `06-save-search-popover.png` — Save-this-search popover with name field, Cancel/Save. Light.
- `07-zero-results.png` — Zero-results state for query "zzzzz" with "Search failed? Re-index folders" action. Light.
- `08-inspector.png` — Inspector open on a selected card: large preview, metadata `<dl>`, tags editor, speaker notes, Add to Tray. Light.
- `09-peek-modal.png` — Peek / Quick Look modal: large slide preview, Open source deck / Add to tray, arrow-nav, speaker notes. Light.
- `10-command-palette.png` — Command palette (⌘K) with ~9 actions incl. Re-index, Export, Toggle theme, Sort options. Light.
- `11-context-menu.png` — Right-click card context menu (Add to Tray, Tags, Peek, Save slide as .pptx, Reveal, Copy path, Find other slides). Light.
- `12-sort-menu.png` — Sort menu open (Name / Recently added / Recently modified [checked] / Most exported). Light.
- `13-tray-populated.png` — Tray with 4 slides (exact-duplicate pair + two 4:3 Brand slides); both "2 off-size" and "2 duplicates" warning pills shown. Light.
- `14-tray-switcher.png` — TraySwitcher dropdown (opens upward) listing Tray 1 (4) and Tray 2 (0, active) + New tray. Light.
- `15-sidebar-scan.png` — Live re-index in progress: sidebar "Scanning… 3 of 6 files" progress bar. Light.

## Sheets / dialogs
- `16-export-pptx.png` — Export sheet, PowerPoint tab, with mixed-aspect fit-mode (Ensure fit / Maximize) and Include speaker notes. Light.
- `17-export-pdf.png` — Export sheet, PDF tab ("rendered by Slideflow's preview engine" note). Light.
- `18-export-png.png` — Export sheet, PNG-images tab with 1920px width preset. Light.
- `19-export-success.png` — Export success state: green check, "Your images are ready", confetti, Reveal in Finder. Light.
- `20-settings-appearance.png` — Settings top: Appearance (Theme System/Light/Dark, Grid columns) + start of Library. Light.
- `21-settings-library.png` — Settings Library section: root folder, exclude-patterns editor, Add folder, Clear & rebuild index. Light.
- `22-settings-fonts.png` — Settings Fonts list with all status dots (available / downloadable / embedded / not-installed). Light.
- `23-settings-updates.png` — Settings Updates section: Automatic-updates toggle + "Restart to Update" (mock update ready). Light.
- `24-settings-ai.png` — Settings AI section with Semantic search toggle OFF (before enabling). Light.
- `25-ai-consent.png` — Semantic-model consent dialog ("Download semantic search model?", ≈490 MB, multilingual-e5-small). Light.
- `26-ai-downloading.png` — Semantic model in progress: AI section "Indexing slides… 7 of 34" progress bar + download-complete toast + sidebar bar. Light.
- `27-settings-ai-ready.png` — Settings AI ready: model id + "34 of 34 slides indexed", Re-run indexing / Delete model. Light.
- `28-confirm-dialog.png` — Destructive ConfirmDialog ("Delete semantic search model?", red Delete button). Light.
- `29-about.png` — About sheet: logo, Version dev, "Restart to Update" (mock update ready), See-what's-new, links, Buy me a coffee. Light.

## AI-dependent states (semantic model ready)
- `30-retrieval-toggle.png` — Header retrieval segmented control (Aa / ✦ / Aa✦, hybrid selected); full window. Light.
- `31-similar-slides.png` — Inspector "Similar slides" AI section with ranked % matches. Light.
- `32-duplicates.png` — Duplicates view: Exact duplicate (2 copies) AND Near duplicate (2 copies · 94% similar) groups. Light.

## Views
- `33-stats.png` — Statistics view: tiles, last-index-run, AI index Ready, Problems, Recent searches, Recent exports. Light.

## Dark-mode pairs
- `34-peek-dark.png` — Peek modal. Dark.
- `35-settings-dark.png` — Settings sheet (Appearance/Library/Fonts). Dark.
- `36-export-dark.png` — Export sheet, PowerPoint tab with fit-mode. Dark.
- `37-duplicates-dark.png` — Duplicates view (exact + near groups). Dark.
- `38-stats-dark.png` — Statistics view. Dark.
- `39-tray-dark.png` — All Slides grid with the docked tray populated (4 slides: exact-duplicate pair + two 4:3 Brand slides) showing both "2 off-size" and "2 duplicates" warning pills; inspector closed. Dark.

## Skipped
None — all 39 requested states were captured.

Note: `05-first-run onboarding` / EmptyState is intentionally not in the requested list; per the screen inventory it is unreachable in mock mode (removing the root still leaves mock decks, so `hasLibrary` stays true) and was not requested.
