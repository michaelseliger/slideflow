# Bundled substitute fonts

Metric-compatible, freely redistributable stand-ins for the two most common
unembedded Microsoft Office fonts. When a deck names one of these fonts but does
**not** embed it, Slideflow substitutes the bundled clone so previews and
PNG/PDF exports render with the intended metrics/shapes instead of falling
through to Helvetica.

| Bundled family | Substitutes for | Metrics |
|----------------|-----------------|---------|
| **Carlito**    | Calibri         | Identical advance widths (metric-compatible) |
| **Caladea**    | Cambria         | Identical advance widths (metric-compatible) |

We deliberately do **not** bundle Arial/Times/Georgia clones — macOS ships Arial,
Times New Roman and Georgia, and the renderer covers Segoe UI, Consolas,
Constantia, Candara, Corbel, Aptos, … with richer named CSS fallback chains
(`crate::fonts::fallback_families`) rather than more bundled bytes.

## Provenance

All files are copied verbatim from the [`google/fonts`](https://github.com/google/fonts)
repository, pinned to commit
[`e4572de925a4c3be12f1f9983ee0adbe1eb6e9fe`](https://github.com/google/fonts/tree/e4572de925a4c3be12f1f9983ee0adbe1eb6e9fe).

Source URLs (raw, at that commit):

- `https://raw.githubusercontent.com/google/fonts/e4572de925a4c3be12f1f9983ee0adbe1eb6e9fe/ofl/carlito/Carlito-Regular.ttf`
- `https://raw.githubusercontent.com/google/fonts/e4572de925a4c3be12f1f9983ee0adbe1eb6e9fe/ofl/carlito/Carlito-Bold.ttf`
- `https://raw.githubusercontent.com/google/fonts/e4572de925a4c3be12f1f9983ee0adbe1eb6e9fe/ofl/carlito/Carlito-Italic.ttf`
- `https://raw.githubusercontent.com/google/fonts/e4572de925a4c3be12f1f9983ee0adbe1eb6e9fe/ofl/carlito/Carlito-BoldItalic.ttf`
- `https://raw.githubusercontent.com/google/fonts/e4572de925a4c3be12f1f9983ee0adbe1eb6e9fe/ofl/caladea/Caladea-Regular.ttf`
- `https://raw.githubusercontent.com/google/fonts/e4572de925a4c3be12f1f9983ee0adbe1eb6e9fe/ofl/caladea/Caladea-Bold.ttf`
- `https://raw.githubusercontent.com/google/fonts/e4572de925a4c3be12f1f9983ee0adbe1eb6e9fe/ofl/caladea/Caladea-Italic.ttf`
- `https://raw.githubusercontent.com/google/fonts/e4572de925a4c3be12f1f9983ee0adbe1eb6e9fe/ofl/caladea/Caladea-BoldItalic.ttf`

Upstream projects:
[Carlito](https://github.com/googlefonts/carlito) ·
[Caladea](https://github.com/huertatipografica/Caladea)

## License

Both families are licensed under the **SIL Open Font License 1.1**. The verbatim
license for each is kept alongside its font files:

- `carlito/OFL.txt`
- `caladea/OFL.txt`

The OFL permits bundling and redistribution (including in a commercial app) as
long as these license files travel with the fonts and neither family is sold on
its own. Keep the `OFL.txt` files next to the `.ttf`s.
