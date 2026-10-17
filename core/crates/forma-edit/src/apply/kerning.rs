//! Kerning pairs and kerning groups.

use std::collections::{BTreeSet, HashSet};

use forma_model::{
    EntityKey, Font, GlyphId, KernRef, KernSide, KerningGroup, KerningGroupId, MasterId,
    is_valid_glyph_name,
};

use super::{Touched, resolve_glyph, resolve_master};
use crate::{EditError, GlyphRef, KernRefInput};

/// What one side of a pair refers to; see [`KernRefInput::resolve`].
fn resolve_side(font: &Font, input: &KernRefInput, side: KernSide) -> Result<KernRef, EditError> {
    match input {
        KernRefInput::Glyph(glyph) => resolve_glyph(font, glyph).map(KernRef::Glyph),
        KernRefInput::Group(name) => input
            .resolve(font, side)
            .ok_or_else(|| EditError::NotFound(format!("{side}-side kerning group '{name}'"))),
    }
}

fn glyph_name(font: &Font, id: GlyphId) -> String {
    font.glyphs
        .get(&id)
        .map_or_else(|| id.to_string(), |glyph| glyph.name.clone())
}

pub(super) fn set_kerning_pair(
    font: &mut Font,
    master: Option<MasterId>,
    first: &KernRefInput,
    second: &KernRefInput,
    value: f64,
) -> Result<Touched, EditError> {
    let master = resolve_master(font, master)?;
    let first = resolve_side(font, first, KernSide::First)?;
    let second = resolve_side(font, second, KernSide::Second)?;
    let limit = 2.0 * f64::from(font.units_per_em);
    if !value.is_finite() || value.abs() > limit {
        return Err(EditError::InvalidArgument(format!(
            "the kerning value must be finite and at most {limit} (twice the units per em) in \
             absolute value, got {value}"
        )));
    }
    // A pair with value 0 is no pair.
    let value = (value != 0.0).then_some(value);
    if font.kerning.pair(master, first, second) == value {
        return Ok(Touched::new());
    }
    font.kerning.set_pair(master, first, second, value);
    Ok(BTreeSet::from([EntityKey::KerningPair(
        master, first, second,
    )]))
}

pub(super) fn set_kerning_group(
    font: &mut Font,
    name: &str,
    side: KernSide,
    members: &[GlyphRef],
) -> Result<Touched, EditError> {
    if !is_valid_glyph_name(name) {
        return Err(EditError::InvalidArgument(format!(
            "invalid kerning group name {name:?}: group names follow the glyph name rules (1 to \
             63 printable ASCII characters without spaces, not starting with a digit or a \
             period)"
        )));
    }
    let existing = font.kerning.group_by_name(side, name).map(|group| group.id);
    let mut ids = Vec::with_capacity(members.len());
    let mut seen = HashSet::new();
    for member in members {
        let id = resolve_glyph(font, member)?;
        if !seen.insert(id) {
            return Err(EditError::InvalidArgument(format!(
                "glyph '{}' is listed more than once",
                glyph_name(font, id)
            )));
        }
        if let Some(other) = font.kerning.group_of(id, side)
            && Some(other) != existing
        {
            let other = font
                .kerning
                .groups
                .get(&other)
                .map_or_else(|| other.to_string(), |group| group.name.clone());
            return Err(EditError::InvalidArgument(format!(
                "glyph '{}' already belongs to the {side}-side kerning group '{other}'; a glyph \
                 belongs to at most one group per side",
                glyph_name(font, id)
            )));
        }
        ids.push(id);
    }

    let id = match existing {
        Some(id) => {
            let group = font.kerning.groups_mut().get_mut(&id).ok_or_else(|| {
                EditError::NotFound(format!("{side}-side kerning group '{name}'"))
            })?;
            if group.members == ids {
                return Ok(Touched::new());
            }
            group.members = ids;
            id
        }
        None => {
            let id = KerningGroupId::new();
            font.kerning.groups_mut().insert(
                id,
                KerningGroup {
                    id,
                    name: name.to_owned(),
                    side,
                    members: ids,
                },
            );
            id
        }
    };
    Ok(BTreeSet::from([EntityKey::KerningGroup(id)]))
}

pub(super) fn delete_kerning_group(
    font: &mut Font,
    name: &str,
    side: KernSide,
) -> Result<Touched, EditError> {
    let id = font
        .kerning
        .group_by_name(side, name)
        .map(|group| group.id)
        .ok_or_else(|| EditError::NotFound(format!("{side}-side kerning group '{name}'")))?;
    font.kerning.groups_mut().remove(&id);
    let mut touched = BTreeSet::from([EntityKey::KerningGroup(id)]);
    touched.extend(
        font.kerning
            .remove_pairs_with(KernRef::Group(id))
            .into_iter()
            .map(|(master, first, second)| EntityKey::KerningPair(master, first, second)),
    );
    Ok(touched)
}
