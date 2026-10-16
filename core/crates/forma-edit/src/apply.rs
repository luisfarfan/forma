//! Applying commands to fonts.
//!
//! [`apply`] clones the font (cheap: glyphs are shared behind [`Arc`]s), edits
//! the clone copy-on-write and returns it, so the input is never modified and a
//! failing command leaves nothing behind.

mod fontwide;
mod glyphs;
mod nodes;
mod outlines;
mod segments;

use std::collections::BTreeSet;
use std::sync::Arc;

use forma_geometry::to_bezpath;
use forma_model::{EntityKey, Font, Glyph, GlyphId, Layer, MasterId, Path, PathId};

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
    };
    next.validate_invariants().map_err(|error| {
        EditError::InvalidArgument(format!("the command would break a font invariant: {error}"))
    })?;
    Ok((next, touched))
}

/// The number of distinct glyphs `cmd` would touch in `font`, for deciding
/// whether a command is a bulk operation.
///
/// Creating a glyph counts as one; deleting counts every distinct listed
/// glyph; applying geometry fixes to an empty list counts every glyph of the
/// font; font-level commands count zero. References to glyphs that do not
/// exist count zero, so this never fails.
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
        | Command::SetMetrics { glyph, .. } => usize::from(find_glyph(font, glyph).is_some()),
        Command::DeleteGlyphs { glyphs } => distinct(glyphs),
        Command::ApplyGeometryFixes { glyphs, .. } if glyphs.is_empty() => font.glyphs.len(),
        Command::ApplyGeometryFixes { glyphs, .. } => distinct(glyphs),
        Command::SetFontInfo { .. } | Command::SetVerticalMetrics { .. } => 0,
    }
}

// ------------------------------------------------------------- resolution

/// Finds the glyph a reference points at.
///
/// An id is looked up first; when no glyph has it, a glyph whose name is the
/// same UUID is used. A name is only ever matched against glyph names.
fn find_glyph(font: &Font, glyph: &GlyphRef) -> Option<GlyphId> {
    match glyph {
        GlyphRef::Id(id) => {
            if font.glyphs.contains_key(id) {
                Some(*id)
            } else {
                font.glyphs
                    .values()
                    .find(|candidate| candidate.name.parse::<GlyphId>().ok() == Some(*id))
                    .map(|candidate| candidate.id)
            }
        }
        GlyphRef::Name(name) => font.glyph_by_name(name).map(|candidate| candidate.id),
    }
}

fn resolve_glyph(font: &Font, glyph: &GlyphRef) -> Result<GlyphId, EditError> {
    find_glyph(font, glyph).ok_or_else(|| EditError::NotFound(format!("glyph '{glyph}'")))
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
