//! Collision checks: kerned pairs whose ink comes too close.

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::rc::Rc;

use forma_model::{Font, GlyphId, MasterId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::SpacingError;
use crate::profile::{Outline, Zone, layer, line_heights};

/// The most pairs one call of [`collisions`] checks.
pub const MAX_CHECK_PAIRS: usize = 250_000;

/// The lines on which each pair is measured.
const COLLISION_LINES: usize = 256;

/// A pair whose ink comes closer than the minimum distance.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Collision {
    /// The left glyph.
    pub left: GlyphId,
    /// The right glyph.
    pub right: GlyphId,
    /// The smallest horizontal distance between the ink of the two glyphs,
    /// in font units; negative when they overlap.
    pub distance: f64,
}

/// The extents of one glyph on the lines of one zone.
type Extents = Rc<Vec<Option<(f64, f64)>>>;

/// The outlines and extents measured so far: pairs share glyphs, and pairs
/// with the same ink heights share zones.
struct Cache<'a> {
    font: &'a Font,
    master: MasterId,
    outlines: HashMap<GlyphId, Rc<Outline>>,
    extents: HashMap<(GlyphId, u64, u64), Extents>,
}

impl Cache<'_> {
    fn outline(&mut self, glyph: GlyphId) -> Result<Rc<Outline>, SpacingError> {
        match self.outlines.entry(glyph) {
            Entry::Occupied(entry) => Ok(Rc::clone(entry.get())),
            Entry::Vacant(entry) => {
                let outline = Rc::new(Outline::of(self.font, glyph, self.master)?);
                Ok(Rc::clone(entry.insert(outline)))
            }
        }
    }

    fn extents(&mut self, glyph: GlyphId, outline: &Outline, zone: Zone) -> Extents {
        let key = (glyph, zone.bottom.to_bits(), zone.top.to_bits());
        Rc::clone(self.extents.entry(key).or_insert_with(|| {
            Rc::new(
                line_heights(zone, COLLISION_LINES)
                    .into_iter()
                    .map(|y| outline.extent(y))
                    .collect(),
            )
        }))
    }
}

/// Finds the pairs of `pairs` whose ink, with the advance of the left glyph
/// and the effective kerning of the pair in `master` applied, comes closer
/// than `min_distance` horizontally.
///
/// Each pair is measured on 256 lines centred in equal bands of the union
/// of the two glyphs' ink heights (their bounding boxes); on each line with
/// ink in both glyphs the distance is the leftmost ink of the right glyph
/// minus the rightmost ink of the left one (non-zero fill, components
/// expanded), and the pair's distance is the smallest of them. A distance
/// below 0 is an overlap, 0 a touch. Pairs with a glyph without ink, or
/// without a common line of ink, never collide. The collisions are sorted
/// by ascending distance; ties keep the order of `pairs`.
///
/// # Errors
///
/// * [`SpacingError::InvalidParameter`] if there are more than
///   [`MAX_CHECK_PAIRS`] pairs, `min_distance` is not finite, or a glyph does
///   not exist.
/// * [`SpacingError::MissingLayer`] if a glyph has no layer in `master`.
/// * [`SpacingError::Geometry`] if an outline cannot be expanded.
pub fn collisions(
    font: &Font,
    master: MasterId,
    pairs: &[(GlyphId, GlyphId)],
    min_distance: f64,
) -> Result<Vec<Collision>, SpacingError> {
    if pairs.len() > MAX_CHECK_PAIRS {
        return Err(SpacingError::InvalidParameter(format!(
            "at most {MAX_CHECK_PAIRS} pairs can be checked at once, got {}",
            pairs.len()
        )));
    }
    if !min_distance.is_finite() {
        return Err(SpacingError::InvalidParameter(format!(
            "min_distance must be finite, got {min_distance}"
        )));
    }
    let mut cache = Cache {
        font,
        master,
        outlines: HashMap::new(),
        extents: HashMap::new(),
    };
    let mut found = Vec::new();
    for &(left, right) in pairs {
        let advance = layer(font, left, master)?.advance_width;
        layer(font, right, master)?;
        let (left_outline, right_outline) = (cache.outline(left)?, cache.outline(right)?);
        let (Some(left_box), Some(right_box)) = (left_outline.bbox, right_outline.bbox) else {
            continue;
        };
        let zone = Zone {
            bottom: left_box.y0.min(right_box.y0),
            top: left_box.y1.max(right_box.y1),
        };
        let offset = advance + font.kerning.effective(master, left, right);
        let left_extents = cache.extents(left, &left_outline, zone);
        let right_extents = cache.extents(right, &right_outline, zone);
        let distance = left_extents
            .iter()
            .zip(right_extents.iter())
            .filter_map(|(l, r)| Some(offset + r.as_ref()?.0 - l.as_ref()?.1))
            .reduce(f64::min);
        if let Some(distance) = distance
            && distance < min_distance
        {
            found.push(Collision {
                left,
                right,
                distance,
            });
        }
    }
    found.sort_by(|a, b| a.distance.total_cmp(&b.distance));
    Ok(found)
}
