"""External oracle checks for a font exported by FORMA.

Milestone 1, acceptance criteria 2 and 8 (docs/04-roadmap.md): independent
industry tools confirm that the TrueType font an agent built through
`forma mcp` is a real, valid font that matches the model.

Usage (an isolated interpreter that has tools/oracle/requirements.txt):

    python -I verify_font.py --ttf X.ttf --expected expected.json
        [--woff2 X.woff2] [--ufo X.ufo --fontmake-parity [--work-dir DIR]]

Checks:

1. OTS: `ots-sanitize` (opentype-sanitizer) accepts the TTF, and the WOFF2
   if one is given.
2. HarfBuzz (uharfbuzz): shaping `shaping.text` gives exactly the glyph names
   and x advances of `expected.json`.
3. fontTools: the family name (name ID 1), the units per em and the cmap
   (character -> glyph name) equal `expected.json`.
4. With --fontmake-parity: fontmake compiles the UFO, and both fonts have the
   same glyph set, the same cmap and, glyph by glyph, the same advance width,
   the same number of contours, a bounding box within +/-1 unit and an
   absolute outline area within 0.5 %. fontc and fontmake may convert cubic
   curves to quadratic ones differently, so points are not compared one by
   one. Every other difference between the two fonts is listed, not failed;
   point differences are explained (implied on-curve points, contour order,
   start points) with the largest point deviation.

Prints a short human summary; the last line is a JSON summary. Exits 0 only if
every check passes, 1 if one fails, 2 on a usage error or a missing package.
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import tempfile
from importlib import metadata
from pathlib import Path

try:
    import ots
    import uharfbuzz as hb
    from fontTools.pens.areaPen import AreaPen
    from fontTools.pens.boundsPen import BoundsPen
    from fontTools.pens.recordingPen import RecordingPen
    from fontTools.ttLib import TTFont
except ImportError as missing:
    print(
        f"verify_font: a required package is missing ({missing}); "
        "install tools/oracle/requirements.txt",
        file=sys.stderr,
    )
    sys.exit(2)

PACKAGES = ("fonttools", "fontmake", "opentype-sanitizer", "uharfbuzz")

# fontmake parity tolerances (Task 20, ruling 3).
BBOX_TOLERANCE = 1.0  # font units, per bounding box coordinate
AREA_TOLERANCE = 0.005  # relative difference of the absolute outline area

# Tables the parity checks compare glyph by glyph (glyf point differences are
# also explained per glyph), so they are not compared as bytes.
OUTLINE_TABLES = {"glyf", "loca", "hmtx", "cmap"}


class Check:
    """The outcome of one check: what failed (if anything) and what was seen."""

    def __init__(self, name: str) -> None:
        self.name = name
        self.problems: list[str] = []
        self.details: dict = {}

    @property
    def ok(self) -> bool:
        return not self.problems

    def fail(self, problem: str) -> None:
        self.problems.append(problem)

    def summary(self) -> dict:
        return {"ok": self.ok, "problems": self.problems, **self.details}


def versions() -> dict:
    found = {}
    for package in PACKAGES:
        try:
            found[package] = metadata.version(package)
        except metadata.PackageNotFoundError:
            found[package] = None
    found["harfbuzz"] = hb.version_string()
    found["python"] = sys.version.split()[0]
    return found


# 1. OTS ---------------------------------------------------------------------


def check_ots(check: Check, path: Path) -> None:
    result = ots.sanitize(str(path), capture_output=True)
    output = (result.stdout + result.stderr).decode("utf-8", "replace").strip()
    check.details.setdefault("files", {})[path.name] = {
        "exit_code": result.returncode,
        "output": output,
    }
    if result.returncode != 0:
        check.fail(f"ots-sanitize rejected {path.name} (exit {result.returncode}): {output}")


# 2. HarfBuzz ----------------------------------------------------------------


def check_shaping(check: Check, ttf: Path, font: TTFont, expected: dict) -> None:
    shaping = expected["shaping"]
    text = shaping["text"]
    face = hb.Face(hb.Blob.from_file_path(str(ttf)))
    hb_font = hb.Font(face)  # scale = units per em: advances in font units
    buffer = hb.Buffer()
    buffer.add_str(text)
    buffer.guess_segment_properties()
    hb.shape(hb_font, buffer, {})
    order = font.getGlyphOrder()
    glyphs = [
        order[info.codepoint] if info.codepoint < len(order) else f"gid{info.codepoint}"
        for info in buffer.glyph_infos
    ]
    advances = [position.x_advance for position in buffer.glyph_positions]
    check.details.update(text=text, glyphs=glyphs, advances=advances)
    if glyphs != shaping["glyphs"]:
        check.fail(f"glyphs: got {glyphs}, expected {shaping['glyphs']}")
    if advances != shaping["advances"]:
        check.fail(f"advances: got {advances}, expected {shaping['advances']}")


# 3. fontTools ---------------------------------------------------------------


def cmap_by_char(font: TTFont) -> dict[str, str]:
    return {chr(code): name for code, name in (font.getBestCmap() or {}).items()}


def mapping_diff(a: dict, b: dict, labels: tuple[str, str] = ("got", "expected")) -> list[str]:
    lines = []
    for key in sorted(set(a) | set(b)):
        if a.get(key) != b.get(key):
            lines.append(f"{key!r}: {labels[0]} {a.get(key)!r}, {labels[1]} {b.get(key)!r}")
    return lines


def check_fonttools(check: Check, font: TTFont, expected: dict) -> None:
    families = sorted({record.toUnicode() for record in font["name"].names if record.nameID == 1})
    upm = font["head"].unitsPerEm
    cmap = cmap_by_char(font)
    check.details.update(family=families, upm=upm, cmap_entries=len(cmap))
    if families != [expected["family"]]:
        check.fail(f"name ID 1: got {families}, expected [{expected['family']!r}]")
    if upm != expected["upm"]:
        check.fail(f"unitsPerEm: got {upm}, expected {expected['upm']}")
    for line in mapping_diff(cmap, expected["cmap"]):
        check.fail(f"cmap {line}")


# 4. fontmake parity -----------------------------------------------------------


def compile_with_fontmake(check: Check, ufo: Path, work_dir: Path) -> Path | None:
    out = work_dir / "fontmake.ttf"
    # A TTF left by an earlier run (--work-dir is reused) must not pass for the
    # output of this one if fontmake fails to write a new file.
    out.unlink(missing_ok=True)
    # The same head timestamps as fontc gets from scripts/acceptance.*.
    env = dict(os.environ, SOURCE_DATE_EPOCH="0")
    command = [sys.executable, "-I", "-m", "fontmake", "-u", str(ufo), "-o", "ttf"]
    command += ["--output-path", str(out)]
    # The output is only shown in a failure message: decode it as UTF-8 and
    # never fail on bytes that are not (otherwise `text=True` uses the locale's
    # code page, and on Windows a message with a non-ASCII path can crash).
    result = subprocess.run(
        command,
        cwd=work_dir,
        env=env,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        check=False,
    )
    if result.returncode != 0 or not out.is_file():
        check.fail(f"fontmake failed (exit {result.returncode}):\n{result.stdout}{result.stderr}")
        return None
    check.details["fontmake_ttf"] = str(out)
    return out


def outline_stats(font: TTFont) -> dict[str, dict]:
    glyph_set = font.getGlyphSet()
    stats = {}
    for name in font.getGlyphOrder():
        glyph = glyph_set[name]
        recording = RecordingPen()
        glyph.draw(recording)
        bounds = BoundsPen(glyph_set)
        glyph.draw(bounds)
        area = AreaPen(glyph_set)
        glyph.draw(area)
        points = glyf_points(font, name)
        stats[name] = {
            "advance": font["hmtx"][name][0],
            "contours": sum(1 for op, _ in recording.value if op in ("closePath", "endPath")),
            "glyf": points,
            "points": None if points is None else sum(len(contour) for contour in points),
            "bounds": bounds.bounds,
            "area": area.value,
        }
    return stats


def glyf_points(font: TTFont, name: str) -> list[list[tuple]] | None:
    """The glyph's TrueType contours as (x, y, on-curve) points."""
    if "glyf" not in font:
        return None
    glyf = font["glyf"]
    coordinates, ends, flags = glyf[name].getCoordinates(glyf)
    contours, start = [], 0
    for end in ends:
        contours.append(
            [
                (float(coordinates[i][0]), float(coordinates[i][1]), bool(flags[i] & 1))
                for i in range(start, end + 1)
            ]
        )
        start = end + 1
    return contours


