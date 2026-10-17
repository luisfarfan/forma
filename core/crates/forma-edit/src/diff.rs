//! Comparing two states of a font.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::sync::Arc;

use forma_model::{EntityKey, Font, Glyph, GlyphId, MasterId, VerticalMetrics};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// How one glyph present in both fonts differs between them.
///
/// A glyph whose only change is its name or its export flag is listed with
/// every flag false.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GlyphDiff {
    /// The glyph name in the second font.
    pub name: String,
    /// Whether the contours changed in any master (a layer present in only
    /// one of the fonts counts as a change).
    pub outline_changed: bool,
    /// Whether the advance width changed in any master (a layer present in
    /// only one of the fonts counts as a change).
    pub advance_changed: bool,
    /// Whether the code points changed.
    pub unicodes_changed: bool,
}

/// The differences between two states of a font.
///
/// Glyphs are matched by their stable id, so a renamed glyph is modified,
/// not removed and added again.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FontDiff {
    /// Names of the glyphs only in the second font, in its glyph order.
    pub added: Vec<String>,
    /// Names of the glyphs only in the first font, in its glyph order.
    pub removed: Vec<String>,
    /// The glyphs present in both fonts that differ in any way, in the glyph
    /// order of the second font.
    pub modified: Vec<GlyphDiff>,
    /// Whether the font info or the units per em changed.
    pub info_changed: bool,
    /// Whether the vertical metrics of any master changed.
    pub metrics_changed: bool,
}

/// Compares two states of a font, `a` before and `b` after.
#[must_use]
pub fn diff(a: &Font, b: &Font) -> FontDiff {
    let mut added = Vec::new();
    let mut modified = Vec::new();
    for id in ordered_ids(b) {
        let Some(after) = b.glyphs.get(&id) else {
            continue;
        };
        match a.glyphs.get(&id) {
            None => added.push(after.name.clone()),
            Some(before) if !same_glyph(before, after) => {
                modified.push(glyph_diff(before, after));
            }
            Some(_) => {}
        }
    }
    let removed = ordered_ids(a)
        .into_iter()
        .filter(|id| !b.glyphs.contains_key(id))
        .filter_map(|id| a.glyphs.get(&id).map(|glyph| glyph.name.clone()))
        .collect();
    FontDiff {
        added,
        removed,
        modified,
        info_changed: info_changed(a, b),
        metrics_changed: metrics_by_master(a) != metrics_by_master(b),
    }
}

/// The entities whose values differ between two fonts.
pub(crate) fn changed_entities(a: &Font, b: &Font) -> BTreeSet<EntityKey> {
    let mut keys = BTreeSet::new();
    if info_changed(a, b) {
        keys.insert(EntityKey::FontInfo);
    }
    let (metrics_a, metrics_b) = (metrics_by_master(a), metrics_by_master(b));
    for master in metrics_a.keys().chain(metrics_b.keys()) {
        if metrics_a.get(master) != metrics_b.get(master) {
            keys.insert(EntityKey::VerticalMetrics(*master));
        }
    }
    if a.glyph_order != b.glyph_order {
        keys.insert(EntityKey::GlyphOrder);
    }
    if !Arc::ptr_eq(&a.glyphs, &b.glyphs) {
        for (id, before) in a.glyphs.iter() {
            let unchanged = b
                .glyphs
                .get(id)
                .is_some_and(|after| same_glyph(before, after));
            if !unchanged {
                keys.insert(EntityKey::Glyph(*id));
            }
        }
        for id in b.glyphs.keys() {
            if !a.glyphs.contains_key(id) {
                keys.insert(EntityKey::Glyph(*id));
            }
        }
    }
    keys
}

fn same_glyph(a: &Arc<Glyph>, b: &Arc<Glyph>) -> bool {
    Arc::ptr_eq(a, b) || a == b
}

fn info_changed(a: &Font, b: &Font) -> bool {
    a.info != b.info || a.units_per_em != b.units_per_em
}

fn metrics_by_master(font: &Font) -> BTreeMap<MasterId, &VerticalMetrics> {
    font.masters
        .iter()
        .map(|master| (master.id, &master.metrics))
        .collect()
}

fn glyph_diff(before: &Glyph, after: &Glyph) -> GlyphDiff {
    let mut outline_changed = false;
    let mut advance_changed = false;
    for master in before.layers.keys().chain(after.layers.keys()) {
        match (before.layers.get(master), after.layers.get(master)) {
            (Some(x), Some(y)) => {
                outline_changed |= x.paths != y.paths;
                advance_changed |= x.advance_width != y.advance_width;
            }
            _ => {
                outline_changed = true;
                advance_changed = true;
            }
        }
    }
    GlyphDiff {
        name: after.name.clone(),
        outline_changed,
        advance_changed,
        unicodes_changed: before.unicodes != after.unicodes,
    }
}

/// The glyph ids of a font in glyph order, followed by any glyph the order
/// does not list (none in a valid font), so no glyph is ever skipped.
fn ordered_ids(font: &Font) -> Vec<GlyphId> {
    let mut seen = HashSet::new();
    let mut ids: Vec<GlyphId> = font
        .glyph_order
        .iter()
        .copied()
        .filter(|id| font.glyphs.contains_key(id) && seen.insert(*id))
        .collect();
    ids.extend(font.glyphs.keys().filter(|id| !seen.contains(*id)));
    ids
}
