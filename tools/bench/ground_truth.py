#!/usr/bin/env python3
"""Ground truth of an OFL test font: advances, side bearings and the kerning
of GPOS flattened to glyph pairs, for the capitals A-Z.

Usage: ground_truth.py <font.ttf> > truth.json

Run it with the oracle venv (fontTools): tools/oracle/.venv/bin/python.

Output (JSON):
  units_per_em
  glyphs:  {"A": {"name", "advance", "lsb", "rsb", "xmin", "xmax"}, ...}
  kerning: [{"left": "A", "right": "V", "value": -74}, ...] (nonzero only,
           in font units, the sum over the lookups of the `kern` feature, as
           a shaper applies them to the pair)
"""

import json
import string
import sys

from fontTools.pens.boundsPen import BoundsPen
from fontTools.ttLib import TTFont


def lookups_of_feature(font, tag):
    gpos = font["GPOS"].table
    indices = []
    for record in gpos.FeatureList.FeatureRecord:
        if record.FeatureTag == tag:
            for index in record.Feature.LookupListIndex:
                if index not in indices:
                    indices.append(index)
    return [gpos.LookupList.Lookup[i] for i in sorted(indices)]


def pair_subtables(lookup):
    for sub in lookup.SubTable:
        if lookup.LookupType == 9:  # Extension positioning
            sub = sub.ExtSubTable
            if sub.LookupType != 2:
                continue
        elif lookup.LookupType != 2:
            continue
        yield sub


def x_advance(value):
    return getattr(value, "XAdvance", 0) or 0 if value is not None else 0


def pair_value(sub, left, right):
    """The kerning a PairPos subtable gives to (left, right), or None if the
    subtable does not cover the pair."""
    coverage = sub.Coverage.glyphs
    if left not in coverage:
        return None
    if sub.Format == 1:
        pair_set = sub.PairSet[coverage.index(left)]
        for record in pair_set.PairValueRecord:
            if record.SecondGlyph == right:
                return x_advance(record.Value1)
        return None
    class1 = sub.ClassDef1.classDefs.get(left, 0)
    class2 = sub.ClassDef2.classDefs.get(right, 0)
    record = sub.Class1Record[class1].Class2Record[class2]
    return x_advance(record.Value1)


def kerning(font, names):
    if "GPOS" not in font:
        return []
    lookups = lookups_of_feature(font, "kern")
    pairs = []
    for left_char, left in names.items():
        for right_char, right in names.items():
            total = 0
            for lookup in lookups:
                for sub in pair_subtables(lookup):
                    value = pair_value(sub, left, right)
                    if value is not None:
                        total += value
                        break  # the first subtable that covers the pair wins
            if total:
                pairs.append({"left": left_char, "right": right_char, "value": total})
    return pairs


def main(path):
    font = TTFont(path)
    cmap = font.getBestCmap()
    glyph_set = font.getGlyphSet()
    hmtx = font["hmtx"]
    names = {c: cmap[ord(c)] for c in string.ascii_uppercase if ord(c) in cmap}
    glyphs = {}
    for char, name in names.items():
        pen = BoundsPen(glyph_set)
        glyph_set[name].draw(pen)
        advance, _ = hmtx[name]
        xmin, _, xmax, _ = pen.bounds if pen.bounds else (0, 0, 0, 0)
        glyphs[char] = {
            "name": name,
            "advance": advance,
            "lsb": xmin,
            "rsb": advance - xmax,
            "xmin": xmin,
            "xmax": xmax,
        }
    json.dump(
        {
            "units_per_em": font["head"].unitsPerEm,
            "glyphs": glyphs,
            "kerning": kerning(font, names),
        },
        sys.stdout,
        indent=1,
        sort_keys=True,
    )
    sys.stdout.write("\n")


if __name__ == "__main__":
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    main(sys.argv[1])
