//! UFO export.
//!
//! A UFO is a folder of property lists and `.glif` files. The one written here
//! is the source of a single master: the font compiler accepts it directly,
//! and other font editors can open it.

use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use forma_model::{Font, Glyph, Layer, Master, MasterId, Node, NodeKind, Path as Contour};
use norad::fontinfo::NonNegativeIntegerOrFloat;
use norad::{Codepoints, ContourPoint, Name, PointType};
use thiserror::Error;
use uuid::Uuid;

/// The file that marks a folder as a UFO.
const METAINFO_FILE: &str = "metainfo.plist";

/// The key of `lib.plist` that holds the glyph order.
const GLYPH_ORDER_KEY: &str = "public.glyphOrder";

/// Why an export failed.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum InteropError {
    /// A glyph to export has no layer for the requested master.
    #[error("glyph {glyph:?} has no layer for master {master}")]
    MissingLayer {
        /// The name of the glyph.
        glyph: String,
        /// The requested master.
        master: MasterId,
    },
    /// The requested master is not in the font.
    #[error("the font has no master {0}")]
    UnknownMaster(MasterId),
    /// The destination exists and is not a UFO, so it was left untouched.
    #[error("{} already exists and is not a UFO; it was left untouched", .0.display())]
    DestinationExists(PathBuf),
    /// The font holds data that cannot be written as a UFO, or the UFO library
    /// failed to write it for a reason other than the file system.
    #[error("UFO error: {0}")]
    Norad(String),
    /// Reading or writing files failed. A write failure of the UFO library
    /// whose cause is a file system error is reported here too, with the
    /// library's description and the kind of that error.
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
}

/// Writes one master of `font` as a UFO 3 folder at `dir`.
///
/// Only glyphs with `export` set are written, each with its advance width,
/// code points and contours from the layer of `master`; `public.glyphOrder`
/// in `lib.plist` lists them in the glyph order of the font. Font info comes
/// from the font and from the vertical metrics of `master`.
///
/// Nothing is written into `dir` directly: the UFO is built in a sibling
/// folder (`<dir>.tmp-<uuid>`) and moved into place only once complete. Other
/// data is never overwritten:
///
/// - if `dir` does not exist (its parent folders are created), the UFO is
///   written there;
/// - if `dir` is a UFO (a folder holding `metainfo.plist`), it is replaced by
///   the new one, and kept as it was if that fails;
/// - anything else at `dir` (a file, a symbolic link, any other folder) is
///   refused with [`InteropError::DestinationExists`].
///
/// # Errors
///
/// [`InteropError::UnknownMaster`] if the font has no such master,
/// [`InteropError::MissingLayer`] if an exported glyph has no layer for it,
/// [`InteropError::DestinationExists`] as described above,
/// [`InteropError::Norad`] if a name, code point or node cannot be expressed
/// in a UFO or the UFO cannot be written, and [`InteropError::Io`] if the
/// file system fails. On any error the previous contents of `dir` are intact.
pub fn write_ufo(font: &Font, master: MasterId, dir: &Path) -> Result<(), InteropError> {
    // Everything that can fail on account of the font happens before the file
    // system is touched.
    let ufo = build_ufo(font, master)?;
    install(&ufo, dir)
}

// ---------------------------------------------------------------------------
// Building the UFO in memory
// ---------------------------------------------------------------------------

fn build_ufo(font: &Font, master_id: MasterId) -> Result<norad::Font, InteropError> {
    let master = font
        .masters
        .iter()
        .find(|master| master.id == master_id)
        .ok_or(InteropError::UnknownMaster(master_id))?;

    let mut ufo = norad::Font::new();
    ufo.font_info = font_info(font, master);

    let mut glyph_order = Vec::new();
    for glyph in exported_glyphs(font) {
        let layer = glyph
            .layers
            .get(&master_id)
            .ok_or_else(|| InteropError::MissingLayer {
                glyph: glyph.name.clone(),
                master: master_id,
            })?;
        ufo.default_layer_mut()
            .insert_glyph(ufo_glyph(glyph, layer)?);
        glyph_order.push(glyph.name.clone());
    }

    // The property list value types come from a crate that norad does not
    // re-export: the element type is inferred from the only conversion a list
    // of values has.
    let glyph_order: Vec<_> = glyph_order.into_iter().map(Into::into).collect();
    ufo.lib
        .insert(GLYPH_ORDER_KEY.to_owned(), glyph_order.into());
    Ok(ufo)
}

