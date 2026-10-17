//! Components: references from a glyph layer to another glyph, drawn through
//! an affine transform.

use std::collections::HashMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::font::{Font, ModelError};
use crate::ids::{GlyphId, MasterId};

/// The smallest absolute determinant of a valid [`Transform`]: below it the
/// transform collapses the component onto a line or a point.
pub const MIN_TRANSFORM_DETERMINANT: f64 = 1e-6;

/// A 2D affine transform, in font units.
///
/// A point `(x, y)` maps to `(xx·x + yx·y + dx, xy·x + yy·y + dy)`, the
/// convention of `kurbo::Affine::new([xx, xy, yx, yy, dx, dy])` and of the UFO
/// component transform.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Transform {
    /// The x scale: how much `x` contributes to the new `x`.
    pub xx: f64,
    /// How much `x` contributes to the new `y`.
    pub xy: f64,
    /// How much `y` contributes to the new `x`.
    pub yx: f64,
    /// The y scale: how much `y` contributes to the new `y`.
    pub yy: f64,
    /// The horizontal offset.
    pub dx: f64,
    /// The vertical offset.
    pub dy: f64,
}

impl Transform {
    /// The transform that leaves every point where it is.
    pub const IDENTITY: Transform = Transform {
        xx: 1.0,
        xy: 0.0,
        yx: 0.0,
        yy: 1.0,
        dx: 0.0,
        dy: 0.0,
    };

    /// The determinant of the linear part, `xx·yy − xy·yx`: the factor by
    /// which the transform scales areas (negative when it mirrors).
    #[must_use]
    pub fn determinant(&self) -> f64 {
        self.xx * self.yy - self.xy * self.yx
    }

    /// Whether the transform is usable for a component: every number is
    /// finite and `|determinant| ≥ 1e-6` (see [`MIN_TRANSFORM_DETERMINANT`]).
    #[must_use]
    pub fn is_valid(&self) -> bool {
        let finite = [self.xx, self.xy, self.yx, self.yy, self.dx, self.dy]
            .iter()
            .all(|value| value.is_finite());
        finite && self.determinant().abs() >= MIN_TRANSFORM_DETERMINANT
    }

    /// Where the point `(x, y)` goes.
    #[must_use]
    pub fn apply(&self, x: f64, y: f64) -> (f64, f64) {
        (
            self.xx * x + self.yx * y + self.dx,
            self.xy * x + self.yy * y + self.dy,
        )
    }

    /// The transform that applies `self` first and `outer` second, as a
    /// component nested in another component is drawn.
    #[must_use]
    pub fn then(&self, outer: &Transform) -> Transform {
        Transform {
            xx: outer.xx * self.xx + outer.yx * self.xy,
            xy: outer.xy * self.xx + outer.yy * self.xy,
            yx: outer.xx * self.yx + outer.yx * self.yy,
            yy: outer.xy * self.yx + outer.yy * self.yy,
            dx: outer.xx * self.dx + outer.yx * self.dy + outer.dx,
            dy: outer.xy * self.dx + outer.yy * self.dy + outer.dy,
        }
    }
}

impl Default for Transform {
    /// The [identity](Transform::IDENTITY).
    fn default() -> Transform {
        Transform::IDENTITY
    }
}

/// A component: the layer of the glyph `base` in the same master, drawn
/// through `transform` as part of another glyph's layer.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Component {
    /// The glyph drawn by the component.
    pub base: GlyphId,
    /// How the base glyph is placed; the identity when omitted.
    #[serde(default)]
    pub transform: Transform,
}

/// Checks the component rules of `font` (see [`Font::validate_invariants`]),
/// reporting the first violation: each component of each glyph (in id order)
/// first, then cycles, master by master.
pub(crate) fn check_components(font: &Font) -> Result<(), ModelError> {
    for glyph in font.glyphs.values() {
        for (master, layer) in &glyph.layers {
            for component in &layer.components {
                let Some(base) = font.glyphs.get(&component.base) else {
                    return Err(ModelError::UnknownComponentBase {
                        glyph: glyph.name.clone(),
                        base: component.base,
                    });
                };
                if !base.layers.contains_key(master) {
                    return Err(ModelError::ComponentMissingLayer {
                        glyph: glyph.name.clone(),
                        base: base.name.clone(),
                        master: *master,
                    });
                }
                if !component.transform.is_valid() {
                    return Err(ModelError::InvalidTransform {
                        glyph: glyph.name.clone(),
                    });
                }
            }
        }
    }
    for master in &font.masters {
        if let Some(id) = find_cycle(font, master.id) {
            let glyph = font
                .glyphs
                .get(&id)
                .map_or_else(|| id.to_string(), |glyph| glyph.name.clone());
            return Err(ModelError::ComponentCycle { glyph });
        }
    }
    Ok(())
}

/// Where a glyph stands in the depth-first walk of [`find_cycle`].
#[derive(Clone, Copy, PartialEq, Eq)]
enum Visit {
    /// On the current path: reaching it again closes a cycle.
    Open,
    /// Fully explored: no cycle goes through it.
    Done,
}

/// A glyph on a component cycle of `master`, if there is one: the first glyph
/// reached twice by a depth-first walk from each glyph in id order. The walk
/// keeps its own stack, so a long chain of components cannot overflow the
/// call stack.
fn find_cycle(font: &Font, master: MasterId) -> Option<GlyphId> {
    let bases = |id: GlyphId| {
        font.glyphs
            .get(&id)
            .and_then(|glyph| glyph.layers.get(&master))
            .map_or(&[][..], |layer| &layer.components[..])
    };
    let mut visits: HashMap<GlyphId, Visit> = HashMap::new();
    for &start in font.glyphs.keys() {
        if visits.contains_key(&start) {
            continue;
        }
        visits.insert(start, Visit::Open);
        // Each entry is a glyph on the current path and the index of its
        // next component to follow.
        let mut path: Vec<(GlyphId, usize)> = vec![(start, 0)];
        while let Some(&(id, next)) = path.last() {
            match bases(id).get(next) {
                Some(component) => {
                    if let Some(top) = path.last_mut() {
                        top.1 += 1;
                    }
                    let base = component.base;
                    match visits.get(&base) {
                        Some(Visit::Open) => return Some(base),
                        Some(Visit::Done) => {}
                        None if font.glyphs.contains_key(&base) => {
                            visits.insert(base, Visit::Open);
                            path.push((base, 0));
                        }
                        None => {}
                    }
                }
                None => {
                    visits.insert(id, Visit::Done);
                    path.pop();
                }
            }
        }
    }
    None
}
