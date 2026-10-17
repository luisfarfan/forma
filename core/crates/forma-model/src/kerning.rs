//! Kerning: groups of glyphs that kern alike, and the kerning pairs of each
//! master.

use std::borrow::Cow;
use std::collections::{BTreeMap, HashSet};
use std::fmt;
use std::str::FromStr;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::de::Error as _;
use serde::ser::SerializeStruct;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::font::{Font, ModelError};
use crate::glyph::is_valid_glyph_name;
use crate::ids::{GlyphId, KerningGroupId, MasterId, parse_canonical_uuid};

/// The side of a kerning pair a group can stand on.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum KernSide {
    /// The left side of a pair in a left-to-right script (UFO `kern1`).
    First,
    /// The right side of a pair in a left-to-right script (UFO `kern2`).
    Second,
}

impl fmt::Display for KernSide {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::First => "first",
            Self::Second => "second",
        })
    }
}

/// One side of a kerning pair: a single glyph or a kerning group.
///
/// Its text form, which is also its serde representation, is
/// `g.<glyph uuid>` or `k.<group uuid>`, with the UUID lowercase and
/// hyphenated. Glyph references sort before group references.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum KernRef {
    /// One glyph.
    Glyph(GlyphId),
    /// Every member of a kerning group.
    Group(KerningGroupId),
}

impl fmt::Display for KernRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Glyph(glyph) => write!(f, "g.{glyph}"),
            Self::Group(group) => write!(f, "k.{group}"),
        }
    }
}

impl FromStr for KernRef {
    type Err = ModelError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let parsed = if let Some(rest) = s.strip_prefix("g.") {
            parse_canonical_uuid(rest).map(|uuid| Self::Glyph(GlyphId(uuid)))
        } else if let Some(rest) = s.strip_prefix("k.") {
            parse_canonical_uuid(rest).map(|uuid| Self::Group(KerningGroupId(uuid)))
        } else {
            None
        };
        parsed.ok_or_else(|| ModelError::InvalidKernRef(s.to_owned()))
    }
}

impl Serialize for KernRef {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for KernRef {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        text.parse().map_err(D::Error::custom)
    }
}

impl JsonSchema for KernRef {
    fn schema_name() -> Cow<'static, str> {
        "KernRef".into()
    }

    fn json_schema(_generator: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "string",
            "description": "One side of a kerning pair: `g.<glyph uuid>` for a glyph or \
                            `k.<group uuid>` for a kerning group.",
            "pattern": "^[gk]\\.[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$"
        })
    }
}

/// A kerning group: glyphs that kern alike on one side of a pair.
///
/// A glyph belongs to at most one group per side. Names follow the glyph name
/// rules and are unique per side; exported to UFO they become
/// `public.kern1.<name>` (first side) or `public.kern2.<name>` (second side).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct KerningGroup {
    /// Stable identifier of the group.
    pub id: KerningGroupId,
    /// The group name, unique on its side.
    pub name: String,
    /// The side of a pair the group stands on.
    pub side: KernSide,
    /// The glyphs of the group.
    pub members: Vec<GlyphId>,
}

/// One kerning pair of a master, as it is serialized.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct KerningPair {
    /// The left side (in a left-to-right script).
    pub first: KernRef,
    /// The right side (in a left-to-right script).
    pub second: KernRef,
    /// The adjustment in font units: negative brings the sides closer. Never
    /// 0 (a pair with value 0 is no pair).
    pub value: f64,
}

/// The kerning of a font: its groups and the pairs of each master.
///
/// Serialized as
/// `{"groups": [group...], "pairs": {"<master uuid>": [pair...]}}`, with the
/// groups sorted by id and the pairs of each master sorted by
/// `(first, second)`, so that equal kerning always has the same JSON.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Kerning {
    /// The groups, keyed by their identifier.
    pub groups: BTreeMap<KerningGroupId, KerningGroup>,
    /// The pairs of each master: `(first, second)` to value.
    pub pairs: BTreeMap<MasterId, BTreeMap<(KernRef, KernRef), f64>>,
}

