//! Applying commands to fonts.
//!
//! [`apply`] clones the font (cheap: glyphs are shared behind [`Arc`]s), edits
//! the clone copy-on-write and returns it, so the input is never modified and a
//! failing command leaves nothing behind.

mod components;
mod fontwide;
mod glyphs;
mod kerning;
mod nodes;
mod outlines;
mod segments;

use std::collections::BTreeSet;
use std::sync::Arc;

use forma_geometry::to_bezpath;
use forma_model::{EntityKey, Font, Glyph, GlyphId, Layer, MasterId, Path, PathId, Review};

use crate::{Command, EditError, GlyphRef};

/// The entities a command changed.
type Touched = BTreeSet<EntityKey>;

/// Applies `cmd` to `font`, returning the edited font and the entities the
/// command touched.
///
/// The input font is not modified; glyphs the command does not touch are
/// shared with it. The result always satisfies
/// [`Font::validate_invariants`].
///
/// # Errors
///
/// * [`EditError::NotFound`] if a glyph, master or contour the command refers
///   to does not exist.
/// * [`EditError::InvalidArgument`] if an argument is out of range, a name or
///   code point is already taken, or the command would leave nothing to do.
/// * [`EditError::InvalidGeometry`] if a contour entering the font is
///   malformed or has a non-finite coordinate.
pub fn apply(font: &Font, cmd: &Command) -> Result<(Font, BTreeSet<EntityKey>), EditError> {
    let mut next = font.clone();
    let touched = match cmd {
        Command::CreateGlyph {
            name,
            unicodes,
            advance_width,
            paths,
        } => glyphs::create_glyph(&mut next, name, unicodes, *advance_width, paths)?,
        Command::UpdateGlyph {
            glyph,
            name,
            unicodes,
            export,
        } => glyphs::update_glyph(
            &mut next,
            glyph,
            name.as_deref(),
            unicodes.as_deref(),
            *export,
        )?,
        Command::DeleteGlyphs { glyphs } => glyphs::delete_glyphs(&mut next, glyphs)?,
        Command::SetOutline {
            glyph,
            master,
            paths,
        } => outlines::set_outline(&mut next, glyph, *master, paths)?,
        Command::ContourEdit {
            glyph,
            master,
            action,
        } => outlines::contour_edit(&mut next, glyph, *master, action)?,
        Command::NodesEdit {
            glyph,
            master,
            path_id,
            action,
        } => nodes::nodes_edit(&mut next, glyph, *master, *path_id, action)?,
        Command::SetMetrics {
            glyph,
            master,
            advance_width,
            lsb,
            rsb,
        } => outlines::set_metrics(&mut next, glyph, *master, *advance_width, *lsb, *rsb)?,
        Command::SetFontInfo {
            family_name,
            style_name,
            version_major,
            version_minor,
            copyright,
            designer,
            units_per_em,
        } => fontwide::set_font_info(
            &mut next,
            &fontwide::InfoUpdate {
                family_name: family_name.as_deref(),
                style_name: style_name.as_deref(),
                version_major: *version_major,
                version_minor: *version_minor,
                copyright: copyright.as_deref(),
                designer: designer.as_deref(),
                units_per_em: *units_per_em,
            },
        )?,
        Command::SetVerticalMetrics {
            master,
            ascender,
            descender,
            x_height,
            cap_height,
        } => fontwide::set_vertical_metrics(
            &mut next,
            *master,
            [*ascender, *descender, *x_height, *cap_height],
        )?,
        Command::ApplyGeometryFixes {
            glyphs,
            kinds,
            grid,
        } => outlines::apply_geometry_fixes(&mut next, glyphs, kinds, *grid)?,
        Command::SetComponents {
            glyph,
            master,
            components,
        } => components::set_components(&mut next, glyph, *master, components)?,
        Command::Decompose { glyphs, master } => components::decompose(&mut next, glyphs, *master)?,
        Command::RemoveOverlap { glyphs, master } => {
            components::remove_overlaps(&mut next, glyphs, *master)?
        }
        Command::SetKerningPair {
            master,
            first,
            second,
            value,
        } => kerning::set_kerning_pair(&mut next, *master, first, second, *value)?,
        Command::SetKerningGroup {
            name,
            side,
            members,
        } => kerning::set_kerning_group(&mut next, name, *side, members)?,
        Command::DeleteKerningGroup { name, side } => {
            kerning::delete_kerning_group(&mut next, name, *side)?
        }
        Command::SetProvenance { glyph, provenance } => {
            glyphs::set_provenance(&mut next, glyph, provenance.as_ref())?
        }
    };

    next.validate_invariants().map_err(|error| {
        EditError::InvalidArgument(format!("the command would break a font invariant: {error}"))
    })?;
    Ok((next, touched))
}

