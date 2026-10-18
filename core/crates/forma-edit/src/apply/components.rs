//! Components (placing glyphs in other glyphs, decomposing them) and the
//! union of overlapping contours.

use std::collections::BTreeSet;

use forma_geometry::{flatten_layer, remove_overlap, to_bezpath};
use forma_model::{Component, EntityKey, Font, GlyphId, Layer, MasterId, Path, PathId};

use super::{
    Touched, glyph_mut, layer_mut, layer_or_new, named_layer, resolve_glyph, resolve_master,
    resolve_targets,
};
use crate::{ComponentInput, EditError, GlyphRef};

pub(super) fn set_components(
    font: &mut Font,
    glyph: &GlyphRef,
    master: Option<MasterId>,
    components: &[ComponentInput],
) -> Result<Touched, EditError> {
    let id = resolve_glyph(font, glyph)?;
    let master = resolve_master(font, master)?;
    let mut resolved = Vec::with_capacity(components.len());
    for (index, input) in components.iter().enumerate() {
        let base = resolve_glyph(font, &input.base)?;
        if base == id {
            return Err(EditError::InvalidArgument(format!(
                "glyph '{glyph}' cannot be a component of itself"
            )));
        }
        if !input.transform.is_valid() {
            return Err(EditError::InvalidArgument(format!(
                "component {index} ('{}'): the transform must be finite and not singular \
                 (|xx·yy − xy·yx| ≥ 1e-6)",
                input.base
            )));
        }
        resolved.push(Component {
            base,
            transform: input.transform,
        });
    }
    // The same list on an existing layer changes nothing.
    let current = font.glyphs.get(&id).and_then(|g| g.layers.get(&master));
    if current.is_some_and(|layer| layer.components == resolved) {
        return Ok(BTreeSet::new());
    }
    // A base without a layer in the master, or a cycle, breaks the font
    // invariants, which `apply` checks afterwards.
    let default = font.default_master().id;
    layer_or_new(glyph_mut(font, id)?, master, default).components = resolved;
    Ok(BTreeSet::from([EntityKey::Glyph(id)]))
}

/// Checks that the contours a command brings into the layer of glyph `name`
/// convert to Bezier paths (which also means that they are finite).
fn check_new_paths<'a>(
    name: &str,
    what: &str,
    paths: impl IntoIterator<Item = &'a Path>,
) -> Result<(), EditError> {
    for path in paths {
        to_bezpath(path).map_err(|error| {
            EditError::InvalidGeometry(format!(
                "{what} glyph '{name}' would produce an invalid contour: {error}"
            ))
        })?;
    }
    Ok(())
}

/// Replaces the contours of each changed glyph's layer in `master` and lets
/// `edit` finish the layer; returns the touched glyphs.
fn replace_paths(
    font: &mut Font,
    master: MasterId,
    changes: Vec<(GlyphId, Vec<Path>)>,
    edit: impl Fn(&mut Layer),
) -> Result<Touched, EditError> {
    let mut touched = Touched::new();
    for (id, paths) in changes {
        let layer = layer_mut(font, id, master)?;
        layer.paths = paths;
        edit(layer);
        touched.insert(EntityKey::Glyph(id));
    }
    Ok(touched)
}

pub(super) fn decompose(
    font: &mut Font,
    glyphs: &[GlyphRef],
    master: Option<MasterId>,
) -> Result<Touched, EditError> {
    let ids = resolve_targets(font, glyphs, "decompose")?;
    let master = resolve_master(font, master)?;
    // Every new outline is worked out on the font as it is, so the order of
    // the glyphs does not matter (decomposing a glyph does not change what
    // it draws).
    let mut changes = Vec::new();
    for id in ids {
        let (name, layer) = named_layer(font, id, master)?;
        if layer.components.is_empty() {
            continue;
        }
        let flat = flatten_layer(font, id, master).map_err(|error| {
            EditError::InvalidGeometry(format!("cannot decompose glyph '{name}': {error}"))
        })?;
        // The flattened contours start with the layer's own ones, as they
        // are; those keep their ids, and the drawn components follow.
        let mut next = layer
            .paths
            .iter()
            .map(|path| path.id.0)
            .max()
            .map_or(Some(0), |highest| highest.checked_add(1));
        let mut paths = layer.paths.clone();
        for path in flat.into_iter().skip(layer.paths.len()) {
            let id = next.ok_or_else(|| {
                EditError::InvalidArgument(format!("contour ids of glyph '{name}' are exhausted"))
            })?;
            paths.push(Path {
                id: PathId(id),
                ..path
            });
            next = id.checked_add(1);
        }
        check_new_paths(name, "decomposing", &paths[layer.paths.len()..])?;
        changes.push((id, paths));
    }
    replace_paths(font, master, changes, |layer| layer.components.clear())
}

pub(super) fn remove_overlaps(
    font: &mut Font,
    glyphs: &[GlyphRef],
    master: Option<MasterId>,
) -> Result<Touched, EditError> {
    let ids = resolve_targets(font, glyphs, "clean")?;
    let master = resolve_master(font, master)?;
    let mut changes = Vec::new();
    for id in ids {
        let (name, layer) = named_layer(font, id, master)?;
        let united = remove_overlap(&layer.paths).map_err(|error| {
            EditError::InvalidGeometry(format!(
                "cannot remove the overlaps of glyph '{name}': {error}"
            ))
        })?;
        // The union returns clean contours unchanged.
        if united == layer.paths {
            continue;
        }
        check_new_paths(name, "removing the overlaps of", &united)?;
        changes.push((id, united));
    }
    replace_paths(font, master, changes, |_| {})
}