impl Kerning {
    /// Whether there are no groups and no pair lists. A master stored with an
    /// empty list of pairs makes the kerning non-empty, so that it survives a
    /// round trip through JSON unchanged.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.groups.is_empty() && self.pairs.is_empty()
    }

    /// The value of the pair `(first, second)` in `master`, if there is one.
    #[must_use]
    pub fn pair(&self, master: MasterId, first: KernRef, second: KernRef) -> Option<f64> {
        self.pairs.get(&master)?.get(&(first, second)).copied()
    }

    /// The group of `side` that `glyph` belongs to, if any.
    #[must_use]
    pub fn group_of(&self, glyph: GlyphId, side: KernSide) -> Option<KerningGroupId> {
        self.groups
            .values()
            .find(|group| group.side == side && group.members.contains(&glyph))
            .map(|group| group.id)
    }

    /// The group of `side` named `name`, if any.
    #[must_use]
    pub fn group_by_name(&self, side: KernSide, name: &str) -> Option<&KerningGroup> {
        self.groups
            .values()
            .find(|group| group.side == side && group.name == name)
    }

    /// The kerning between the glyphs `left` and `right` in `master`, looked
    /// up in the UFO order: the glyph-glyph pair, then glyph-group (the
    /// second-side group of `right`), then group-glyph (the first-side group
    /// of `left`), then group-group. 0 when there is no such pair.
    #[must_use]
    pub fn effective(&self, master: MasterId, left: GlyphId, right: GlyphId) -> f64 {
        let Some(pairs) = self.pairs.get(&master) else {
            return 0.0;
        };
        let (left_glyph, right_glyph) = (KernRef::Glyph(left), KernRef::Glyph(right));
        let left_group = self.group_of(left, KernSide::First).map(KernRef::Group);
        let right_group = self.group_of(right, KernSide::Second).map(KernRef::Group);
        [
            Some((left_glyph, right_glyph)),
            right_group.map(|group| (left_glyph, group)),
            left_group.map(|group| (group, right_glyph)),
            left_group.zip(right_group),
        ]
        .into_iter()
        .flatten()
        .find_map(|key| pairs.get(&key).copied())
        .unwrap_or(0.0)
    }

    /// Checks the kerning rules of `font` (see
    /// [`Font::validate_invariants`]), reporting the first violation: groups
    /// first, in id order, then the pairs of each master.
    pub(crate) fn check(&self, font: &Font) -> Result<(), ModelError> {
        let glyph_name = |id: GlyphId| {
            font.glyphs
                .get(&id)
                .map_or_else(|| id.to_string(), |glyph| glyph.name.clone())
        };
        let mut names: HashSet<(KernSide, &str)> = HashSet::new();
        let mut members: HashSet<(KernSide, GlyphId)> = HashSet::new();
        for (&key, group) in &self.groups {
            if key != group.id {
                return Err(ModelError::GroupKeyMismatch { key, id: group.id });
            }
            if !is_valid_glyph_name(&group.name) {
                return Err(ModelError::InvalidGroupName(group.name.clone()));
            }
            if !names.insert((group.side, &group.name)) {
                return Err(ModelError::DuplicateGroupName {
                    side: group.side,
                    name: group.name.clone(),
                });
            }
            for &member in &group.members {
                if !font.glyphs.contains_key(&member) {
                    return Err(ModelError::KerningUnknownGlyph(member));
                }
                if !members.insert((group.side, member)) {
                    return Err(ModelError::GlyphInTwoGroups {
                        glyph: glyph_name(member),
                        side: group.side,
                    });
                }
            }
        }

        let masters: HashSet<MasterId> = font.masters.iter().map(|master| master.id).collect();
        let limit = 2.0 * f64::from(font.units_per_em);
        for (&master, pairs) in &self.pairs {
            if !masters.contains(&master) {
                return Err(ModelError::KerningUnknownMaster(master));
            }
            for (&(first, second), &value) in pairs {
                self.check_ref(font, first, KernSide::First)?;
                self.check_ref(font, second, KernSide::Second)?;
                if !value.is_finite() || value == 0.0 || value.abs() > limit {
                    return Err(ModelError::InvalidKerningValue { value });
                }
            }
        }
        Ok(())
    }

    /// Checks that `reference` exists and, for a group, stands on `side`.
    fn check_ref(&self, font: &Font, reference: KernRef, side: KernSide) -> Result<(), ModelError> {
        match reference {
            KernRef::Glyph(id) if font.glyphs.contains_key(&id) => Ok(()),
            KernRef::Glyph(id) => Err(ModelError::KerningUnknownGlyph(id)),
            KernRef::Group(id) => match self.groups.get(&id) {
                None => Err(ModelError::KerningUnknownGroup(id)),
                Some(group) if group.side != side => Err(ModelError::KerningGroupWrongSide {
                    group: group.name.clone(),
                }),
                Some(_) => Ok(()),
            },
        }
    }
}