def explicit(contour: list[tuple]) -> list[tuple]:
    """`contour` with every implied on-curve point (the midpoint of two
    consecutive off-curve points) written out."""
    points = []
    for index, (x, y, on) in enumerate(contour):
        points.append((x, y, on))
        next_x, next_y, next_on = contour[(index + 1) % len(contour)]
        if not on and not next_on:
            points.append(((x + next_x) / 2, (y + next_y) / 2, True))
    return points


def contour_deviation(a: list[tuple], b: list[tuple]) -> float | None:
    """The largest point distance between two contours, at the start point
    that makes it smallest; None if no start point lines up their on- and
    off-curve points."""
    if len(a) != len(b):
        return None
    best = None
    for shift in range(len(a)):
        rotated = a[shift:] + a[:shift]
        if any(p[2] != q[2] for p, q in zip(rotated, b)):
            continue
        deviation = max(
            (max(abs(p[0] - q[0]), abs(p[1] - q[1])) for p, q in zip(rotated, b)), default=0.0
        )
        best = deviation if best is None else min(best, deviation)
    return best


def point_deviation(a: list[list[tuple]], b: list[list[tuple]]) -> float | None:
    """How far apart the points of two glyphs are once implied on-curve
    points are explicit, whatever the contour order and start points; None if
    their contours do not pair up."""
    left, right = [explicit(c) for c in a], [explicit(c) for c in b]
    if len(left) != len(right):
        return None
    worst = 0.0
    for contour in left:
        pairs = [(contour_deviation(contour, other), i) for i, other in enumerate(right)]
        pairs = [(deviation, i) for deviation, i in pairs if deviation is not None]
        if not pairs:
            return None
        deviation, index = min(pairs)
        worst = max(worst, deviation)
        right.pop(index)
    return worst


