#!/usr/bin/env bash
#
# Visual fidelity harness (dev-only, never CI): renders every slide of a deck
# two ways and lays them out side by side in one HTML page —
#
#   LEFT  = LibreOffice's rendering (soffice -> PDF -> pdftoppm PNG), the oracle
#   RIGHT = slideflow's own SVG renderer (the `render_demo` example)
#
# so you can eyeball how close our previews are after a renderer change. Run it
# against the real corpus (~/Desktop/Folienpool) after each fidelity phase.
#
# LibreOffice is used ONLY here as an offline comparison oracle — never at
# runtime. Font rendering differs across machines, so this is a human judgment
# aid, not a pass/fail gate.
#
# Usage:
#   scripts/render_contact_sheet.sh <deck.pptx> [out_dir] [tier]
#     tier: thumb | preview | full   (default: preview)
#
# Then open  <out_dir>/contact_sheet.html  in a browser.

set -euo pipefail

DECK="${1:?usage: render_contact_sheet.sh <deck.pptx> [out_dir] [tier]}"
OUT="${2:-$(mktemp -d)}"
TIER="${3:-preview}"

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SOFFICE="$(command -v soffice || echo /Applications/LibreOffice.app/Contents/MacOS/soffice)"

mkdir -p "$OUT"
BASE="$(basename "$DECK")"
BASE="${BASE%.*}"

echo "==> LibreOffice: $DECK -> PDF"
"$SOFFICE" --headless --convert-to pdf --outdir "$OUT" "$DECK" >/dev/null 2>&1

echo "==> PDF -> per-slide PNGs"
pdftoppm -png -r 110 "$OUT/$BASE.pdf" "$OUT/src" >/dev/null 2>&1

echo "==> Building render_demo (release)"
cargo build --release -q -p slideflow-core --example render_demo --manifest-path "$REPO_ROOT/Cargo.toml"
DEMO="$REPO_ROOT/target/release/examples/render_demo"

HTML="$OUT/contact_sheet.html"
{
  echo "<!doctype html><meta charset=utf-8><title>Contact sheet — $BASE</title>"
  echo "<style>body{font:13px system-ui;margin:24px;background:#111;color:#ddd}"
  echo "h1{font-size:15px}.row{display:grid;grid-template-columns:1fr 1fr;gap:12px;margin:18px 0;align-items:start}"
  echo ".cell img{width:100%;border:1px solid #333;background:#fff}.lbl{font-size:11px;color:#888;margin:2px 0}"
  echo ".hdr{position:sticky;top:0;background:#111;padding:6px 0}</style>"
  echo "<h1>$BASE — <span style=color:#888>left: LibreOffice · right: slideflow ($TIER)</span></h1>"
  echo "<div class=row><div class=hdr>LibreOffice (oracle)</div><div class=hdr>slideflow renderer</div></div>"
} > "$HTML"

i=0
for src in $(ls "$OUT"/src-*.png | sort -V); do
  i=$((i + 1))
  ours="ours-$i.svg"
  if "$DEMO" "$DECK" "$i" "$OUT/$ours" "$TIER" >/dev/null 2>&1; then
    ourcell="<img src=\"$ours\">"
  else
    ourcell="<div class=lbl>render failed</div>"
  fi
  {
    echo "<div class=row>"
    echo "  <div class=cell><div class=lbl>slide $i</div><img src=\"$(basename "$src")\"></div>"
    echo "  <div class=cell><div class=lbl>slide $i</div>$ourcell</div>"
    echo "</div>"
  } >> "$HTML"
done

echo "==> $i slides"
echo "==> open $HTML"