/// The glyphs to export, in glyph order.
///
/// A well-formed font lists every glyph in its glyph order; an exportable
/// glyph that is missing from it is still written, after the listed ones,
/// rather than silently lost.
fn exported_glyphs(font: &Font) -> Vec<&Glyph> {
    let mut seen = HashSet::new();
    let mut glyphs = Vec::new();
    for id in &font.glyph_order {
        if let Some(glyph) = font.glyphs.get(id)
            && glyph.export
            && seen.insert(*id)
        {
            glyphs.push(&**glyph);
        }
    }
    for (id, glyph) in font.glyphs.iter() {
        if glyph.export && !seen.contains(id) {
            glyphs.push(&**glyph);
        }
    }
    glyphs
}

fn font_info(font: &Font, master: &Master) -> norad::FontInfo {
    let info = &font.info;
    let metrics = &master.metrics;
    // norad writes a whole number as a plist integer and anything else as a
    // real, which is what the UFO number types call for.
    norad::FontInfo {
        family_name: Some(info.family_name.clone()),
        style_name: Some(info.style_name.clone()),
        units_per_em: Some(NonNegativeIntegerOrFloat::from(u32::from(
            font.units_per_em,
        ))),
        ascender: Some(metrics.ascender),
        descender: Some(metrics.descender),
        x_height: Some(metrics.x_height),
        cap_height: Some(metrics.cap_height),
        version_major: Some(i32::from(info.version_major)),
        version_minor: Some(u32::from(info.version_minor)),
        copyright: info.copyright.clone(),
        open_type_name_designer: info.designer.clone(),
        ..norad::FontInfo::default()
    }
}

fn ufo_glyph(glyph: &Glyph, layer: &Layer) -> Result<norad::Glyph, InteropError> {
    // `norad::Glyph::new` panics on a name that is empty or has control
    // characters, so check it with the fallible constructor first.
    Name::new(&glyph.name).map_err(|error| InteropError::Norad(error.to_string()))?;

    let codepoints = glyph
        .unicodes
        .iter()
        .map(|&codepoint| {
            char::from_u32(codepoint).ok_or_else(|| {
                InteropError::Norad(format!(
                    "glyph {:?}: U+{codepoint:04X} is not a Unicode scalar value",
                    glyph.name
                ))
            })
        })
        .collect::<Result<Vec<char>, _>>()?;

    let mut out = norad::Glyph::new(&glyph.name);
    out.width = layer.advance_width;
    out.codepoints = Codepoints::new(codepoints);
    out.contours = layer
        .paths
        .iter()
        .map(|path| ufo_contour(&glyph.name, path))
        .collect::<Result<_, _>>()?;
    Ok(out)
}

/// Converts a contour. The model already uses the UFO convention (off-curve
/// points precede the on-curve point they lead into), so the nodes keep their
/// order; the only change is that an open contour starts with a `move`.
fn ufo_contour(glyph: &str, path: &Contour) -> Result<norad::Contour, InteropError> {
    let points = path
        .nodes
        .iter()
        .enumerate()
        .map(|(index, node)| ufo_point(glyph, node, !path.closed && index == 0))
        .collect::<Result<_, _>>()?;
    Ok(norad::Contour::new(points, None))
}

fn ufo_point(
    glyph: &str,
    node: &Node,
    starts_open_contour: bool,
) -> Result<ContourPoint, InteropError> {
    let typ = if starts_open_contour {
        PointType::Move
    } else {
        match node.kind {
            NodeKind::Line => PointType::Line,
            NodeKind::Curve => PointType::Curve,
            NodeKind::QCurve => PointType::QCurve,
            NodeKind::OffCurve => PointType::OffCurve,
            other => {
                return Err(InteropError::Norad(format!(
                    "glyph {glyph:?}: node kind {other:?} cannot be written as a UFO point"
                )));
            }
        }
    };
    // UFO does not allow a smooth off-curve point.
    let smooth = node.smooth && typ != PointType::OffCurve;
    Ok(ContourPoint::new(node.x, node.y, typ, smooth, None, None))
}

