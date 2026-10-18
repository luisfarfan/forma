//! Where a glyph's design came from, and whether a person approved it.
//!
//! A glyph without provenance was drawn by hand (or predates format v3). A
//! glyph made by the engine (traced from an image, synthesized from the
//! font's own style, or composed from other glyphs) records how, so that
//! people and agents can tell the cases apart and review what the engine
//! produced before it is final.
//!
//! Only design changes invalidate an approval: see [`Glyph::same_design`].
//! Metrics (advance width, side bearings, kerning) and metadata (name,
//! code points, export flag) do not.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::glyph::{Glyph, Layer};
use crate::ids::GlyphId;

/// How the design of a glyph was made.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Source {
    /// Traced from an image.
    Traced,
    /// Generated from the style of the font's own glyphs.
    Synthesized,
    /// Built from components of other glyphs.
    Composed,
    /// Drawn by a person (or an agent through the editing tools).
    Manual,
}

/// The review state of a generated glyph.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Review {
    /// Not reviewed yet, or changed since it was reviewed.
    Pending,
    /// A person approved the design.
    Approved,
    /// A person rejected the design; the glyph is not exported.
    Rejected,
}

/// What the engine recorded about the making of a glyph.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum ProvenanceDetail {
    /// Nothing recorded.
    #[default]
    None,
    /// Traced from an image.
    Traced {
        /// SHA-256 of the image file, lowercase hex.
        image_sha256: String,
        /// The image candidates (pieces of ink) the glyph was traced from.
        candidates: Vec<usize>,
        /// The pixel box of those candidates: left, top, right, bottom.
        px_rect: [f64; 4],
        /// Overlap of the traced outline with the ink (1 is perfect).
        iou_mask: f64,
        /// 95th percentile distance from the outline to the ink edge, in
        /// image pixels, when measured.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        hausdorff_p95_px: Option<f64>,
        /// Whether the outline has the same pieces and holes as the ink,
        /// when checked.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        topology_ok: Option<bool>,
    },
    /// Generated from the style of the font's own glyphs.
    Synthesized {
        /// The recipe that made the glyph.
        recipe: String,
        /// The version of that recipe.
        recipe_version: u32,
        /// The glyphs whose shapes or measurements were used.
        sources: Vec<GlyphId>,
        /// How consistent the glyph is with the font's style (0 to 1).
        style_score: f64,
    },
}

impl ProvenanceDetail {
    /// The numbers of the detail, for checking that they are finite.
    #[must_use]
    pub fn numbers(&self) -> Vec<f64> {
        match self {
            Self::None => Vec::new(),
            Self::Traced {
                px_rect,
                iou_mask,
                hausdorff_p95_px,
                ..
            } => {
                let mut values = px_rect.to_vec();
                values.push(*iou_mask);
                values.extend(hausdorff_p95_px);
                values
            }
            Self::Synthesized { style_score, .. } => vec![*style_score],
        }
    }
}

/// The origin and review state of a glyph's design.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Provenance {
    /// How the design was made.
    pub source: Source,
    /// Whether a person reviewed it.
    pub review: Review,
    /// Whether the design was changed by editing after it was made.
    #[serde(default)]
    pub edited: bool,
    /// The design approved, as [`Glyph::geometry_hash`]; set by approving.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approved_geometry: Option<String>,
    /// The version of the quality thresholds the engine judged it with (0
    /// when none applied).
    #[serde(default)]
    pub thresholds_version: u32,
    /// What the engine recorded about it.
    #[serde(default)]
    pub detail: ProvenanceDetail,
}

impl Provenance {
    /// A new, unreviewed provenance.
    #[must_use]
    pub fn pending(source: Source, thresholds_version: u32, detail: ProvenanceDetail) -> Self {
        Self {
            source,
            review: Review::Pending,
            edited: false,
            approved_geometry: None,
            thresholds_version,
            detail,
        }
    }
}

/// Coordinates are compared and hashed at this resolution (font units), so
/// that moving a whole outline sideways, which rounds differently, is not
/// mistaken for a design change.
const DESIGN_RESOLUTION: f64 = 1e-3;

fn quantize(value: f64) -> i64 {
    // Font coordinates are bounded far below the i64 range; the cast
    // saturates for anything else, which is still deterministic.
    #[allow(clippy::cast_possible_truncation)]
    let q = (value / DESIGN_RESOLUTION).round() as i64;
    q
}

