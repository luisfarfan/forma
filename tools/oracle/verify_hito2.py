"""External oracle checks for the fonts of Milestone 2 (docs/04-roadmap.md,
Hito 2, criteria 3, 5 and 6).

Usage (an isolated interpreter that has tools/oracle/requirements.txt):

    python -I verify_hito2.py --ttf X.ttf --woff2 X.woff2 --expected X-expected.json

`expected.json` is written by the scenario tests (`hito2-expected.json` of
`mcp_scenario_hito2.rs`, `unicase-expected.json` of `unicase_font.rs`):
family, upm, glyphs {name: {unicodes, advance}}, cmap, optional composites,
shaping {text, glyphs} and kerning [{left, right, value}], optional kerning_domain [name].

Checks:

1. OTS accepts the TTF and the WOFF2.
2. HarfBuzz (uharfbuzz): shaping `shaping.text` gives the expected glyph
   names, and for every expected kerning pair the x advance of the first
   glyph equals its hmtx advance plus the expected value, exactly; and up to 50
   pairs without expected kerning (a strided pick over all of them, in sorted
   order) get no kerning at all.
3. fontTools: family, units per em and cmap equal the expected ones; the
   glyphs that are components (`composites`) are exactly the expected ones;
   `GPOS` has a `kern` feature with at least one lookup.
4. Chrome (if one is installed: `google-chrome`, `google-chrome-stable`,
   `chromium`, `chromium-browser` on the PATH, or the macOS app): a page
   with `@font-face` loads the WOFF2 and `document.fonts.check` says true.
   Otherwise this check prints SKIPPED, unless the environment variable
   FORMA_REQUIRE_CHROME is set (CI on Ubuntu): then it FAILS.

Exits 0 only if every check passes, 1 if one fails, 2 on a usage error or a
missing package.
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import signal
import subprocess
import sys
import tempfile
from pathlib import Path

try:
    import ots
    import uharfbuzz as hb
    from fontTools.ttLib import TTFont
except ImportError as missing:
    print(
        f"verify_hito2: a required package is missing ({missing}); "
        "install tools/oracle/requirements.txt",
        file=sys.stderr,
    )
    sys.exit(2)

MAC_CHROME = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
CHROME_NAMES = ("google-chrome", "google-chrome-stable", "chromium", "chromium-browser")
MARKER = "FORMA_FONT_CHECK"
ZERO_SAMPLE = 50


class Check:
    def __init__(self, name: str) -> None:
        self.name = name
        self.problems: list[str] = []
        self.notes: list[str] = []

    def fail(self, problem: str) -> None:
        self.problems.append(problem)

    def report(self) -> bool:
        status = "FAIL" if self.problems else "ok"
        extra = f" ({'; '.join(self.notes)})" if self.notes else ""
        print(f"[{status}] {self.name}{extra}")
        for problem in self.problems[:20]:
            print(f"    {problem}")
        if len(self.problems) > 20:
            print(f"    ... and {len(self.problems) - 20} more")
        return not self.problems


def check_ots(check: Check, *paths: Path) -> None:
    for path in paths:
        result = ots.sanitize(str(path), capture_output=True)
        output = (result.stdout + result.stderr).decode("utf-8", "replace").strip()
        if result.returncode != 0:
            check.fail(f"ots-sanitize rejected {path.name} (exit {result.returncode}): {output}")
        else:
            check.notes.append(path.name)


def check_shaping(check: Check, ttf: Path, font: TTFont, expected: dict) -> None:
    face = hb.Face(hb.Blob.from_file_path(str(ttf)))
    hb_font = hb.Font(face)  # scale = units per em: advances in font units
    order = font.getGlyphOrder()

    def shape(text: str):
        buffer = hb.Buffer()
        buffer.add_str(text)
        buffer.guess_segment_properties()
        hb.shape(hb_font, buffer, {})
        names = [
            order[info.codepoint] if info.codepoint < len(order) else f"gid{info.codepoint}"
            for info in buffer.glyph_infos
        ]
        return names, [position.x_advance for position in buffer.glyph_positions]

    shaping = expected.get("shaping")
    if shaping:
        names, _ = shape(shaping["text"])
        if names != shaping["glyphs"]:
            check.fail(f"shaping {shaping['text']!r}: got {names}, expected {shaping['glyphs']}")
    glyphs = expected["glyphs"]
    pairs = expected.get("kerning", [])
    first_char = {}
    for name, entry in glyphs.items():
        if entry["unicodes"]:
            first_char[name] = chr(int(entry["unicodes"][0][2:], 16))
    checked = 0
    for pair in pairs:
        left, right = pair["left"], pair["right"]
        text = first_char[left] + first_char[right]
        names, advances = shape(text)
        want = glyphs[left]["advance"] + pair["value"]
        if names != [left, right]:
            check.fail(f"{left} {right}: shaped to {names}")
        elif advances[0] != want:
            check.fail(f"{left} {right}: advance {advances[0]}, expected {glyphs[left]['advance']} + {pair['value']}")
        checked += 1
    if not pairs:
        check.fail("expected.json has no kerning pairs to check")
    check.notes.append(f"{checked} kerning pairs")

    # Pairs with no expected kerning must not be kerned by accident: a
    # deterministic sample (at most ZERO_SAMPLE, picked at an even stride over
    # all of them in sorted order, so it spans every left glyph rather than
    # the first few) must advance by exactly the hmtx advance.
    # `kerning_domain` (optional) names the glyphs whose every kerned pair is
    # listed; without it, every encoded glyph is in the domain.
    domain = sorted(g for g in expected.get("kerning_domain", first_char) if g in first_char)
    listed = {(pair["left"], pair["right"]) for pair in pairs}
    unkerned = sorted(
        (left, right)
        for left in domain
        for right in domain
        if (left, right) not in listed
    )
    stride = max(1, -(-len(unkerned) // ZERO_SAMPLE))  # ceil: at most ZERO_SAMPLE picks
    candidates = unkerned[::stride]
    zero_checked = 0
    for left, right in candidates:
        names, advances = shape(first_char[left] + first_char[right])
        if names != [left, right]:
            continue  # reshaped (ligature, mark or substitution): not a plain pair
        if advances[0] != glyphs[left]["advance"]:
            check.fail(
                f"{left} {right}: advance {advances[0]}, expected the unkerned {glyphs[left]['advance']}"
            )
        zero_checked += 1
    check.notes.append(f"{zero_checked} unkerned pairs")


def check_fonttools(check: Check, font: TTFont, expected: dict) -> None:
    families = sorted({r.toUnicode() for r in font["name"].names if r.nameID == 1})
    if families != [expected["family"]]:
        check.fail(f"name ID 1: got {families}, expected [{expected['family']!r}]")
    upm = font["head"].unitsPerEm
    if upm != expected["upm"]:
        check.fail(f"unitsPerEm: got {upm}, expected {expected['upm']}")
    cmap = {f"U+{code:04X}": name for code, name in (font.getBestCmap() or {}).items()}
    for key in sorted(set(cmap) | set(expected["cmap"])):
        if cmap.get(key) != expected["cmap"].get(key):
            check.fail(f"cmap {key}: got {cmap.get(key)!r}, expected {expected['cmap'].get(key)!r}")
    check.notes.append(f"{len(cmap)} cmap entries")
    if "composites" in expected:
        glyf = font["glyf"]
        found = sorted(name for name in font.getGlyphOrder() if glyf[name].isComposite())
        if found != sorted(expected["composites"]):
            check.fail(f"composite glyphs: got {found}, expected {sorted(expected['composites'])}")
        check.notes.append(f"{len(found)} composites")
    if "GPOS" not in font:
        check.fail("no GPOS table")
        return
    gpos = font["GPOS"].table
    kern_lookups = {
        index
        for record in gpos.FeatureList.FeatureRecord
        if record.FeatureTag == "kern"
        for index in record.Feature.LookupListIndex
    }
    if not kern_lookups:
        check.fail("GPOS has no kern feature with lookups")
    check.notes.append(f"GPOS kern lookups {sorted(kern_lookups)}")


def find_chrome() -> str | None:
    for name in CHROME_NAMES:
        found = shutil.which(name)
        if found:
            return found
    if sys.platform == "darwin" and os.access(MAC_CHROME, os.X_OK):
        return MAC_CHROME
    return None


def _kill_tree(proc: subprocess.Popen) -> None:
    """Kills Chrome and all its children, then reaps it."""
    try:
        if os.name == "posix":
            os.killpg(proc.pid, signal.SIGKILL)
        else:
            subprocess.run(
                ["taskkill", "/F", "/T", "/PID", str(proc.pid)],
                capture_output=True, check=False,
            )
    except OSError:
        pass
    try:
        proc.kill()
    except OSError:
        pass
    try:
        proc.wait(timeout=10)
    except subprocess.TimeoutExpired:
        pass


def check_chrome(check: Check, woff2: Path) -> bool:
    """Returns False if no browser exists (the check is skipped)."""
    chrome = find_chrome()
    if chrome is None:
        return False
    with tempfile.TemporaryDirectory(prefix="forma-chrome-") as folder:
        work = Path(folder)
        shutil.copyfile(woff2, work / "font.woff2")
        page = work / "page.html"
        page.write_text(
            f"""<!doctype html><meta charset="utf-8">
