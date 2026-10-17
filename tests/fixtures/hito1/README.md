# Fixture `hito1`: the glyphs of the Milestone 1 agent scenario

`glyphs.json` holds twelve glyphs (`.notdef space A B C H O a b c n o`) of a small geometric sans
drawn for FORMA's tests. It is **original work** of the FORMA project, licensed like the rest of the
repository under **MIT OR Apache-2.0**. It is not derived from any existing typeface.

## Contents

- `family_name`, `units_per_em` (1000) and `vertical_metrics` (ascender 800, descender -200,
  cap height 700, x-height 500, the defaults of a new FORMA document).
- `glyphs`: each entry is exactly the arguments of a `glyph_create` operation (`name`, `unicodes`,
  `advance_width` and, except for `space`, `svg_path`), so the scenario can send them as they are
  inside one `edit_batch`.

## Design rules

- Font units, y axis up, integer coordinates only.
- Outer contours run counter-clockwise and inner contours (counters) clockwise.
- Round shapes are explicit cubic `C` segments with a node at every horizontal and vertical extreme
  (four segments per ellipse, handles at 0.5523 of the radius); there are no SVG arcs. Partial arcs
  (the terminals of `C` and `c`, the joints of `a`, `b` and `B`) are split at the extremes too.
- No overlapping contours.

**One defect is deliberate:** the counter of `o` (its second contour) runs counter-clockwise, the wrong
direction. It is the only issue `geometry_check` reports for this set (`wrong_direction`, fixable),
and `geometry_fix` repairs it. Do not "fix" it here: the scenario test depends on it.
