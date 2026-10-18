//! The design of a glyph, as review compares it: its outline with the
//! components expanded, in every master, up to a sideways move.
//!
//! An approval stands as long as the design does. Spacing moves a whole
//! outline sideways (and realigns composites with their bases), which is
//! not a design change; neither are advance widths, names, code points or
//! the export flag. Moving a node, or a mark relative to its base, is.

use forma_model::{Font, Glyph, MasterId, NodeKind, Path};
use sha2::{Digest, Sha256};

use crate::components::flatten_layer;

/// Coordinates are compared and hashed at this resolution (font units), so
/// that moving a whole outline sideways, which rounds differently, is not
/// mistaken for a design change.
const DESIGN_RESOLUTION: f64 = 1e-3;

fn quantize(value: f64) -> i64 {
    // Font coordinates are bounded far below the i64 range; the cast
    // saturates for anything else (NaN gives 0), which is still
    // deterministic.
    #[allow(clippy::cast_possible_truncation)]
    let q = (value / DESIGN_RESOLUTION).round() as i64;
    q
}

fn kind_code(kind: NodeKind) -> i64 {
    match kind {
        NodeKind::Line => 0,
        NodeKind::Curve => 1,
        NodeKind::QCurve => 2,
        NodeKind::OffCurve => 3,
        _ => 4,
    }
}

/// The quantized contours with every x relative to the leftmost node.
fn paths_design(paths: &[Path], out: &mut Vec<i64>) {
    let left = paths
        .iter()
        .flat_map(|path| path.nodes.iter().map(|node| node.x))
        .fold(f64::INFINITY, f64::min);
    let left = if left.is_finite() { left } else { 0.0 };
    for path in paths {
        out.push(i64::from(path.closed));
        out.push(i64::try_from(path.nodes.len()).unwrap_or(i64::MAX));
        for node in &path.nodes {
            out.extend([
                quantize(node.x - left),
                quantize(node.y),
                kind_code(node.kind),
                i64::from(node.smooth),
            ]);
        }
    }
}

/// The design of `glyph` (a glyph of `font`) in `master`: the layer with
/// its components expanded through `font`; the raw layer, components by
/// offset, if they cannot be expanded.
fn layer_design(font: &Font, glyph: &Glyph, master: MasterId, out: &mut Vec<i64>) {
    let Some(layer) = glyph.layers.get(&master) else {
        return;
    };
    if layer.components.is_empty() {
        paths_design(&layer.paths, out);
        return;
    }
    if let Ok(flat) = flatten_layer(font, glyph.id, master) {
        out.push(-2);
        paths_design(&flat, out);
        return;
    }
    paths_design(&layer.paths, out);
    out.push(-1);
    for component in &layer.components {
        let t = &component.transform;
        let base = component.base.0.as_u128();
        // The two halves of the id, bit for bit.
        #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
        out.extend([(base >> 64) as i64, base as i64]);
        for value in [t.xx, t.xy, t.yx, t.yy, t.dx, t.dy] {
            out.push(quantize(value));
        }
    }
}

fn signature(font: &Font, glyph: &Glyph) -> Vec<(MasterId, Vec<i64>)> {
    glyph
        .layers
        .keys()
        .map(|&master| {
            let mut design = Vec::new();
            layer_design(font, glyph, master, &mut design);
            (master, design)
        })
        .collect()
}

/// Whether glyph `a` of font `font_a` and glyph `b` of font `font_b` have
/// the same design: the same expanded outline in every master, up to
/// moving each layer's outline sideways. Typically `a` is a glyph before an
/// edit and `b` the same glyph after it.
#[must_use]
pub fn same_design(font_a: &Font, a: &Glyph, font_b: &Font, b: &Glyph) -> bool {
    signature(font_a, a) == signature(font_b, b)
}

/// A fingerprint of the design of `glyph` (a glyph of `font`; see
/// [`same_design`]): the lowercase hex SHA-256 of its signature. Two glyphs
/// with the same design have the same hash.
#[must_use]
pub fn design_hash(font: &Font, glyph: &Glyph) -> String {
    let mut hasher = Sha256::new();
    for (master, design) in signature(font, glyph) {
        hasher.update(master.0.as_bytes());
        for value in design {
            hasher.update(value.to_le_bytes());
        }
    }
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Arc;

    use forma_model::{Component, Glyph, GlyphId, Layer, Node, PathId, Transform};

    use super::*;

    fn square(x: f64, y: f64, size: f64) -> Path {
        let node = |x: f64, y: f64| Node {
            x,
            y,
            kind: NodeKind::Line,
            smooth: false,
        };
        Path {
            id: PathId(0),
            closed: true,
            nodes: vec![
                node(x, y),
                node(x + size, y),
                node(x + size, y + size),
                node(x, y + size),
            ],
        }
    }

    fn add(font: &mut Font, name: &str, paths: Vec<Path>, components: Vec<Component>) -> GlyphId {
        let id = GlyphId::new();
        let master = font.default_master().id;
        let glyph = Glyph {
            id,
            name: name.into(),
            unicodes: vec![],
            export: true,
            layers: BTreeMap::from([(
                master,
                Layer {
                    advance_width: 500.0,
                    paths,
                    components,
                },
            )]),
            provenance: None,
        };
        Arc::make_mut(&mut font.glyphs).insert(id, Arc::new(glyph));
        font.glyph_order.push(id);
        id
    }

    fn component(base: GlyphId, dx: f64, dy: f64) -> Component {
        Component {
            base,
            transform: Transform {
                dx,
                dy,
                ..Transform::IDENTITY
            },
        }
    }

    #[test]
    fn a_composite_moved_as_a_whole_keeps_its_design() {
        let mut font = Font::new("T", 1000);
        let base = add(&mut font, "A", vec![square(50.0, 0.0, 400.0)], vec![]);
        let mark = add(&mut font, "acute", vec![square(0.0, 600.0, 80.0)], vec![]);
        let before = add(
            &mut font,
            "Aacute",
            vec![],
            vec![component(base, 0.0, 0.0), component(mark, 210.0, 0.0)],
        );
        let a = font.glyphs[&before].clone();
        // Spacing realigns: the base moved 30 units right inside its own
        // glyph, and the composite's mark follows by 30.
        let mut after_font = font.clone();
        {
            let glyphs = Arc::make_mut(&mut after_font.glyphs);
            let base_glyph = Arc::make_mut(glyphs.get_mut(&base).expect("base"));
            let layer = base_glyph.layers.values_mut().next().expect("layer");
            layer.paths = vec![square(80.0, 0.0, 400.0)];
            let comp = Arc::make_mut(glyphs.get_mut(&before).expect("composite"));
            let layer = comp.layers.values_mut().next().expect("layer");
            layer.components[1].transform.dx = 240.0;
        }
        let b = after_font.glyphs[&before].clone();
        assert!(same_design(&font, &a, &after_font, &b));
        assert_eq!(design_hash(&font, &a), design_hash(&after_font, &b));
        // Moving the mark relative to the base is a design change.
        {
            let glyphs = Arc::make_mut(&mut after_font.glyphs);
            let comp = Arc::make_mut(glyphs.get_mut(&before).expect("composite"));
            comp.layers.values_mut().next().expect("layer").components[1]
                .transform
                .dx = 250.0;
        }
        let c = after_font.glyphs[&before].clone();
        assert!(!same_design(&font, &a, &after_font, &c));
    }
}
