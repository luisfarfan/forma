"""External oracle checks for the fonts of mode C (Milestone 2.5, Task 19).

Usage (an isolated interpreter that has tools/oracle/requirements.txt):

    python -I verify_hito2_5.py <mode-c output folder>...

Each folder is the `--out` of `forma-bench run --mode c --level <level>`:
one sub-folder per case with `results.json`, `font.ttf` and `font.woff2`.
For every case whose pipeline exported fonts:

1. OTS (`ots-sanitize`, opentype-sanitizer) accepts the TTF and the WOFF2.
2. fontTools: every character of the layout is in the cmap.
3. HarfBuzz (uharfbuzz): shaping the layout's characters gives no
   `.notdef` and a positive advance for each.
4. fontspector (if on the PATH): the number of FAIL and WARN results of
   its `opentype` profile, as in scripts/acceptance.* (reported here; the
   acceptance scripts fail on a FAIL).

Prints one line per case and a JSON summary as the last line. Exits 0 only
if every OTS, cmap and shaping check passes, 1 otherwise, 2 on a usage
error or a missing package.
"""

from __future__ import annotations

import json
import shutil
import subprocess
import sys
from pathlib import Path

try:
    import ots
    import uharfbuzz as hb
    from fontTools.ttLib import TTFont
except ImportError as missing:
    print(f"verify_hito2_5: a required package is missing ({missing})", file=sys.stderr)
    sys.exit(2)


def ots_ok(path: Path) -> bool:
    return ots.sanitize(str(path), capture_output=True).returncode == 0


def shaping_ok(ttf: Path, text: str) -> tuple[bool, str]:
    blob = hb.Blob.from_file_path(str(ttf))
    font = hb.Font(hb.Face(blob))
    buf = hb.Buffer()
    buf.add_str(text)
    buf.guess_segment_properties()
    hb.shape(font, buf, {})
    bad = [
        i
        for i, (info, pos) in enumerate(zip(buf.glyph_infos, buf.glyph_positions))
        if info.codepoint == 0 or pos.x_advance <= 0
    ]
    return (not bad, f"{len(bad)} bad of {len(buf.glyph_infos)}")


def fontspector_counts(ttf: Path) -> dict | None:
    exe = shutil.which("fontspector")
    if not exe:
        return None
    out = subprocess.run(
        [exe, "--profile", "opentype", "--json", "-", str(ttf)],
        capture_output=True,
        text=True,
        check=False,
    )
    try:
        data = json.loads(out.stdout)
    except json.JSONDecodeError:
        return {"error": out.stderr.strip()[:200]}
    counts: dict[str, int] = {}
    text = json.dumps(data)
    for status in ("FAIL", "WARN"):
        counts[status.lower()] = text.count(f'"{status}"')
    return counts


def check_case(case_dir: Path) -> dict | None:
    results = case_dir / "results.json"
    ttf, woff2 = case_dir / "font.ttf", case_dir / "font.woff2"
    if not (results.exists() and ttf.exists() and woff2.exists()):
        return None
    layout = json.loads(results.read_text(encoding="utf-8")).get("layout", [])
    chars = "".join(layout)
    font = TTFont(str(ttf))
    cmap = font.getBestCmap() or {}
    missing = [c for c in chars if ord(c) not in cmap]
    shaped, detail = shaping_ok(ttf, chars) if chars else (True, "no text")
    return {
        "case": case_dir.name,
        "ots_ttf": ots_ok(ttf),
        "ots_woff2": ots_ok(woff2),
        "cmap_missing": missing,
        "shaping_ok": shaped,
        "shaping": detail,
        "fontspector": fontspector_counts(ttf),
    }


def main(argv: list[str]) -> int:
    if not argv:
        print(__doc__, file=sys.stderr)
        return 2
    cases = []
    for folder in argv:
        for case_dir in sorted(Path(folder).iterdir()):
            if case_dir.is_dir():
                checked = check_case(case_dir)
                if checked:
                    cases.append(checked)
                    print(
                        f"{checked['case']}: OTS {checked['ots_ttf']}/{checked['ots_woff2']}, "
                        f"cmap missing {len(checked['cmap_missing'])}, "
                        f"shaping {checked['shaping']}, fontspector {checked['fontspector']}"
                    )
    ok = all(
        c["ots_ttf"] and c["ots_woff2"] and not c["cmap_missing"] and c["shaping_ok"]
        for c in cases
    )
    print(json.dumps({"cases": len(cases), "ok": ok}))
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
