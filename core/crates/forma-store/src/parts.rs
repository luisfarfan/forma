//! Splitting a font into the pieces that both formats store, and putting it
//! back together.
//!
//! Both `.forma` and `.formapackage` keep the font as one *font document*
//! (everything except the glyphs) plus one *glyph document* per glyph, each a
//! canonical JSON text. The two formats differ only in where those texts live.

use std::collections::BTreeMap;
use std::fmt::Display;
use std::sync::Arc;

use forma_model::{Font, FontId, Glyph, GlyphId, Kerning, Master};
use serde::de::Error as _;
use serde_json::Value;

use crate::StoreError;
use crate::canonical::{canonical_string, try_to_canonical_json};

/// The format version this build writes. Version 2 adds component lists in
/// glyph layers and the kerning of the font document; both are optional
/// fields, so a version 1 document is also a valid version 2 document.
pub const FORMAT_VERSION: u32 = 2;

/// The format versions this build reads. Saving always writes
/// [`FORMAT_VERSION`], which loses nothing: a migrated document means the
/// same font.
pub const READABLE_VERSIONS: [u32; 2] = [1, 2];

/// Checks that `version` is one this build can read.
pub(crate) fn ensure_readable(version: u32) -> Result<(), StoreError> {
    if READABLE_VERSIONS.contains(&version) {
        Ok(())
    } else if version > FORMAT_VERSION {
        Err(StoreError::UnsupportedVersion(version))
    } else {
        Err(StoreError::Invalid(format!(
            "{version} is not a format version"
        )))
    }
}

/// Brings a font document written by format `version` to the current format.
///
/// Pure: no I/O, and the same input gives the same output. From 1 to 2 there
/// is nothing to change, because the fields that version 2 adds
/// (`kerning`) default to empty when absent; the function is the place where
/// later versions add their steps.
///
/// # Errors
///
/// [`StoreError::UnsupportedVersion`] for a version newer than this build,
/// [`StoreError::Invalid`] for version 0.
pub fn migrate_font_document(version: u32, document: Value) -> Result<Value, StoreError> {
    ensure_readable(version)?;
    Ok(document)
}

/// Brings a glyph document written by format `version` to the current format.
/// See [`migrate_font_document`]; the `components` of a layer default to
/// empty when absent.
///
/// # Errors
///
/// As [`migrate_font_document`].
pub fn migrate_glyph_document(version: u32, document: Value) -> Result<Value, StoreError> {
    ensure_readable(version)?;
    Ok(document)
}

/// Written to the manifest of every document, for diagnostics.
pub(crate) const GENERATOR: &str = concat!("forma ", env!("CARGO_PKG_VERSION"));

/// The name of the font field that is stored as separate glyph documents.
const GLYPHS_FIELD: &str = "glyphs";

/// A font ready to be written.
pub(crate) struct Encoded {
    /// The identifier of the font.
    pub font_id: FontId,
    /// The font without its glyphs, as canonical JSON.
    pub font_json: String,
    /// The glyphs, in identifier order, each as canonical JSON.
    pub glyphs: Vec<EncodedGlyph>,
}

/// One glyph ready to be written.
pub(crate) struct EncodedGlyph {
    pub id: GlyphId,
    pub name: String,
    pub json: String,
}

/// Wraps a message that describes where a document went wrong into the error
/// type for unreadable JSON, keeping the position information of the cause.
pub(crate) fn json_error_in(context: impl Display, cause: &dyn Display) -> StoreError {
    StoreError::Json(serde_json::Error::custom(format!("{context}: {cause}")))
}

/// Reports a violation of the data model rules.
fn invalid(cause: impl Display) -> StoreError {
    StoreError::Invalid(cause.to_string())
}

/// Checks that nothing in the font would be changed by a trip through JSON.
///
/// JSON cannot hold NaN or infinity; `serde_json` writes them as `null`,
/// which would not load back. Refusing them at save time keeps every saved
/// document loadable.
fn ensure_finite(font: &Font) -> Result<(), StoreError> {
    ensure_masters_finite(&font.masters)?;
    ensure_kerning_finite(&font.kerning)?;
    font.glyphs
        .values()
        .try_for_each(|glyph| ensure_glyph_finite(glyph))
}

/// The master half of [`ensure_finite`], also used for journal records.
pub(crate) fn ensure_masters_finite(masters: &[Master]) -> Result<(), StoreError> {
    let metrics = masters.iter().flat_map(|master| {
        let metrics = &master.metrics;
        [
            metrics.ascender,
            metrics.descender,
            metrics.x_height,
            metrics.cap_height,
        ]
    });
    if metrics.into_iter().any(|value| !value.is_finite()) {
        return Err(StoreError::Invalid(
            "a master has a vertical metric that is not a finite number".to_owned(),
        ));
    }
    Ok(())
}