// ------------------------------------------------------------------- serde

// The serialized form of `Kerning`, for reading and for the schema; its doc
// comments are the schema descriptions.

/// The kerning of a font: its groups and the kerning pairs of each master.
#[derive(Deserialize, JsonSchema)]
#[schemars(rename = "Kerning")]
struct KerningDocument {
    /// The kerning groups, sorted by id. Each glyph belongs to at most one
    /// group per side; names are unique per side.
    #[serde(default)]
    groups: Vec<KerningGroup>,
    /// The kerning pairs of each master, keyed by master id, each list sorted
    /// by `(first, second)` and without repeated pairs.
    #[serde(default)]
    pairs: BTreeMap<MasterId, Vec<KerningPair>>,
}

/// The groups of a [`Kerning`], serialized as a list in id order.
struct GroupList<'a>(&'a BTreeMap<KerningGroupId, KerningGroup>);

impl Serialize for GroupList<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_seq(self.0.values())
    }
}

/// The pairs of one master, serialized as a list of [`KerningPair`]s in
/// `(first, second)` order.
struct PairList<'a>(&'a BTreeMap<(KernRef, KernRef), f64>);

impl Serialize for PairList<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_seq(self.0.iter().map(|(&(first, second), &value)| KerningPair {
            first,
            second,
            value,
        }))
    }
}

/// The pairs of every master, serialized as a map from master id to list.
struct MasterPairs<'a>(&'a BTreeMap<MasterId, BTreeMap<(KernRef, KernRef), f64>>);

impl Serialize for MasterPairs<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_map(
            self.0
                .iter()
                .map(|(master, pairs)| (master, PairList(pairs))),
        )
    }
}

impl Serialize for Kerning {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut document = serializer.serialize_struct("Kerning", 2)?;
        document.serialize_field("groups", &GroupList(&self.groups))?;
        document.serialize_field("pairs", &MasterPairs(&self.pairs))?;
        document.end()
    }
}

impl<'de> Deserialize<'de> for Kerning {
    /// Reads the serialized form; a group id or a pair of a master listed
    /// twice is an error, so every value has one place in the JSON.
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let document = KerningDocument::deserialize(deserializer)?;
        let mut groups = BTreeMap::new();
        for group in document.groups {
            let id = group.id;
            if groups.insert(id, group).is_some() {
                return Err(D::Error::custom(format!(
                    "kerning group {id} is listed more than once"
                )));
            }
        }
        let mut pairs = BTreeMap::new();
        for (master, list) in document.pairs {
            let mut values = BTreeMap::new();
            for pair in list {
                if values
                    .insert((pair.first, pair.second), pair.value)
                    .is_some()
                {
                    return Err(D::Error::custom(format!(
                        "kerning pair {}/{} of master {master} is listed more than once",
                        pair.first, pair.second
                    )));
                }
            }
            pairs.insert(master, values);
        }
        Ok(Kerning { groups, pairs })
    }
}

impl JsonSchema for Kerning {
    fn schema_name() -> Cow<'static, str> {
        "Kerning".into()
    }

    fn schema_id() -> Cow<'static, str> {
        concat!(module_path!(), "::Kerning").into()
    }

    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        KerningDocument::json_schema(generator)
    }
}