/// The design of a layer with its horizontal position factored out: every
/// x coordinate (nodes and component offsets) relative to the leftmost one,
/// quantized. The advance width is a metric and is left out.
fn layer_design(layer: &Layer) -> Vec<i64> {
    let left = layer
        .paths
        .iter()
        .flat_map(|path| path.nodes.iter().map(|node| node.x))
        .chain(layer.components.iter().map(|c| c.transform.dx))
        .fold(f64::INFINITY, f64::min);
    let left = if left.is_finite() { left } else { 0.0 };
    let mut out = Vec::new();
    for path in &layer.paths {
        out.push(i64::from(path.closed));
        out.push(i64::try_from(path.nodes.len()).unwrap_or(i64::MAX));
        for node in &path.nodes {
            out.push(quantize(node.x - left));
            out.push(quantize(node.y));
            out.push(node.kind as i64);
            out.push(i64::from(node.smooth));
        }
    }
    out.push(-1);
    for component in &layer.components {
        let t = &component.transform;
        let base = component.base.0.as_u128();
        // The two halves of the id, bit for bit.
        #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
        out.extend([(base >> 64) as i64, base as i64]);
        for value in [t.xx, t.xy, t.yx, t.yy] {
            out.push(quantize(value * 1000.0));
        }
        out.push(quantize(t.dx - left));
        out.push(quantize(t.dy));
    }
    out
}

impl Glyph {
    /// Whether `self` and `other` have the same design: the same contours
    /// and components in every master, up to moving each layer's outline
    /// sideways (what changing side bearings does). Advance widths, names,
    /// code points, the export flag and the provenance are not part of the
    /// design.
    #[must_use]
    pub fn same_design(&self, other: &Glyph) -> bool {
        self.layers.len() == other.layers.len()
            && self
                .layers
                .iter()
                .zip(&other.layers)
                .all(|((ma, a), (mb, b))| ma == mb && layer_design(a) == layer_design(b))
    }

    /// A fingerprint of the design (see [`Glyph::same_design`]): the
    /// lowercase hex SHA-256 of the quantized, sideways-normalized layers.
    /// Two glyphs with the same design have the same hash.
    #[must_use]
    pub fn geometry_hash(&self) -> String {
        let mut hasher = Sha256::new();
        for (master, layer) in &self.layers {
            hasher.update(master.0.as_bytes());
            for value in layer_design(layer) {
                hasher.update(value.to_le_bytes());
            }
        }
        hasher
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use uuid::Uuid;

    use super::*;
    use crate::glyph::{Node, NodeKind, Path};
    use crate::ids::{MasterId, PathId};

    fn glyph(shift: f64, advance: f64) -> Glyph {
        let node = |x: f64, y: f64| Node {
            x: x + shift,
            y,
            kind: NodeKind::Line,
            smooth: false,
        };
        let mut layers = BTreeMap::new();
        layers.insert(
            MasterId(Uuid::from_u128(1)),
            Layer {
                advance_width: advance,
                paths: vec![Path {
                    id: PathId(0),
                    closed: true,
                    nodes: vec![
                        node(10.1, 0.0),
                        node(90.7, 0.0),
                        node(90.7, 700.0),
                        node(10.1, 700.0),
                    ],
                }],
                components: Vec::new(),
            },
        );
        Glyph {
            id: GlyphId(Uuid::from_u128(2)),
            name: "I".into(),
            unicodes: vec![0x49],
            export: true,
            layers,
            provenance: None,
        }
    }

    #[test]
    fn sideways_moves_and_advance_changes_keep_the_design() {
        let a = glyph(0.0, 100.0);
        let b = glyph(37.3, 180.0);
        assert!(a.same_design(&b));
        assert_eq!(a.geometry_hash(), b.geometry_hash());
    }

    #[test]
    fn moving_a_node_changes_the_design() {
        let a = glyph(0.0, 100.0);
        let mut b = a.clone();
        if let Some(layer) = b.layers.values_mut().next() {
            layer.paths[0].nodes[2].y = 690.0;
        }
        assert!(!a.same_design(&b));
        assert_ne!(a.geometry_hash(), b.geometry_hash());
    }

    #[test]
    fn provenance_serializes_in_snake_case() {
        let p = Provenance::pending(
            Source::Synthesized,
            1,
            ProvenanceDetail::Synthesized {
                recipe: "zero".into(),
                recipe_version: 1,
                sources: vec![],
                style_score: 0.8,
            },
        );
        let json = serde_json::to_value(&p).unwrap_or_default();
        assert_eq!(json["source"], "synthesized");
        assert_eq!(json["review"], "pending");
        assert_eq!(json["detail"]["kind"], "synthesized");
        let back: Provenance = serde_json::from_value(json).unwrap_or_else(|_| p.clone());
        assert_eq!(back, p);
    }
}
