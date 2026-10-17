"""Draw the README banner (light and dark SVG) from the outlines of the FORMA
wordmark, a five-glyph font made with the forma CLI itself.

Usage: python3 scripts/readme_banner.py [docs/assets/forma-wordmark.forma] [docs/assets]
Needs `forma` on PATH (cargo install --path core/crates/forma-cli --locked).
"""
import json
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
DOC = Path(sys.argv[1]) if len(sys.argv) > 1 else ROOT / "docs/assets/forma-wordmark.forma"
OUT = Path(sys.argv[2]) if len(sys.argv) > 2 else ROOT / "docs/assets"

W, H = 1200, 380
SCALE = 0.2
TRACK = 70  # extra spacing between letters, in font units
BASELINE = 262
CAP = 700
EDIT = "O"  # the glyph shown "in edit mode"

THEMES = {
    "light": dict(bg="#F6F5F1", ink="#141414", guide="#DCD8CE", label="#9C978B",
                  accent="#FF5A1F", tint="#FF5A1F", pill_bg="#FFFFFF", pill_ink="#141414",
                  pill_line="#E4E0D6", node_line="#FFFFFF"),
    "dark": dict(bg="#0E1014", ink="#F1EFE9", guide="#262A33", label="#5F6573",
                 accent="#FF6A2B", tint="#FF6A2B", pill_bg="#171A21", pill_ink="#F1EFE9",
                 pill_line="#2A2F3A", node_line="#0E1014"),
}


def load(g):
    args = json.dumps({"glyph": g})
    out = subprocess.run(["forma", "call", "glyph_get", "--doc-path", str(DOC), "--args", args],
                         check=True, capture_output=True, text=True).stdout
    return json.loads(out)["glyph"]


glyphs = {g: load(g) for g in "FORMA"}
total = sum(glyphs[g]["advance_width"] for g in "FORMA") + TRACK * 4
x0 = (W - total * SCALE) / 2


def pt(ox, x, y):
    return (round(ox + x * SCALE, 2), round(BASELINE - y * SCALE, 2))


def contour_d(ox, nodes):
    n = len(nodes)
    start = next(i for i, nd in enumerate(nodes) if nd["kind"] != "offcurve")
    sx, sy = pt(ox, nodes[start]["x"], nodes[start]["y"])
    d = [f"M{sx} {sy}"]
    pending = []
    for k in range(1, n + 1):
        nd = nodes[(start + k) % n]
        p = pt(ox, nd["x"], nd["y"])
        if nd["kind"] == "offcurve":
            pending.append(p)
        elif nd["kind"] == "curve":
            (a, b), (c, e) = pending
            d.append(f"C{a} {b} {c} {e} {p[0]} {p[1]}")
            pending = []
        else:
            d.append(f"L{p[0]} {p[1]}")
    return " ".join(d) + " Z"


def handles(ox, nodes):
    """Handle lines (on-curve -> off-curve) and node positions."""
    n = len(nodes)
    lines, on, off = [], [], []
    for i, nd in enumerate(nodes):
        p = pt(ox, nd["x"], nd["y"])
        if nd["kind"] == "offcurve":
            off.append(p)
            prev, nxt = nodes[(i - 1) % n], nodes[(i + 1) % n]
            # an off-curve is tied to its on-curve neighbour
            anchor = prev if prev["kind"] != "offcurve" else nxt
            lines.append((p, pt(ox, anchor["x"], anchor["y"])))
        else:
            on.append(p)
    return lines, on, off