/// The glyph half of [`ensure_finite`], also used for journal records.
pub(crate) fn ensure_glyph_finite(glyph: &Glyph) -> Result<(), StoreError> {
    for layer in glyph.layers.values() {
        let finite = layer.advance_width.is_finite()
            && layer
                .paths
                .iter()
                .flat_map(|path| &path.nodes)
                .all(|node| node.x.is_finite() && node.y.is_finite())
            && layer.components.iter().all(|component| {
                let t = &component.transform;
                [t.xx, t.xy, t.yx, t.yy, t.dx, t.dy]
                    .iter()
                    .all(|value| value.is_finite())
            });
        if !finite {
            return Err(StoreError::Invalid(format!(
                "glyph {:?} has an advance width, coordinate or component transform that is \
                 not a finite number",
                glyph.name
            )));
        }
    }
    Ok(())
}

/// The kerning part of [`ensure_finite`], also used for journal records.
pub(crate) fn ensure_kerning_finite(kerning: &Kerning) -> Result<(), StoreError> {
    let finite = kerning
        .pairs
        .values()
        .flat_map(|pairs| pairs.values())
        .all(|value| value.is_finite());
    if finite {
        Ok(())
    } else {
        Err(StoreError::Invalid(
            "a kerning pair has a value that is not a finite number".to_owned(),
        ))
    }
}

/// Validates `font` and renders it as canonical JSON documents.
///
/// A font that [`Font::validate_invariants`] rejects is not encoded: what
/// cannot be loaded back must not be written.
pub(crate) fn encode(font: &Font) -> Result<Encoded, StoreError> {
    font.validate_invariants().map_err(invalid)?;
    ensure_finite(font)?;

    // Serialize the font with an empty glyph map (a cheap clone: the glyphs
    // are shared) rather than converting every glyph just to drop it again.
    let mut shell = font.clone();
    shell.glyphs = Arc::new(BTreeMap::new());
    let mut document = serde_json::to_value(&shell)?;
    if let Value::Object(fields) = &mut document {
        fields.remove(GLYPHS_FIELD);
    }
    let font_json = canonical_string(&document)?;

    let glyphs = font
        .glyphs
        .iter()
        .map(|(&id, glyph)| {
            Ok(EncodedGlyph {
                id,
                name: glyph.name.clone(),
                json: try_to_canonical_json(&**glyph)?,
            })
        })
        .collect::<Result<Vec<_>, StoreError>>()?;

    Ok(Encoded {
        font_id: font.id,
        font_json,
        glyphs,
    })
}

/// Reads a font document: a font whose glyph map is still empty.
///
/// `context` names the document in error messages.
pub(crate) fn decode_font_document(
    bytes: &[u8],
    context: &str,
    version: u32,
) -> Result<Font, StoreError> {
    let document: Value =
        serde_json::from_slice(bytes).map_err(|err| json_error_in(context, &err))?;
    let mut document = migrate_font_document(version, document)?;
    let Value::Object(fields) = &mut document else {
        return Err(StoreError::Invalid(format!("{context}: not a JSON object")));
    };
    if fields.contains_key(GLYPHS_FIELD) {
        return Err(StoreError::Invalid(format!(
            "{context}: the font document must not contain glyphs"
        )));
    }
    // The glyphs are stored apart; the model's own deserializer fills in the
    // rest, so fields added to the model are picked up automatically.
    fields.insert(
        GLYPHS_FIELD.to_owned(),
        Value::Object(serde_json::Map::new()),
    );
    serde_json::from_value(document).map_err(|err| json_error_in(context, &err))
}

/// Reads a glyph document.
pub(crate) fn decode_glyph_document(
    bytes: &[u8],
    context: &str,
    version: u32,
) -> Result<Glyph, StoreError> {
    let document: Value =
        serde_json::from_slice(bytes).map_err(|err| json_error_in(context, &err))?;
    let document = migrate_glyph_document(version, document)?;
    serde_json::from_value(document).map_err(|err| json_error_in(context, &err))
}

/// Puts a font back together from its font document and its glyphs, and
/// checks the result against the data model rules.
///
/// Glyphs are keyed by the id inside their document, never by where they were
/// stored. Two glyphs with one id are an error.
pub(crate) fn assemble(
    mut font: Font,
    glyphs: impl IntoIterator<Item = Glyph>,
) -> Result<Font, StoreError> {
    let mut map: BTreeMap<GlyphId, Arc<Glyph>> = BTreeMap::new();
    for glyph in glyphs {
        if let Some(first) = map.get(&glyph.id) {
            return Err(StoreError::Invalid(format!(
                "glyph id {} is used by more than one glyph ({:?} and {:?})",
                glyph.id, first.name, glyph.name
            )));
        }
        map.insert(glyph.id, Arc::new(glyph));
    }
    font.glyphs = Arc::new(map);
    font.validate_invariants().map_err(invalid)?;
    Ok(font)
}
