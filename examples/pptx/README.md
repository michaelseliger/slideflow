# Example PPTX corpus

A directory of complex, varied `.pptx` files for manual and automated testing of
Slideflow's indexing, search, thumbnail rendering, and composition/export.

There are two groups:

- **Generated decks** (this directory) — reproducible with python-pptx via
  [`scripts/generate_examples.py`](../../scripts/generate_examples.py).
- **Downloaded decks** ([`real/`](./real)) — complex third-party presentations,
  fetched best-effort by
  [`scripts/download_real_examples.py`](../../scripts/download_real_examples.py).

Every **generated** deck deliberately sets a docProps `title` that is
**different** from its file name, plus an `author`. This exercises the app's
"show the real file name, not the docProps title" behaviour — a deck whose
title happened to equal its file name would be a silent hole in the corpus.

## Regenerating

```sh
python3 scripts/generate_examples.py            # (re)generate every deck
python3 scripts/generate_examples.py tables     # regenerate a single deck
python3 scripts/generate_examples.py --list     # list deck names
```

Requires **python-pptx ≥ 1.0**. **Pillow** is optional and used to synthesise
images; without it the image-bearing decks fall back to a 1×1 placeholder PNG.
Generation is deterministic (fixed RNG seed) so re-runs don't change file sizes.

To re-fetch the real-world decks:

```sh
python3 scripts/download_real_examples.py
```

## Generated decks

| File | What's inside | What it stress-tests |
|------|---------------|----------------------|
| `text-hierarchy.pptx` | 8 slides: title & section layouts, 5-level bullets, explicit fonts/sizes/colors/bold/italic, mixed runs in one paragraph, an explicit line break (`<a:br>`), a long wrapping paragraph, all four alignments, a dense outline. | Text extraction, run/paragraph formatting, bullet levels, index/search over body text. |
| `tables.pptx` | 4 slides: header row + zebra cell fills, horizontally **and** vertically merged cells with custom column widths, and a large 10×8 table. | Table parsing, merged-cell handling, cell fills, large grids. |
| `charts.pptx` | 7 slides: clustered bar (with legend), stacked bar, line, pie (data labels), doughnut, and XY scatter. Each chart embeds a real `.xlsx` workbook. | Chart parsing; **composer** recursive rels-following & dedup of embedded workbooks. |
| `images.pptx` | 8 slides: PNG with real alpha transparency, JPEG gradient "photo", the same logo reused across 3 slides (twice each), a rotated image, and a 1×1 px image. | Image decoding, thumbnails, **content-hash dedup** of reused media, rotation, degenerate sizes. |
| `shapes.pptx` | 6 slides: autoshape gallery (rounded rect, arrow, 5-point star, chevron, pentagon, oval), gradient/solid fills, dashed outline, an explicit outer shadow, elbow & straight connectors, a group shape, and a freeform polygon. | Shape geometry, fills/lines/effects, connectors, groups, freeform paths. |
| `notes-and-links.pptx` | 4 slides: multi-paragraph speaker notes with umlauts, external hyperlinks on text runs, and an internal slide-to-slide jump (`click_action.target_slide`). | Notes extraction, hyperlink handling, internal-link edge cases in composition. |
| `classic-4x3.pptx` | 3 slides at **4:3** (9 144 000 × 6 858 000 EMU): title, bullets, image. | Mixed-slide-size composition; non-widescreen layout. |
| `unicode-i18n.pptx` | 3 slides: German umlauts/ß, CJK, Cyrillic, Greek, emoji, RTL Arabic & Hebrew, and a diacritics line; plus a title packed with scripts. | Unicode handling, diacritics-insensitive search, i18n indexing. |
| `big-deck.pptx` | 40 slides cycling through layouts, each with a title + bulleted body; a shared logo on every 5th slide. | Scanning throughput, virtualized grid, dedup across many slides. |
| `edge-cases.pptx` | 5 slides: a completely empty slide, an image-only slide (no text), a 300+ char title, a 4-level nested group, and a shrink-to-fit autofit text box. | Empty/degenerate slides, deep group nesting, very long text, autofit. |

## Downloaded real-world decks (`real/`)

Best-effort downloads from public test corpora (Apache POI, python-pptx). These
are third-party files; re-run `scripts/download_real_examples.py` to refetch.

| File | What's inside | What it stress-tests | Source |
|------|---------------|----------------------|--------|
| `artistic-effects.pptx` | 2 slides, artistic image effects on photos (~950 KB); 16:9-ish `9144000 × 5143500` EMU. | Real image-effect XML, unusual slide size, larger media. | [Apache POI · `ArtisticEffectSample.pptx`](https://raw.githubusercontent.com/apache/poi/trunk/test-data/slideshow/ArtisticEffectSample.pptx) |
| `chart-picture-bg.pptx` | 1 slide, charts over picture-fill slide backgrounds (~640 KB); 16:9. | Chart + picture-background parsing; an untitled slide (title fallback). | [Apache POI · `chart-picture-bg.pptx`](https://raw.githubusercontent.com/apache/poi/trunk/test-data/slideshow/chart-picture-bg.pptx) |
| `academic-talk-how-we-refactor.pptx` | Real academic conference talk: 28 slides, many images, 12 slides with speaker notes (~2 MB). | End-to-end parse throughput (~340 ms), notes extraction, a genuinely messy real deck. | [Apache POI · `ca.ubc.cs.people_~emhill_presentations_HowWeRefactor.pptx`](https://raw.githubusercontent.com/apache/poi/trunk/test-data/slideshow/ca.ubc.cs.people_~emhill_presentations_HowWeRefactor.pptx) |
| `no-core-props.pptx` | 1 slide, a deck with **no** core properties (no docProps title). | The filename-fallback path when a deck has no title at all. | [python-pptx · `no-core-props.pptx`](https://raw.githubusercontent.com/scanny/python-pptx/master/tests/test_files/no-core-props.pptx) |

## Validating the corpus

```sh
# Every deck parses in the Rust core (every line must print "OK"):
cargo run -q --example corpus_check -- examples/pptx
cargo run -q --example corpus_check -- examples/pptx/real

# Compose a few slides from a deck and inspect the result:
cargo run -q --example compose_demo -- /tmp/out.pptx \
    examples/pptx/charts.pptx:2 examples/pptx/tables.pptx:4 examples/pptx/images.pptx:3
```