def bbox_delta(a, b) -> float | None:
    """The largest coordinate difference of two bounding boxes; None if only
    one of the glyphs is empty."""
    if a is None and b is None:
        return 0.0
    if a is None or b is None:
        return None
    return float(max(abs(x - y) for x, y in zip(a, b)))


def area_delta(a: float, b: float) -> float:
    """The relative difference of two absolute outline areas."""
    a, b = abs(a), abs(b)
    largest = max(a, b)
    return 0.0 if largest == 0 else abs(a - b) / largest


def table_fields(table) -> dict:
    fields = {}
    for key, value in sorted(vars(table).items()):
        if key.startswith("_") or key == "tableTag" or callable(value):
            continue
        if hasattr(value, "__dict__"):  # e.g. OS/2 panose
            for sub, subvalue in sorted(vars(value).items()):
                fields[f"{key}.{sub}"] = subvalue
        elif isinstance(value, (int, float, str, bytes, bool, type(None))):
            fields[key] = value
    return fields


def name_records(font: TTFont) -> dict:
    return {
        f"{r.nameID} ({r.platformID}/{r.platEncID}/{r.langID:#x})": r.toUnicode()
        for r in font["name"].names
    }


def other_differences(forma: TTFont, fontmake: TTFont, stats_a: dict, stats_b: dict) -> list[str]:
    """Differences outside the parity definition: listed, never failed."""
    lines = []
    tags_a, tags_b = set(forma.keys()) - {"GlyphOrder"}, set(fontmake.keys()) - {"GlyphOrder"}
    for tag in sorted(tags_a - tags_b):
        lines.append(f"table {tag}: only in FORMA")
    for tag in sorted(tags_b - tags_a):
        lines.append(f"table {tag}: only in fontmake")
    if forma.getGlyphOrder() != fontmake.getGlyphOrder():
        lines.append(f"glyph order: {forma.getGlyphOrder()} vs {fontmake.getGlyphOrder()}")
    for tag in sorted(tags_a & tags_b):
        if tag in OUTLINE_TABLES:
            continue
        if tag == "name":
            for key, (a, b) in diff_dicts(name_records(forma), name_records(fontmake)).items():
                lines.append(f"name {key}: {a!r} vs {b!r}")
        elif tag in ("head", "hhea", "maxp", "OS/2", "post"):
            fields = diff_dicts(table_fields(forma[tag]), table_fields(fontmake[tag]))
            for key, (a, b) in fields.items():
                lines.append(f"{tag}.{key}: {a!r} vs {b!r}")
        elif forma.getTableData(tag) != fontmake.getTableData(tag):
            lines.append(
                f"table {tag}: different bytes ({len(forma.getTableData(tag))} vs "
                f"{len(fontmake.getTableData(tag))})"
            )
    for name in [name for name in forma.getGlyphOrder() if name in stats_b]:
        a, b = stats_a[name], stats_b[name]
        if a["glyf"] == b["glyf"]:
            continue
        line = f"glyf {name}: {a['points']} vs {b['points']} points; "
        deviation = point_deviation(a["glyf"], b["glyf"]) if a["glyf"] and b["glyf"] else None
        if deviation is None:
            line += "the outlines differ point by point"
        else:
            line += (
                "the same points once implied on-curve points are explicit, "
                f"contour order and start points aside (max deviation {deviation:g} units)"
            )
        lines.append(line)
    return lines


