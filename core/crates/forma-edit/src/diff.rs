//! Comparing two states of a font.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::sync::Arc;

use forma_model::{EntityKey, Font, Glyph, GlyphId, Layer, MasterId, VerticalMetrics};
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
    /// Whether the components changed in any master (a layer present in only
    /// one of the fonts counts as a change when it has components).
    pub components_changed: bool,
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
    /// Whether any kerning group or kerning pair changed.
    pub kerning_changed: bool,
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
        kerning_changed: !kerning_changes(a, b).is_empty(),
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
    keys.extend(kerning_changes(a, b));
    keys
}

/// The kerning groups and pairs whose values differ between two fonts: a
/// group or pair present in only one of them, or with a different value. A
/// master stored without pairs is the same as a master with no pair list.
fn kerning_changes(a: &Font, b: &Font) -> BTreeSet<EntityKey> {
    let mut keys = BTreeSet::new();
    let (a, b) = (&a.kerning, &b.kerning);
    if a == b {
        return keys;
    }
    for id in a.groups.keys().chain(b.groups.keys()) {
        if a.groups.get(id) != b.groups.get(id) {
            keys.insert(EntityKey::KerningGroup(*id));
        }
    }
    let empty = BTreeMap::new();
    for master in a.pairs.keys().chain(b.pairs.keys()) {
        let before = a.pairs.get(master).unwrap_or(&empty);
        let after = b.pairs.get(master).unwrap_or(&empty);
        if before == after {
            continue;
        }
        for (first, second) in before.keys().chain(after.keys()) {
            if before.get(&(*first, *second)) != after.get(&(*first, *second)) {
                keys.insert(EntityKey::KerningPair(*master, *first, *second));
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
    let mut components_changed = false;
    for master in before.layers.keys().chain(after.layers.keys()) {
        match (before.layers.get(master), after.layers.get(master)) {
            (Some(x), Some(y)) => {
                outline_changed |= x.paths != y.paths;
                advance_changed |= x.advance_width != y.advance_width;
                components_changed |= x.components != y.components;
            }
            (x, y) => {
                outline_changed = true;
                advance_changed = true;
                let lone = x
                    .or(y)
                    .is_some_and(|layer: &Layer| !layer.components.is_empty());
                components_changed |= lone;
            }
        }
    }
    GlyphDiff {
        name: after.name.clone(),
        outline_changed,
        advance_changed,
        unicodes_changed: before.unicodes != after.unicodes,
        components_changed,
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

#[cfg(test)]
mod tests {
    use forma_model::{KernRef, KernSide, KerningGroup, KerningGroupId, MasterId};

    use super::*;

    fn gid(n: u128) -> GlyphId {
        format!("00000000-0000-0000-0000-{n:012x}")
            .parse()
            .expect("an id")
    }

    fn kid(n: u128) -> KerningGroupId {
        format!("00000000-0000-0000-0000-{n:012x}")
            .parse()
            .expect("an id")
    }

    fn group(n: u128, name: &str, members: &[GlyphId]) -> KerningGroup {
        KerningGroup {
            id: kid(n),
            name: name.to_owned(),
            side: KernSide::First,
            members: members.to_vec(),
        }
    }

    fn set_pair(font: &mut Font, master: MasterId, first: KernRef, second: KernRef, value: f64) {
        font.kerning
            .pairs
            .entry(master)
            .or_default()
            .insert((first, second), value);
    }

    #[test]
    fn changed_entities_lists_kerning_groups_and_pairs() {
        let mut a = Font::new("Kern", 1000);
        let master = a.masters[0].id;
        let other = MasterId::new();
        let (g1, g2, g3) = (
            KernRef::Glyph(gid(1)),
            KernRef::Glyph(gid(2)),
            KernRef::Glyph(gid(3)),
        );
        for n in [1, 2, 3] {
            a.kerning.groups.insert(kid(n), group(n, "G", &[]));
        }
        set_pair(&mut a, master, g1, g2, -10.0); // changed
        set_pair(&mut a, master, g1, g3, -20.0); // removed
        set_pair(&mut a, master, g2, g3, -30.0); // kept
        set_pair(&mut a, other, g3, g1, -40.0); // its whole master goes
        let mut b = a.clone();
        b.kerning.groups.remove(&kid(1));
        b.kerning.groups.insert(kid(2), group(2, "G", &[gid(1)]));
        b.kerning.groups.insert(kid(4), group(4, "H", &[]));
        let pairs = b.kerning.pairs.get_mut(&master).expect("pairs");
        pairs.insert((g1, g2), -15.0);
        pairs.remove(&(g1, g3));
        pairs.insert((g3, g2), -50.0); // added
        b.kerning.pairs.remove(&other);

        let expected = BTreeSet::from([
            EntityKey::KerningGroup(kid(1)),
            EntityKey::KerningGroup(kid(2)),
            EntityKey::KerningGroup(kid(4)),
            EntityKey::KerningPair(master, g1, g2),
            EntityKey::KerningPair(master, g1, g3),
            EntityKey::KerningPair(master, g3, g2),
            EntityKey::KerningPair(other, g3, g1),
        ]);
        assert_eq!(changed_entities(&a, &b), expected);
        assert_eq!(changed_entities(&b, &a), expected);
        assert!(diff(&a, &b).kerning_changed);
        assert!(changed_entities(&a, &a.clone()).is_empty());

        // A master stored with an empty pair list holds the same pairs as no
        // list at all.
        let mut c = b.clone();
        c.kerning.pairs.insert(other, BTreeMap::new());
        assert!(changed_entities(&b, &c).is_empty());
        assert!(!diff(&b, &c).kerning_changed);
    }
}