/// The number of distinct glyphs `cmd` would touch in `font`, for deciding
/// whether a command is a bulk operation.
///
/// Creating a glyph counts as one; deleting, decomposing and removing
/// overlaps count every distinct listed glyph (even one the command would
/// leave alone); applying geometry fixes to an empty list counts every glyph
/// of the font; font-level and kerning commands count zero. References to
/// glyphs that do not exist count zero, so this never fails.
#[must_use]
pub fn affected_glyph_count(font: &Font, cmd: &Command) -> usize {
    let distinct = |refs: &[GlyphRef]| -> usize {
        refs.iter()
            .filter_map(|glyph| find_glyph(font, glyph))
            .collect::<BTreeSet<_>>()
            .len()
    };
    match cmd {
        Command::CreateGlyph { .. } => 1,
        Command::UpdateGlyph { glyph, .. }
        | Command::SetOutline { glyph, .. }
        | Command::ContourEdit { glyph, .. }
        | Command::NodesEdit { glyph, .. }
        | Command::SetMetrics { glyph, .. }
        | Command::SetComponents { glyph, .. }
        | Command::SetProvenance { glyph, .. } => usize::from(find_glyph(font, glyph).is_some()),
        Command::DeleteGlyphs { glyphs }
        | Command::Decompose { glyphs, .. }
        | Command::RemoveOverlap { glyphs, .. } => distinct(glyphs),
        Command::ApplyGeometryFixes { glyphs, .. } if glyphs.is_empty() => font.glyphs.len(),
        Command::ApplyGeometryFixes { glyphs, .. } => distinct(glyphs),
        Command::SetFontInfo { .. }
        | Command::SetVerticalMetrics { .. }
        | Command::SetKerningPair { .. }
        | Command::SetKerningGroup { .. }
        | Command::DeleteKerningGroup { .. } => 0,
    }
}

// -------------------------------------------------------------- provenance

/// After a transaction (from `before` to `next`, touching `touched`):
/// every touched glyph with a provenance whose design changed (see
/// [`forma_geometry::same_design`]) is marked as edited, and an approval of
/// it no longer stands, so it goes back to review. A rejection stands (the
/// glyph stays out of exported fonts until someone reviews it again).
/// Changes of metrics or metadata alone keep the review state, and so does
/// a glyph whose provenance the transaction itself set.
///
/// It runs once per transaction, not per command: spacing moves a base
/// glyph and then realigns its composites, and only the end state says
/// whether a design changed.
///
/// # Errors
///
/// [`EditError::NotFound`] cannot happen for a glyph of `next`.
pub fn mark_design_changes(
    before: &Font,
    next: &mut Font,
    touched: &BTreeSet<EntityKey>,
) -> Result<(), EditError> {
    for key in touched {
        let EntityKey::Glyph(id) = key else { continue };
        let (Some(old), Some(new)) = (before.glyphs.get(id), next.glyphs.get(id)) else {
            continue;
        };
        if new.provenance.is_none()
            || new.provenance != old.provenance
            || forma_geometry::same_design(before, old, next, new)
        {
            continue;
        }
        let glyph = glyph_mut(next, *id)?;
        if let Some(provenance) = glyph.provenance.as_mut() {
            provenance.edited = true;
            // What the engine asked to look at was about the design it made,
            // which a person has now changed.
            provenance.attention.clear();
            if provenance.review == Review::Approved {
                provenance.review = Review::Pending;
                provenance.approved_geometry = None;
            }
        }
    }
    Ok(())
}

// ------------------------------------------------------------- resolution

/// Finds the glyph a reference points at; see [`GlyphRef::resolve`].
fn find_glyph(font: &Font, glyph: &GlyphRef) -> Option<GlyphId> {
    glyph.resolve(font)
}

fn resolve_glyph(font: &Font, glyph: &GlyphRef) -> Result<GlyphId, EditError> {
    find_glyph(font, glyph).ok_or_else(|| EditError::NotFound(format!("glyph '{glyph}'")))
}