def diff_dicts(a: dict, b: dict) -> dict:
    missing = "<absent>"
    return {
        key: (a.get(key, missing), b.get(key, missing))
        for key in sorted(set(a) | set(b))
        if a.get(key, missing) != b.get(key, missing)
    }


def check_parity(check: Check, forma: TTFont, ufo: Path, work_dir: Path) -> None:
    fontmake_path = compile_with_fontmake(check, ufo, work_dir)
    if fontmake_path is None:
        return
    fontmake = TTFont(fontmake_path)
    stats_a, stats_b = outline_stats(forma), outline_stats(fontmake)

    glyphs_a, glyphs_b = set(stats_a), set(stats_b)
    for name in sorted(glyphs_a - glyphs_b):
        check.fail(f"glyph {name}: only in FORMA")
    for name in sorted(glyphs_b - glyphs_a):
        check.fail(f"glyph {name}: only in fontmake")
    for line in mapping_diff(cmap_by_char(forma), cmap_by_char(fontmake), ("FORMA", "fontmake")):
        check.fail(f"cmap {line}")

    rows = []
    max_bbox, max_area = 0.0, 0.0
    for name in forma.getGlyphOrder():
        if name not in stats_b:
            continue
        a, b = stats_a[name], stats_b[name]
        if a["advance"] != b["advance"]:
            check.fail(f"{name}: advance {a['advance']} vs {b['advance']}")
        if a["contours"] != b["contours"]:
            check.fail(f"{name}: {a['contours']} vs {b['contours']} contours")
        bbox = bbox_delta(a["bounds"], b["bounds"])
        if bbox is None or bbox > BBOX_TOLERANCE:
            check.fail(f"{name}: bounding box {a['bounds']} vs {b['bounds']}")
        area = area_delta(a["area"], b["area"])
        if area > AREA_TOLERANCE:
            check.fail(f"{name}: area {abs(a['area']):.1f} vs {abs(b['area']):.1f} ({area:.3%})")
        if bbox is not None:
            max_bbox = max(max_bbox, bbox)
        max_area = max(max_area, area)
        rows.append(
            {
                "glyph": name,
                "advance": [a["advance"], b["advance"]],
                "contours": [a["contours"], b["contours"]],
                "points": [a["points"], b["points"]],
                "bbox_delta": bbox,
                "area": [round(abs(a["area"]), 1), round(abs(b["area"]), 1)],
                "area_delta_percent": round(area * 100, 4),
            }
        )

    others = other_differences(forma, fontmake, stats_a, stats_b)
    check.details.update(
        glyphs=len(rows),
        max_bbox_delta=max_bbox,
        max_area_delta_percent=round(max_area * 100, 4),
        measurements=rows,
        other_differences=others,
    )


# Report ------------------------------------------------------------------------


def print_parity_table(details: dict) -> None:
    print(
        f"    {'glyph':<10}{'advance':>9}{'contours':>10}{'points F/fm':>13}"
        f"{'bbox d':>8}{'area FORMA':>12}{'area fontmake':>15}{'area d %':>10}"
    )
    for row in details.get("measurements", []):
        points = f"{row['points'][0]}/{row['points'][1]}"
        bbox = "n/a" if row["bbox_delta"] is None else f"{row['bbox_delta']:.2f}"
        print(
            f"    {row['glyph']:<10}{row['advance'][0]:>9}{row['contours'][0]:>10}{points:>13}"
            f"{bbox:>8}{row['area'][0]:>12.1f}{row['area'][1]:>15.1f}"
            f"{row['area_delta_percent']:>10.4f}"
        )
    others = details.get("other_differences", [])
    print(f"    other differences (not part of parity): {len(others)}")
    for line in others:
        print(f"      - {line}")


def load_expected(parser: argparse.ArgumentParser, path: Path) -> dict:
    try:
        expected = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError) as error:
        parser.error(f"cannot read {path}: {error}")
    shape = {"family": str, "upm": int, "cmap": dict, "shaping": dict}
    shaping_shape = {"text": str, "glyphs": list, "advances": list}
    if not isinstance(expected, dict) or any(
        not isinstance(expected.get(key), kind) for key, kind in shape.items()
    ):
        parser.error(f"{path}: expected {{family, upm, cmap, shaping}}")
    if any(not isinstance(expected["shaping"].get(key), kind) for key, kind in shaping_shape.items()):
        parser.error(f"{path}: expected shaping = {{text, glyphs, advances}}")
    return expected


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--ttf", type=Path, required=True, help="the TrueType font to check")
    parser.add_argument("--expected", type=Path, required=True, help="expected.json")
    parser.add_argument("--woff2", type=Path, help="a WOFF2 of the same font, for OTS")
    parser.add_argument("--ufo", type=Path, help="the UFO the font was compiled from")
    parser.add_argument(
        "--fontmake-parity", action="store_true", help="compile the UFO with fontmake and compare"
    )
    parser.add_argument(
        "--work-dir", type=Path, help="where fontmake writes its TTF (default: a temporary folder)"
    )
    args = parser.parse_args(argv)
    if args.fontmake_parity and args.ufo is None:
        parser.error("--fontmake-parity needs --ufo")
    for path in (args.ttf, args.expected, args.woff2):
        if path is not None and not path.is_file():
            parser.error(f"no such file: {path}")
    if args.ufo is not None and not args.ufo.is_dir():
        parser.error(f"no such UFO folder: {args.ufo}")
    args.expected_data = load_expected(parser, args.expected)
    return args


