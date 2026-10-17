//! Edits of the outline and the advance width of a glyph layer.

use std::collections::BTreeSet;

use forma_geometry::{IssueKind, bbox, fix_paths, to_bezpath};
use forma_model::{EntityKey, Font, GlyphId, Layer, MasterId, Node, Path, PathId};
use kurbo::{Affine, Point};

use super::segments::reversed_nodes;
use super::{
    Touched, check_advance_width, check_path, checked_paths, glyph_mut, layer_mut, resolve_glyph,
    resolve_master,
};
use crate::{ContourAction, EditError, GlyphRef};

pub(super) fn set_outline(
    font: &mut Font,
    glyph: &GlyphRef,
    master: Option<MasterId>,
    paths: &[Path],
) -> Result<Touched, EditError> {
    let id = resolve_glyph(font, glyph)?;
    let master = resolve_master(font, master)?;
    let paths = checked_paths(paths)?;
    let default = font.default_master().id;
    let target = glyph_mut(font, id)?;
    if let Some(layer) = target.layers.get_mut(&master) {
        layer.paths = paths;
    } else {
        // The glyph has not been drawn in this master yet: start from the
        // advance width of the default master (or of any layer it has).
        let advance_width = target
            .layers
            .get(&default)
            .or_else(|| target.layers.values().next())
            .map_or(0.0, |layer| layer.advance_width);
        target.layers.insert(
            master,
            Layer {
                advance_width,
                paths,
                components: Vec::new(),
            },
        );
    }
    Ok(BTreeSet::from([EntityKey::Glyph(id)]))
}

fn missing_contour(path_id: PathId, glyph: &GlyphRef) -> EditError {
    EditError::NotFound(format!("contour {} of glyph '{glyph}'", path_id.0))
}

pub(super) fn contour_edit(
    font: &mut Font,
    glyph: &GlyphRef,
    master: Option<MasterId>,
    action: &ContourAction,
) -> Result<Touched, EditError> {
    let id = resolve_glyph(font, glyph)?;
    let master = resolve_master(font, master)?;
    let layer = layer_mut(font, id, master)?;

    match action {
        ContourAction::Add { path } => {
            check_path(path)?;
            let next_id = layer
                .paths
                .iter()
                .map(|existing| existing.id.0)
                .max()
                .map_or(Some(0), |highest| highest.checked_add(1))
                .ok_or_else(|| EditError::InvalidArgument("contour ids are exhausted".into()))?;
            layer.paths.push(Path {
                id: PathId(next_id),
                ..path.clone()
            });
        }
        ContourAction::Delete { path_id } => {
            let position = layer
                .paths
                .iter()
                .position(|path| path.id == *path_id)
                .ok_or_else(|| missing_contour(*path_id, glyph))?;
            layer.paths.remove(position);
        }
        ContourAction::Reverse { path_id } => {
            let path = find_contour(&mut layer.paths, *path_id, glyph)?;
            path.nodes = reversed_nodes(path)?;
        }
        ContourAction::Transform { path_id, affine } => {
            if let Some(bad) = affine.iter().find(|value| !value.is_finite()) {
                return Err(EditError::InvalidArgument(format!(
                    "the affine coefficients must be finite, got {bad}"
                )));
            }
            let affine = Affine::new(*affine);
            let path = find_contour(&mut layer.paths, *path_id, glyph)?;
            let transformed = Path {
                id: path.id,
                closed: path.closed,
                nodes: path.nodes.iter().map(|node| moved(node, affine)).collect(),
            };
            check_path(&transformed)?;
            *path = transformed;
        }
    }
    Ok(BTreeSet::from([EntityKey::Glyph(id)]))
}

fn find_contour<'a>(
    paths: &'a mut [Path],
    path_id: PathId,
    glyph: &GlyphRef,
) -> Result<&'a mut Path, EditError> {
    paths
        .iter_mut()
        .find(|path| path.id == path_id)
        .ok_or_else(|| missing_contour(path_id, glyph))
}

fn moved(node: &Node, affine: Affine) -> Node {
    let point = affine * Point::new(node.x, node.y);
    Node {
        x: point.x,
        y: point.y,
        ..node.clone()
    }
}