/// The glyphs a command that works on a list of glyphs (`what` it does to
/// them) refers to, each once, in the order they are listed.
fn resolve_targets(
    font: &Font,
    glyphs: &[GlyphRef],
    what: &str,
) -> Result<Vec<GlyphId>, EditError> {
    if glyphs.is_empty() {
        return Err(EditError::InvalidArgument(format!(
            "no glyphs to {what}: the list is empty"
        )));
    }
    let mut seen = BTreeSet::new();
    let mut ids = Vec::with_capacity(glyphs.len());
    for glyph in glyphs {
        let id = resolve_glyph(font, glyph)?;
        if seen.insert(id) {
            ids.push(id);
        }
    }
    Ok(ids)
}

/// The master to edit: the given one, or the default (first) master.
fn resolve_master(font: &Font, master: Option<MasterId>) -> Result<MasterId, EditError> {
    match master {
        None => Ok(font.default_master().id),
        Some(id) if font.masters.iter().any(|candidate| candidate.id == id) => Ok(id),
        Some(id) => Err(EditError::NotFound(format!("master '{id}'"))),
    }
}

// ------------------------------------------------------ copy-on-write access

fn glyph_mut(font: &mut Font, id: GlyphId) -> Result<&mut Glyph, EditError> {
    Arc::make_mut(&mut font.glyphs)
        .get_mut(&id)
        .map(Arc::make_mut)
        .ok_or_else(|| EditError::NotFound(format!("glyph '{id}'")))
}

fn layer_mut(font: &mut Font, glyph: GlyphId, master: MasterId) -> Result<&mut Layer, EditError> {
    let Glyph { name, layers, .. } = glyph_mut(font, glyph)?;
    layers.get_mut(&master).ok_or_else(|| {
        EditError::NotFound(format!("layer of glyph '{name}' for master '{master}'"))
    })
}

/// The name of a glyph and its layer in `master`, read without copying.
fn named_layer(font: &Font, id: GlyphId, master: MasterId) -> Result<(&str, &Layer), EditError> {
    let glyph = font
        .glyphs
        .get(&id)
        .ok_or_else(|| EditError::NotFound(format!("glyph '{id}'")))?;
    let layer = glyph.layers.get(&master).ok_or_else(|| {
        EditError::NotFound(format!(
            "layer of glyph '{}' for master '{master}'",
            glyph.name
        ))
    })?;
    Ok((&glyph.name, layer))
}

/// The layer of `glyph` in `master`. A glyph not drawn in that master yet
/// gets an empty layer (no contours, no components) with the advance width
/// of its layer in the `default` master, or of any layer it has.
fn layer_or_new(glyph: &mut Glyph, master: MasterId, default: MasterId) -> &mut Layer {
    let advance_width = glyph
        .layers
        .get(&default)
        .or_else(|| glyph.layers.values().next())
        .map_or(0.0, |layer| layer.advance_width);
    glyph.layers.entry(master).or_insert_with(|| Layer {
        advance_width,
        paths: Vec::new(),
        components: Vec::new(),
    })
}

// ------------------------------------------------------------- validation

/// Checks that a contour converts to a Bezier path, which also guarantees
/// that every coordinate is finite.
fn check_path(path: &Path) -> Result<(), EditError> {
    to_bezpath(path)
        .map(drop)
        .map_err(|error| EditError::InvalidGeometry(error.to_string()))
}

/// Checks the contours and numbers them from 0 in order, ignoring their ids.
fn checked_paths(paths: &[Path]) -> Result<Vec<Path>, EditError> {
    paths
        .iter()
        .enumerate()
        .map(|(index, path)| {
            to_bezpath(path)
                .map_err(|error| EditError::InvalidGeometry(format!("contour {index}: {error}")))?;
            let id = u32::try_from(index)
                .map_err(|_| EditError::InvalidArgument("too many contours".into()))?;
            Ok(Path {
                id: PathId(id),
                ..path.clone()
            })
        })
        .collect()
}

/// Checks that an advance width is a finite, non-negative number.
fn check_advance_width(value: f64) -> Result<(), EditError> {
    if value.is_finite() && value >= 0.0 {
        Ok(())
    } else {
        Err(EditError::InvalidArgument(format!(
            "advance width must be finite and not negative, got {value}"
        )))
    }
}