<style>@font-face {{ font-family: "FormaCheck"; src: url("font.woff2") format("woff2"); }}
body {{ font-family: "FormaCheck", monospace; }}</style>
<body>ABC<pre id="r">pending</pre>
<script>
document.fonts.load('16px "FormaCheck"', 'ABC').then(function (faces) {{
  document.getElementById('r').textContent =
    '{MARKER}=' + (faces.length > 0 && document.fonts.check('16px "FormaCheck"', 'ABC'));
}}).catch(function (error) {{
  document.getElementById('r').textContent = '{MARKER}=error ' + error;
}});
</script>""",
            encoding="utf-8",
        )
        profile = tempfile.mkdtemp(prefix="profile-", dir=work)
        command = [
            chrome,
            "--headless=new",
            "--disable-gpu",
            "--disable-extensions",
            "--disable-background-networking",
            "--disable-sync",
            "--metrics-recording-only",
            "--no-first-run",
            "--no-default-browser-check",
            "--password-store=basic",
            "--allow-file-access-from-files",
            f"--user-data-dir={profile}",
            "--virtual-time-budget=5000",
            "--dump-dom",
            page.as_uri(),
        ]
        if sys.platform == "darwin":
            command.insert(1, "--use-mock-keychain")
        if sys.platform.startswith("linux"):
            # CI containers and root users cannot start Chrome's sandbox.
            command.insert(1, "--no-sandbox")
        spawn: dict = {}
        if os.name == "posix":
            spawn["start_new_session"] = True  # own process group, killable as a whole
        else:
            spawn["creationflags"] = subprocess.CREATE_NEW_PROCESS_GROUP
        out_file, err_file = work / "stdout.txt", work / "stderr.txt"
        with open(out_file, "wb") as out, open(err_file, "wb") as err:
            proc = subprocess.Popen(
                command, stdin=subprocess.DEVNULL, stdout=out, stderr=err, **spawn
            )
            try:
                returncode = proc.wait(timeout=120)
            except subprocess.TimeoutExpired:
                _kill_tree(proc)
                tail = err_file.read_text(encoding="utf-8", errors="replace").splitlines()[-15:]
                detail = "\n".join(tail) or "(empty stderr)"
                check.fail(f"Chrome did not finish within 120 s; last stderr lines:\n{detail}")
                return True
        result = argparse.Namespace(
            stdout=out_file.read_text(encoding="utf-8", errors="replace"),
            returncode=returncode,
        )
        dom = result.stdout
        if f"{MARKER}=true" in dom:
            check.notes.append(Path(chrome).name)
        else:
            marker = next((line for line in dom.splitlines() if MARKER in line), "no marker")
            check.fail(f"document.fonts.check was not true (exit {result.returncode}): {marker.strip()}")
    return True


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("--ttf", required=True, type=Path)
    parser.add_argument("--woff2", required=True, type=Path)
    parser.add_argument("--expected", required=True, type=Path)
    args = parser.parse_args()
    for path in (args.ttf, args.woff2, args.expected):
        if not path.is_file():
            print(f"verify_hito2: {path} does not exist", file=sys.stderr)
            return 2
    expected = json.loads(args.expected.read_text(encoding="utf-8"))
    font = TTFont(str(args.ttf))
    print(f"verify_hito2: {args.ttf.name} (HarfBuzz {hb.version_string()})")

    ok = True
    ots_check = Check("OTS (TTF and WOFF2)")
    check_ots(ots_check, args.ttf, args.woff2)
    ok &= ots_check.report()
    shaping = Check("HarfBuzz shaping and kerning")
    check_shaping(shaping, args.ttf, font, expected)
    ok &= shaping.report()
    tools = Check("fontTools (names, cmap, composites, GPOS)")
    check_fonttools(tools, font, expected)
    ok &= tools.report()
    chrome = Check("Chrome @font-face and document.fonts.check")
    if check_chrome(chrome, args.woff2):
        ok &= chrome.report()
    else:
        if os.environ.get("FORMA_REQUIRE_CHROME"):
            print("[FAIL] Chrome @font-face and document.fonts.check (no Chrome or Chromium found, "
                  "and FORMA_REQUIRE_CHROME is set)")
            ok = False
        else:
            print("[SKIPPED] Chrome @font-face and document.fonts.check (no Chrome or Chromium found)")
    print("verify_hito2: " + ("OK" if ok else "FAILED"))
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