pub(super) fn set_metrics(
    font: &mut Font,
    glyph: &GlyphRef,
    master: Option<MasterId>,
    advance_width: Option<f64>,
    lsb: Option<f64>,
    rsb: Option<f64>,
) -> Result<Touched, EditError> {
    let id = resolve_glyph(font, glyph)?;
    let master = resolve_master(font, master)?;
    if advance_width.is_none() && lsb.is_none() && rsb.is_none() {
        return Err(EditError::InvalidArgument(
            "nothing to set: give advance_width, lsb or rsb".into(),
        ));
    }
    if advance_width.is_some() && rsb.is_some() {
        return Err(EditError::InvalidArgument(
            "advance_width and rsb cannot be set together: the right side bearing \
             determines the advance width"
                .into(),
        ));
    }
    for (what, value) in [("advance_width", advance_width), ("lsb", lsb), ("rsb", rsb)] {
        if value.is_some_and(|v| !v.is_finite()) {
            return Err(EditError::InvalidArgument(format!("{what} must be finite")));
        }
    }

    let layer = layer_mut(font, id, master)?;
    let mut paths = layer.paths.clone();
    let mut advance = layer.advance_width;

    if let Some(lsb) = lsb {
        let rect = bbox(&paths).ok_or_else(no_contours)?;
        let dx = lsb - rect.min_x();
        paths = shifted(&paths, dx);
        advance += dx;
    }
    if let Some(rsb) = rsb {
        let rect = bbox(&paths).ok_or_else(no_contours)?;
        advance = rect.max_x() + rsb;
    }
    if let Some(advance_width) = advance_width {
        advance = advance_width;
    }
    check_advance_width(advance)?;
    if lsb.is_some() {
        // Only the shifted contours are new; others are left as they were.
        for path in &paths {
            check_path(path)?;
        }
    }

    layer.paths = paths;
    layer.advance_width = advance;
    Ok(BTreeSet::from([EntityKey::Glyph(id)]))
}

fn no_contours() -> EditError {
    EditError::InvalidArgument("the side bearings of a glyph without contours are undefined".into())
}

fn shifted(paths: &[Path], dx: f64) -> Vec<Path> {
    paths
        .iter()
        .map(|path| Path {
            nodes: path
                .nodes
                .iter()
                .map(|node| Node {
                    x: node.x + dx,
                    ..node.clone()
                })
                .collect(),
            ..path.clone()
        })
        .collect()
}

/// The new contours of the layers of one glyph that a fix changed.
type LayerFixes = Vec<(MasterId, Vec<Path>)>;

/// The smallest positive grid accepted. A smaller one cannot be a real design
/// grid, and dividing coordinates by it overflows to infinity.
const MIN_GRID: f64 = 1e-6;

pub(super) fn apply_geometry_fixes(
    font: &mut Font,
    glyphs: &[GlyphRef],
    kinds: &[IssueKind],
    grid: f64,
) -> Result<Touched, EditError> {
    if !(grid.is_finite() && grid >= 0.0) {
        return Err(EditError::InvalidArgument(format!(
            "the grid must be finite and not negative, got {grid}"
        )));
    }
    if grid > 0.0 && grid < MIN_GRID {
        return Err(EditError::InvalidArgument(format!(
            "the grid must be 0 (no grid) or at least {MIN_GRID}, got {grid}"
        )));
    }
    let targets: BTreeSet<GlyphId> = if glyphs.is_empty() {
        font.glyphs.keys().copied().collect()
    } else {
        glyphs
            .iter()
            .map(|glyph| resolve_glyph(font, glyph))
            .collect::<Result<_, _>>()?
    };

    // Work out the fixes on the shared glyphs first, so that only the glyphs
    // that really change are copied.
    let mut fixes: Vec<(GlyphId, LayerFixes)> = Vec::new();
    for id in targets {
        let Some(glyph) = font.glyphs.get(&id) else {
            continue;
        };
        let mut changed = LayerFixes::new();
        for (&master, layer) in &glyph.layers {
            let fixed = fix_paths(&layer.paths, kinds, grid);
            if fixed == layer.paths {
                continue;
            }
            // Contours entering the font must convert and be finite. Only the
            // changed ones are checked: a malformed contour that was already
            // there (and that the fixes leave alone) must not block the rest.
            for path in fixed.iter().filter(|path| !layer.paths.contains(path)) {
                to_bezpath(path).map_err(|error| {
                    EditError::InvalidGeometry(format!(
                        "fixing glyph '{}' would produce an invalid contour: {error}",
                        glyph.name
                    ))
                })?;
            }
            changed.push((master, fixed));
        }
        if !changed.is_empty() {
            fixes.push((id, changed));
        }
    }

    let mut touched = Touched::new();
    for (id, changed) in fixes {
        let glyph = glyph_mut(font, id)?;
        for (master, paths) in changed {
            if let Some(layer) = glyph.layers.get_mut(&master) {
                layer.paths = paths;
            }
        }
        touched.insert(EntityKey::Glyph(id));
    }
    Ok(touched)
}
