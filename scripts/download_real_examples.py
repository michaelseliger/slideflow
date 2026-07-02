#!/usr/bin/env python3
"""Best-effort download of a few complex real-world .pptx decks into
examples/pptx/real/.

These are third-party presentations used as realistic parse/render/compose
stress tests. Failures are skipped silently; each file is verified to be a real
zip (PK header) that python-pptx can open, and invalid downloads are deleted.

Usage:
    python3 scripts/download_real_examples.py

Sources (all publicly hosted test corpora):
  * Apache POI test-data/slideshow  (Apache-2.0)
  * python-pptx tests/test_files    (MIT)
"""
from __future__ import annotations

import socket
import urllib.request
from pathlib import Path

from pptx import Presentation

REAL = Path(__file__).resolve().parent.parent / "examples" / "pptx" / "real"
socket.setdefaulttimeout(45)

# (target name, source raw URL, description)
WANTED = [
    ("artistic-effects.pptx",
     "https://raw.githubusercontent.com/apache/poi/trunk/test-data/slideshow/ArtisticEffectSample.pptx",
     "Artistic image effects applied to photos."),
    ("chart-picture-bg.pptx",
     "https://raw.githubusercontent.com/apache/poi/trunk/test-data/slideshow/chart-picture-bg.pptx",
     "Charts plus picture-fill slide backgrounds; 16:9 size."),
    ("academic-talk-how-we-refactor.pptx",
     "https://raw.githubusercontent.com/apache/poi/trunk/test-data/slideshow/ca.ubc.cs.people_~emhill_presentations_HowWeRefactor.pptx",
     "Real academic conference talk: 28 slides, many images, speaker notes."),
    ("no-core-props.pptx",
     "https://raw.githubusercontent.com/scanny/python-pptx/master/tests/test_files/no-core-props.pptx",
     "Deck with NO core properties (no docProps title) — filename-fallback test."),
]


def try_fetch(url: str) -> bytes | None:
    # apache/poi's default branch is 'trunk'; fall back to 'master' just in case.
    for u in (url, url.replace("/trunk/", "/master/")):
        try:
            req = urllib.request.Request(u, headers={"User-Agent": "slideflow-corpus/1.0"})
            with urllib.request.urlopen(req) as r:
                if r.status == 200:
                    return r.read()
        except Exception:
            continue
    return None


def main() -> int:
    REAL.mkdir(parents=True, exist_ok=True)
    ok = 0
    for name, url, _desc in WANTED:
        dest = REAL / name
        data = try_fetch(url)
        if not data or data[:2] != b"PK":
            print(f"SKIP  {name} (fetch failed or not a zip)")
            continue
        dest.write_bytes(data)
        try:
            n = len(list(Presentation(str(dest)).slides))
        except Exception as e:  # noqa: BLE001 - best effort
            dest.unlink(missing_ok=True)
            print(f"DROP  {name} (python-pptx cannot open): {e}")
            continue
        print(f"OK    {name:38} {len(data) // 1024:>5} KB  slides={n}")
        ok += 1
    print(f"\n{ok}/{len(WANTED)} downloaded and validated into {REAL}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
