//! Font-level information and vertical metrics.

use std::collections::BTreeSet;

use forma_model::{EntityKey, Font, MAX_UNITS_PER_EM, MIN_UNITS_PER_EM, MasterId};

use super::{Touched, resolve_master};
use crate::EditError;

/// The fields of a `SetFontInfo` command.
pub(super) struct InfoUpdate<'a> {
    pub family_name: Option<&'a str>,
    pub style_name: Option<&'a str>,
    pub version_major: Option<u16>,
    pub version_minor: Option<u16>,
    pub copyright: Option<&'a str>,
    pub designer: Option<&'a str>,
    pub units_per_em: Option<u16>,
}

fn non_empty(what: &str, value: &str) -> Result<(), EditError> {
    if value.trim().is_empty() {
        Err(EditError::InvalidArgument(format!(
            "{what} must not be empty"
        )))
    } else {
        Ok(())
    }
}

/// An optional text field: the empty string removes it.
fn optional_text(value: &str) -> Option<String> {
    (!value.is_empty()).then(|| value.to_owned())
}

pub(super) fn set_font_info(
    font: &mut Font,
    update: &InfoUpdate<'_>,
) -> Result<Touched, EditError> {
    let nothing = update.family_name.is_none()
        && update.style_name.is_none()
        && update.version_major.is_none()
        && update.version_minor.is_none()
        && update.copyright.is_none()
        && update.designer.is_none()
        && update.units_per_em.is_none();
    if nothing {
        return Err(EditError::InvalidArgument(
            "nothing to set: give at least one font info field".into(),
        ));
    }
    if let Some(name) = update.family_name {
        non_empty("family_name", name)?;
    }
    if let Some(name) = update.style_name {
        non_empty("style_name", name)?;
    }
    if let Some(units) = update.units_per_em
        && !(MIN_UNITS_PER_EM..=MAX_UNITS_PER_EM).contains(&units)
    {
        return Err(EditError::InvalidArgument(format!(
            "units_per_em {units} is outside {MIN_UNITS_PER_EM}..={MAX_UNITS_PER_EM}"
        )));
    }

    if let Some(name) = update.family_name {
        name.clone_into(&mut font.info.family_name);
    }
    if let Some(name) = update.style_name {
        name.clone_into(&mut font.info.style_name);
    }
    if let Some(major) = update.version_major {
        font.info.version_major = major;
    }
    if let Some(minor) = update.version_minor {
        font.info.version_minor = minor;
    }
    if let Some(text) = update.copyright {
        font.info.copyright = optional_text(text);
    }
    if let Some(text) = update.designer {
        font.info.designer = optional_text(text);
    }
    if let Some(units) = update.units_per_em {
        font.units_per_em = units;
    }
    Ok(BTreeSet::from([EntityKey::FontInfo]))
}

/// Sets vertical metrics given as `[ascender, descender, x_height, cap_height]`.
pub(super) fn set_vertical_metrics(
    font: &mut Font,
    master: Option<MasterId>,
    values: [Option<f64>; 4],
) -> Result<Touched, EditError> {
    let master = resolve_master(font, master)?;
    if values.iter().all(Option::is_none) {
        return Err(EditError::InvalidArgument(
            "nothing to set: give at least one vertical metric".into(),
        ));
    }
    let names = ["ascender", "descender", "x_height", "cap_height"];
    for (name, value) in names.iter().zip(values) {
        if value.is_some_and(|v| !v.is_finite()) {
            return Err(EditError::InvalidArgument(format!("{name} must be finite")));
        }
    }
    let target = font
        .masters
        .iter_mut()
        .find(|candidate| candidate.id == master)
        .ok_or_else(|| EditError::NotFound(format!("master '{master}'")))?;
    let [ascender, descender, x_height, cap_height] = values;
    let metrics = &mut target.metrics;
    metrics.ascender = ascender.unwrap_or(metrics.ascender);
    metrics.descender = descender.unwrap_or(metrics.descender);
    metrics.x_height = x_height.unwrap_or(metrics.x_height);
    metrics.cap_height = cap_height.unwrap_or(metrics.cap_height);
    Ok(BTreeSet::from([EntityKey::VerticalMetrics(master)]))
}