def build(theme):
    t = THEMES[theme]
    cap_y = BASELINE - CAP * SCALE
    over_y = BASELINE - 710 * SCALE
    parts = [
        f'<svg xmlns="http://www.w3.org/2000/svg" width="{W}" height="{H}" viewBox="0 0 {W} {H}" '
        f'role="img" aria-label="FORMA">',
        "<title>FORMA</title>",
        f'<rect width="{W}" height="{H}" rx="20" fill="{t["bg"]}"/>',
    ]
    mono = "ui-monospace,SFMono-Regular,Menlo,Consolas,monospace"
    # metric guides, like a font editor's edit view
    for y, label in ((cap_y, "cap height  700"), (BASELINE, "baseline  0")):
        parts.append(f'<line x1="40" x2="{W-40}" y1="{y}" y2="{y}" stroke="{t["guide"]}" '
                     f'stroke-width="1" stroke-dasharray="4 6"/>')
        parts.append(f'<text x="44" y="{y-8}" font-family="{mono}" font-size="12" '
                     f'fill="{t["label"]}" letter-spacing="0.5">{label}</text>')
    ox = x0
    edit_box = None
    for g in "FORMA":
        gl = glyphs[g]
        d = " ".join(contour_d(ox, p["nodes"]) for p in gl["paths"])
        if g == EDIT:
            parts.append(f'<path d="{d}" fill="{t["tint"]}" fill-opacity="0.10" fill-rule="nonzero" '
                         f'stroke="{t["ink"]}" stroke-width="1.6" stroke-linejoin="round"/>')
            for p in gl["paths"]:
                lines, on, off = handles(ox, p["nodes"])
                for (a, b) in lines:
                    parts.append(f'<line x1="{a[0]}" y1="{a[1]}" x2="{b[0]}" y2="{b[1]}" '
                                 f'stroke="{t["accent"]}" stroke-width="1.2"/>')
                for (x, y) in off:
                    parts.append(f'<circle cx="{x}" cy="{y}" r="3.6" fill="{t["bg"]}" '
                                 f'stroke="{t["accent"]}" stroke-width="1.6"/>')
                for (x, y) in on:
                    parts.append(f'<rect x="{x-4.5}" y="{y-4.5}" width="9" height="9" rx="1.5" '
                                 f'fill="{t["accent"]}" stroke="{t["node_line"]}" stroke-width="1.4"/>')
            edit_box = (ox, ox + gl["advance_width"] * SCALE)
        else:
            parts.append(f'<path d="{d}" fill="{t["ink"]}" fill-rule="nonzero"/>')
        ox += (gl["advance_width"] + TRACK) * SCALE
    # the agent editing the glyph: actor + revision, as FORMA's history records it
    ex0, ex1 = edit_box
    label = "agent:claude-code  ·  editing O  ·  rev 42"
    pw, ph = round(30 + len(label) * 7.9 + 16), 30
    px, py = (ex0 + ex1) / 2 - 46, over_y - 58
    parts.append(f'<line x1="{px + 96}" y1="{py+ph}" x2="{(ex0+ex1)/2 + 52}" y2="{over_y-6}" '
                 f'stroke="{t["accent"]}" stroke-width="1.2" stroke-dasharray="3 3"/>')
    parts.append(f'<rect x="{px}" y="{py}" width="{pw}" height="{ph}" rx="15" fill="{t["pill_bg"]}" '
                 f'stroke="{t["pill_line"]}"/>')
    parts.append(f'<circle cx="{px+17}" cy="{py+ph/2}" r="4.5" fill="{t["accent"]}"/>')
    parts.append(f'<text x="{px+30}" y="{py+ph/2+4.5}" font-family="{mono}" font-size="13" '
                 f'fill="{t["pill_ink"]}">{label}</text>')
    # footer: what the picture is
    sans = "ui-sans-serif,-apple-system,'Segoe UI',Helvetica,Arial,sans-serif"
    parts.append(f'<text x="{W/2}" y="{H-36}" text-anchor="middle" font-family="{sans}" '
                 f'font-size="15" fill="{t["label"]}" letter-spacing="2.5">'
                 f'FONT EDITOR  ·  MCP-NATIVE  ·  MACOS  ·  WINDOWS  ·  LINUX</text>')
    parts.append("</svg>")
    return "\n".join(parts) + "\n"


OUT.mkdir(parents=True, exist_ok=True)
for theme in THEMES:
    (OUT / f"forma-banner-{theme}.svg").write_text(build(theme))
print(f"wrote {OUT}/forma-banner-light.svg and forma-banner-dark.svg")