def run(check: Check, function, *arguments) -> Check:
    """Runs one check; an exception (an unreadable font, an oracle error) is
    a failure of that check, so the other checks and the summary still run."""
    try:
        function(check, *arguments)
    except Exception as error:  # noqa: BLE001 - reported as a failure
        check.fail(f"{type(error).__name__}: {error}")
    return check


def check_parity_in(check: Check, font: TTFont, ufo: Path, work_dir: Path | None) -> None:
    if work_dir is not None:
        work_dir.mkdir(parents=True, exist_ok=True)
        check_parity(check, font, ufo.resolve(), work_dir.resolve())
    else:
        with tempfile.TemporaryDirectory(prefix="forma-fontmake-") as temporary:
            check_parity(check, font, ufo.resolve(), Path(temporary))
        check.details.pop("fontmake_ttf", None)  # removed with the folder


def print_report(checks: list[Check], found: dict) -> None:
    print("oracles: " + ", ".join(f"{name} {version}" for name, version in found.items()))
    for check in checks:
        print(f"{check.name:<16} {'PASS' if check.ok else 'FAIL'}")
        for problem in check.problems:
            print(f"    {problem}")
    by_name = {check.name: check.details for check in checks}
    for name, result in by_name.get("OTS", {}).get("files", {}).items():
        print(f"    OTS {name}: {result['output']}")
    shaping = by_name.get("HarfBuzz", {})
    if "glyphs" in shaping:
        print(f"    shaped {shaping['text']!r}: {shaping['glyphs']}")
        print(f"    advances: {shaping['advances']}")
    fonttools = by_name.get("fontTools", {})
    if "upm" in fonttools:
        print(
            f"    family {fonttools['family']}, upm {fonttools['upm']}, "
            f"{fonttools['cmap_entries']} cmap entries"
        )
    parity = by_name.get("fontmake parity", {})
    if "measurements" in parity:
        print(
            f"    parity: {parity['glyphs']} glyphs, max bbox delta {parity['max_bbox_delta']}, "
            f"max area delta {parity['max_area_delta_percent']} %"
        )
        print_parity_table(parity)


def main(argv: list[str]) -> int:
    # A console or pipe in a legacy code page (Windows) must not turn a
    # non-ASCII name into a crash; the JSON summary line is ASCII anyway.
    sys.stdout.reconfigure(errors="backslashreplace")
    args = parse_args(argv)
    expected = args.expected_data

    checks = [run(Check("OTS"), check_ots, args.ttf)]
    if args.woff2 is not None:
        run(checks[0], check_ots, args.woff2)

    try:
        font = TTFont(args.ttf)
        font.getGlyphOrder()
    except Exception as error:  # noqa: BLE001 - reported as a failure
        unreadable = f"fontTools cannot read {args.ttf.name}: {type(error).__name__}: {error}"
        font = None
    names = ["HarfBuzz", "fontTools"] + (["fontmake parity"] if args.fontmake_parity else [])
    if font is None:
        for name in names:
            check = Check(name)
            check.fail(unreadable)
            checks.append(check)
    else:
        checks.append(run(Check("HarfBuzz"), check_shaping, args.ttf, font, expected))
        checks.append(run(Check("fontTools"), check_fonttools, font, expected))
        if args.fontmake_parity:
            parity = Check("fontmake parity")
            checks.append(run(parity, check_parity_in, font, args.ufo, args.work_dir))

    found = versions()
    print_report(checks, found)
    ok = all(check.ok for check in checks)
    print("VERIFY OK" if ok else "VERIFY FAILED")
    summary = {
        "ok": ok,
        "ttf": str(args.ttf),
        "versions": found,
        "checks": {check.name: check.summary() for check in checks},
    }
    print(json.dumps(summary, sort_keys=True))
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
