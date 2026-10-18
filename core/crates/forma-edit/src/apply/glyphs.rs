//! Creating, renaming and deleting glyphs.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;

use forma_model::{
    EntityKey, Font, Glyph, GlyphId, KernRef, Layer, Path, Provenance, ProvenanceDetail,
    is_valid_glyph_name,
};

use super::{
    Touched, check_advance_width, checked_paths, glyph_mut, resolve_glyph, resolve_targets,
};
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
            provenance: None,
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
    let listed = resolve_targets(font, glyphs, "delete")?;
    let ids: BTreeSet<GlyphId> = listed.iter().copied().collect();
    check_not_used(font, &listed, &ids)?;

    Arc::make_mut(&mut font.glyphs).retain(|id, _| !ids.contains(id));
    font.glyph_order.retain(|id| !ids.contains(id));
    let mut touched: Touched = ids.iter().copied().map(EntityKey::Glyph).collect();
    touched.insert(EntityKey::GlyphOrder);
    // The deleted glyphs leave their kerning groups, and their pairs go.
    for &id in &ids {
        touched.extend(
            font.kerning
                .remove_from_groups(id)
                .into_iter()
                .map(EntityKey::KerningGroup),
        );
        touched.extend(
            font.kerning
                .remove_pairs_with(KernRef::Glyph(id))
                .into_iter()
                .map(|(master, first, second)| EntityKey::KerningPair(master, first, second)),
        );
    }
    Ok(touched)
}

/// Checks that no glyph that stays uses one of the glyphs `deleted` (listed
/// in the command as `listed`) as a component, naming the first such glyph
/// in `listed` order and its users in glyph order.
fn check_not_used(
    font: &Font,
    listed: &[GlyphId],
    deleted: &BTreeSet<GlyphId>,
) -> Result<(), EditError> {
    // One pass over the glyphs that stay: the users of each deleted glyph.
    let mut users: HashMap<GlyphId, Vec<&str>> = HashMap::new();
    for id in &font.glyph_order {
        if deleted.contains(id) {
            continue;
        }
        let Some(glyph) = font.glyphs.get(id) else {
            continue;
        };
        let mut bases: Vec<GlyphId> = glyph
            .layers
            .values()
            .flat_map(|layer| &layer.components)
            .map(|component| component.base)
            .filter(|base| deleted.contains(base))
            .collect();
        bases.sort_unstable();
        bases.dedup();
        for base in bases {
            users.entry(base).or_default().push(&glyph.name);
        }
    }
    for id in listed {
        if let Some(names) = users.get(id) {
            let name = font
                .glyphs
                .get(id)
                .map_or_else(|| id.to_string(), |glyph| glyph.name.clone());
            return Err(EditError::InvalidArgument(format!(
                "glyph '{name}' is used as a component by: {}",
                names.join(", ")
            )));
        }
    }
    Ok(())
}

/// Sets or clears the provenance of a glyph; see
/// [`Command::SetProvenance`](crate::Command::SetProvenance).
pub(super) fn set_provenance(
    font: &mut Font,
    glyph: &GlyphRef,
    provenance: Option<&Provenance>,
) -> Result<Touched, EditError> {
    let id = resolve_glyph(font, glyph)?;
    if let Some(provenance) = provenance {
        check_provenance(provenance)?;
    }
    if font.glyphs.get(&id).and_then(|g| g.provenance.as_ref()) == provenance {
        return Ok(Touched::new());
    }
    glyph_mut(font, id)?.provenance = provenance.cloned();
    Ok(BTreeSet::from([EntityKey::Glyph(id)]))
}

fn check_provenance(provenance: &Provenance) -> Result<(), EditError> {
    if !provenance
        .detail
        .numbers()
        .iter()
        .all(|value| value.is_finite())
    {
        return Err(EditError::InvalidArgument(
            "provenance measurements must be finite numbers".into(),
        ));
    }
    let unit = |value: f64, what: &str| {
        if (0.0..=1.0).contains(&value) {
            Ok(())
        } else {
            Err(EditError::InvalidArgument(format!(
                "{what} must be within 0 to 1, got {value}"
            )))
        }
    };
    match &provenance.detail {
        ProvenanceDetail::Traced { iou_mask, .. } => unit(*iou_mask, "iou_mask"),
        ProvenanceDetail::Synthesized { style_score, .. } => unit(*style_score, "style_score"),
        _ => Ok(()),
    }
}
