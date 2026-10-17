//! Creating, renaming and deleting glyphs.

use std::collections::{BTreeSet, HashSet};
use std::sync::Arc;

use forma_model::{EntityKey, Font, Glyph, GlyphId, Layer, Path, is_valid_glyph_name};

use super::{Touched, check_advance_width, checked_paths, glyph_mut, resolve_glyph};
use crate::{EditError, GlyphRef};

/// Checks that `name` is valid and not used by a glyph other than `owner`.
fn check_name(font: &Font, owner: Option<GlyphId>, name: &str) -> Result<(), EditError> {
    if !is_valid_glyph_name(name) {
        return Err(EditError::InvalidArgument(format!(
            "invalid glyph name {name:?}: use 1 to 63 printable ASCII characters without \
             spaces, not starting with a digit or a period"
        )));
    }
    let taken = font
        .glyphs
        .values()
        .any(|glyph| glyph.name == name && Some(glyph.id) != owner);
    if taken {
        return Err(EditError::InvalidArgument(format!(
            "a glyph named {name:?} already exists"
        )));
    }
    Ok(())
}

/// Checks that the code points are valid, distinct, and not used by a glyph
/// other than `owner`.
fn check_unicodes(font: &Font, owner: Option<GlyphId>, unicodes: &[u32]) -> Result<(), EditError> {
    let mut seen = HashSet::new();
    for &codepoint in unicodes {
        if char::from_u32(codepoint).is_none() {
            return Err(EditError::InvalidArgument(format!(
                "U+{codepoint:04X} is not a Unicode scalar value"
            )));
        }
        if !seen.insert(codepoint) {
            return Err(EditError::InvalidArgument(format!(
                "code point U+{codepoint:04X} is listed more than once"
            )));
        }
    }
    for glyph in font.glyphs.values() {
        if Some(glyph.id) == owner {
            continue;
        }
        if let Some(&codepoint) = glyph.unicodes.iter().find(|u| seen.contains(u)) {
            return Err(EditError::InvalidArgument(format!(
                "code point U+{codepoint:04X} already belongs to glyph '{}'",
                glyph.name
            )));
        }
    }
    Ok(())
}

pub(super) fn create_glyph(
    font: &mut Font,
    name: &str,
    unicodes: &[u32],
    advance_width: f64,
    paths: &[Path],
) -> Result<Touched, EditError> {
    check_name(font, None, name)?;
    check_unicodes(font, None, unicodes)?;
    check_advance_width(advance_width)?;
    let paths = checked_paths(paths)?;

    let id = GlyphId::new();
    let layers = font
        .masters
        .iter()
        .map(|master| {
            (
                master.id,
                Layer {
                    advance_width,
                    paths: paths.clone(),
                    components: Vec::new(),
                },
            )
        })
        .collect();
    Arc::make_mut(&mut font.glyphs).insert(
        id,
        Arc::new(Glyph {
            id,
            name: name.to_owned(),
            unicodes: unicodes.to_vec(),
            export: true,
            layers,
        }),
    );
    font.glyph_order.push(id);
    Ok(BTreeSet::from([
        EntityKey::Glyph(id),
        EntityKey::GlyphOrder,
    ]))
}

pub(super) fn update_glyph(
    font: &mut Font,
    glyph: &GlyphRef,
    name: Option<&str>,
    unicodes: Option<&[u32]>,
    export: Option<bool>,
) -> Result<Touched, EditError> {
    let id = resolve_glyph(font, glyph)?;
    if name.is_none() && unicodes.is_none() && export.is_none() {
        return Err(EditError::InvalidArgument(
            "nothing to update: give a name, unicodes or export".into(),
        ));
    }
    if let Some(name) = name {
        check_name(font, Some(id), name)?;
    }
    if let Some(unicodes) = unicodes {
        check_unicodes(font, Some(id), unicodes)?;
    }
    let target = glyph_mut(font, id)?;
    if let Some(name) = name {
        name.clone_into(&mut target.name);
    }
    if let Some(unicodes) = unicodes {
        unicodes.clone_into(&mut target.unicodes);
    }
    if let Some(export) = export {
        target.export = export;
    }
    Ok(BTreeSet::from([EntityKey::Glyph(id)]))
}

pub(super) fn delete_glyphs(font: &mut Font, glyphs: &[GlyphRef]) -> Result<Touched, EditError> {
    if glyphs.is_empty() {
        return Err(EditError::InvalidArgument(
            "no glyphs to delete: the list is empty".into(),
        ));
    }
    let ids = glyphs
        .iter()
        .map(|glyph| resolve_glyph(font, glyph))
        .collect::<Result<BTreeSet<GlyphId>, _>>()?;
    Arc::make_mut(&mut font.glyphs).retain(|id, _| !ids.contains(id));
    font.glyph_order.retain(|id| !ids.contains(id));
    let mut touched: Touched = ids.into_iter().map(EntityKey::Glyph).collect();
    touched.insert(EntityKey::GlyphOrder);
    Ok(touched)
}
