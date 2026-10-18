//! Flattening of glyph components into plain contours.

use forma_model::{Font, GlyphId, MasterId, Path, PathId, Transform};
use kurbo::Rect;

use crate::{GeometryError, bbox, transform_paths};

/// The deepest component nesting [`flatten_layer`] follows. Real fonts nest
/// two or three levels; the limit only keeps a hand-built chain from
/// overflowing the call stack.
const MAX_COMPONENT_DEPTH: usize = 64;

/// The most layers [`flatten_layer`] visits for one glyph, counting a base
/// each time a component draws it. A font where every level uses the next one
/// twice reaches an astronomical count without any cycle; the limit stops it.
const MAX_VISITED_LAYERS: usize = 10_000;

/// The walk over the components of one layer.
struct Flattener<'a> {
    font: &'a Font,
    master: MasterId,
    /// The glyphs being expanded, outermost first: reaching one again is a
    /// cycle.
    stack: Vec<GlyphId>,
    visited: usize,
    paths: Vec<Path>,
}

impl Flattener<'_> {
    /// Adds the contours of `glyph` and, recursively, those of its
    /// components, all moved by `to_root` (the transform from the glyph's own
    /// space to the space of the glyph being flattened).
    fn visit(&mut self, glyph: GlyphId, to_root: &Transform) -> Result<(), GeometryError> {
        if self.stack.contains(&glyph) {
            return Err(GeometryError::InvalidPath(format!(
                "glyph {glyph} contains itself through its components"
            )));
        }
        if self.stack.len() >= MAX_COMPONENT_DEPTH {
            return Err(GeometryError::InvalidPath(format!(
                "components are nested more than {MAX_COMPONENT_DEPTH} levels deep"
            )));
        }
        self.visited += 1;
        if self.visited > MAX_VISITED_LAYERS {
            return Err(GeometryError::InvalidPath(format!(
                "the glyph expands to more than {MAX_VISITED_LAYERS} component layers"
            )));
        }

        let source =
            self.font.glyphs.get(&glyph).ok_or_else(|| {
                GeometryError::InvalidPath(format!("glyph {glyph} does not exist"))
            })?;
        let layer = source.layers.get(&self.master).ok_or_else(|| {
            GeometryError::InvalidPath(format!(
                "glyph {} has no layer for master {}",
                source.name, self.master
            ))
        })?;

        self.paths.extend(transform_paths(&layer.paths, to_root));

        self.stack.push(glyph);
        for component in &layer.components {
            if !component.transform.is_valid() {
                return Err(GeometryError::InvalidPath(format!(
                    "a component of glyph {} has an invalid transform",
                    source.name
                )));
            }
            // The component's own transform acts first, then the one that
            // places the glyph that holds it.
            let composed = component.transform.then(to_root);
            self.visit(component.base, &composed)?;
        }
        self.stack.pop();
        Ok(())
    }
}

/// The contours of `glyph` in `master` with every component replaced by the
/// contours of its base, moved by the component transform.
///
/// The layer's own contours come first, then those of each component in
/// order. Components nested in components are expanded recursively, the
/// transforms composing as in [`Transform::then`]; a base used twice is
/// drawn twice. A mirroring transform keeps contour directions (see
/// [`transform_paths`]), and two mirrors in a row cancel out. The returned
/// contours are numbered from 0 in that order.
///
/// # Errors
///
/// [`GeometryError::InvalidPath`] if the glyph, or the base of a component,
/// does not exist or has no layer in `master`; if a component transform is
/// not [valid](Transform::is_valid); or if the components form a cycle, nest
/// more than 64 levels or expand to more than 10 000 layers. A font that
/// passes [`Font::validate_invariants`] has none of these problems except
/// possibly the two size limits; the checks make a hand-built font fail
/// cleanly instead of looping or overflowing the stack.
pub fn flatten_layer(
    font: &Font,
    glyph: GlyphId,
    master: MasterId,
) -> Result<Vec<Path>, GeometryError> {
    let mut flattener = Flattener {
        font,
        master,
        stack: Vec::new(),
        visited: 0,
        paths: Vec::new(),
    };
    flattener.visit(glyph, &Transform::IDENTITY)?;
    let mut paths = flattener.paths;
    for (index, path) in paths.iter_mut().enumerate() {
        let id = u32::try_from(index)
            .map_err(|_| GeometryError::InvalidPath("too many contours".into()))?;
        path.id = PathId(id);
    }
    Ok(paths)
}

/// The exact bounding box of the glyph layer with its components expanded
/// (see [`flatten_layer`] and [`bbox`]), or `None` if it has no outline.
///
/// # Errors
///
/// The same as [`flatten_layer`].
pub fn layer_bbox(
    font: &Font,
    glyph: GlyphId,
    master: MasterId,
) -> Result<Option<Rect>, GeometryError> {
    Ok(bbox(&flatten_layer(font, glyph, master)?))
}