// ---------------------------------------------------------------------------
// Putting it on disk
// ---------------------------------------------------------------------------

/// Writes `ufo` to `dir` through a sibling staging folder, without ever
/// deleting anything that is not a UFO.
fn install(ufo: &norad::Font, dir: &Path) -> Result<(), InteropError> {
    // `symlink_metadata`: a symbolic link is not followed, so it is never
    // mistaken for the folder it points to.
    let replacing = match fs::symlink_metadata(dir) {
        Ok(metadata) if metadata.is_dir() && dir.join(METAINFO_FILE).is_file() => true,
        Ok(_) => return Err(InteropError::DestinationExists(dir.to_path_buf())),
        Err(error) if error.kind() == io::ErrorKind::NotFound => false,
        Err(error) => return Err(error.into()),
    };

    let staging = sibling(dir, "tmp")?;
    if let Some(parent) = dir.parent().filter(|parent| !parent.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }

    // `norad::Font::save` deletes whatever is at its target, so it only ever
    // gets the staging folder, which did not exist a moment ago.
    if let Err(error) = ufo.save(&staging) {
        discard(&staging);
        return Err(write_failure(&error));
    }
    if let Err(error) = move_into_place(&staging, dir, replacing) {
        discard(&staging);
        return Err(error.into());
    }
    Ok(())
}

/// Moves the finished UFO at `staging` to `dir`.
///
/// When `replacing`, the old UFO is first renamed out of the way and only
/// deleted once the new one is in place; if that cannot be done, the old one
/// is put back, so a failure never leaves `dir` without a UFO.
fn move_into_place(staging: &Path, dir: &Path, replacing: bool) -> io::Result<()> {
    if !replacing {
        return fs::rename(staging, dir);
    }
    let backup = sibling(dir, "old")?;
    fs::rename(dir, &backup)?;
    if let Err(error) = fs::rename(staging, dir) {
        // Best effort: if even this fails the old UFO is still intact in `backup`.
        let _ = fs::rename(&backup, dir);
        return Err(error);
    }
    // The export succeeded; a leftover backup is not worth failing it for.
    discard(&backup);
    Ok(())
}

/// A path next to `dir`, named `<dir name>.<tag>-<uuid>`.
fn sibling(dir: &Path, tag: &str) -> io::Result<PathBuf> {
    let mut name = dir
        .file_name()
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "the destination has no file name",
            )
        })?
        .to_os_string();
    name.push(format!(".{tag}-{}", Uuid::new_v4()));
    Ok(dir.with_file_name(name))
}

/// Classifies a failure of norad to write the UFO: one caused by the file
/// system (an [`io::Error`] somewhere in its chain of causes) is
/// [`InteropError::Io`], with the kind of that error and the whole
/// description; anything else is [`InteropError::Norad`].
fn write_failure(error: &(dyn std::error::Error + 'static)) -> InteropError {
    let mut cause = Some(error);
    while let Some(current) = cause {
        if let Some(io_error) = current.downcast_ref::<io::Error>() {
            return InteropError::Io(io::Error::new(io_error.kind(), describe(error)));
        }
        cause = current.source();
    }
    InteropError::Norad(describe(error))
}

/// An error with the chain of its causes: norad's messages say what failed
/// ("failed to create target font directory") and keep the reason in the source.
fn describe(error: &dyn std::error::Error) -> String {
    let mut text = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        text.push_str(": ");
        text.push_str(&cause.to_string());
        source = cause.source();
    }
    text
}

/// Removes a folder this module created, ignoring failure: it is clean-up
/// after the real outcome is already decided.
fn discard(path: &Path) {
    let _ = fs::remove_dir_all(path);
}
