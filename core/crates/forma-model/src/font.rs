//! The font document.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use thiserror::Error;

use crate::component::check_components;
use crate::glyph::{Glyph, is_valid_glyph_name};
use crate::ids::{FontId, GlyphId, KerningGroupId, MasterId};
use crate::kerning::{KernSide, Kerning};

/// The smallest `units_per_em` allowed by OpenType.
pub const MIN_UNITS_PER_EM: u16 = 16;
/// The largest `units_per_em` allowed by OpenType.
pub const MAX_UNITS_PER_EM: u16 = 16384;

/// A violation of a data model rule, or text that is not a valid model value.
///
/// Not `Eq`: [`ModelError::InvalidKerningValue`] carries the offending
/// number, which may be NaN.
#[derive(Debug, Clone, PartialEq, Error)]
#[non_exhaustive]
pub enum ModelError {
    /// `units_per_em` is outside the OpenType range.
    #[error(
        "units per em {0} is outside the OpenType range {MIN_UNITS_PER_EM}..={MAX_UNITS_PER_EM}"
    )]
    UnitsPerEmOutOfRange(u16),
    /// The font has no masters.
    #[error("the font has no masters")]
    NoMasters,
    /// Two masters share an identifier.
    #[error("master id {0} is used by more than one master")]
    DuplicateMasterId(MasterId),
    /// A glyph is stored under a key that is not its own identifier.
    #[error("the glyph with id {id} is stored under key {key}")]
    GlyphKeyMismatch {
        /// The key in the glyph map.
        key: GlyphId,
        /// The identifier inside the glyph.
        id: GlyphId,
    },
    /// A glyph name breaks the naming rules.
    #[error("glyph {glyph} has the invalid name {name:?}")]
    InvalidGlyphName {
        /// The offending glyph.
        glyph: GlyphId,
        /// Its name.
        name: String,
    },
    /// Two glyphs share a name.
    #[error("glyphs {first} and {second} have the same name {name:?}")]
    DuplicateGlyphName {
        /// The shared name.
        name: String,
        /// The glyph encountered first.
        first: GlyphId,
        /// The glyph encountered second.
        second: GlyphId,
    },
    /// The glyph order lists the same glyph more than once.
    #[error("glyph {0} appears more than once in the glyph order")]
    DuplicateInGlyphOrder(GlyphId),
    /// The glyph order lists a glyph that does not exist.
    #[error("the glyph order lists glyph {0}, which does not exist")]
    GlyphOrderUnknownGlyph(GlyphId),
    /// A glyph is missing from the glyph order.
    #[error("glyph {0} is missing from the glyph order")]
    GlyphMissingFromOrder(GlyphId),
    /// A code point is assigned more than once.
    #[error("code point U+{codepoint:04X} is assigned to both glyph {first} and glyph {second}")]
    DuplicateUnicode {
        /// The code point.
        codepoint: u32,
        /// The glyph that was assigned it first (may equal `second`).
        first: GlyphId,
        /// The glyph that was assigned it again.
        second: GlyphId,
    },
    /// A glyph has a layer for a master the font does not have.
    #[error("glyph {glyph} has a layer for the unknown master {master}")]
    UnknownMaster {
        /// The glyph with the stray layer.
        glyph: GlyphId,
        /// The missing master.
        master: MasterId,
    },
    /// Text is not a valid actor.
    #[error("invalid actor {0:?}: expected \"user\", \"cli\" or \"agent:<name>\"")]
    InvalidActor(String),
    /// Text is not a valid entity key.
    #[error("invalid entity key {0:?}")]
    InvalidEntityKey(String),
    /// Text is not a valid kerning reference.
    #[error("invalid kerning reference {0:?}: expected \"g.<glyph uuid>\" or \"k.<group uuid>\"")]
    InvalidKernRef(String),
    /// A component refers to a glyph that does not exist.
    #[error("glyph {glyph:?} has a component of the unknown glyph {base}")]
    UnknownComponentBase {
        /// The name of the glyph with the component.
        glyph: String,
        /// The missing base glyph.
        base: GlyphId,
    },
    /// A component refers to a glyph that has no layer in the component's
    /// master.
    #[error(
        "glyph {glyph:?} has a component of {base:?} in master {master}, where {base:?} has no layer"
    )]
    ComponentMissingLayer {
        /// The name of the glyph with the component.
        glyph: String,
        /// The name of the base glyph.
        base: String,
        /// The master of the component.
        master: MasterId,
    },
    /// Following components from a glyph leads back to it.
    #[error("the components of glyph {glyph:?} lead back to it (a component cycle)")]
    ComponentCycle {
        /// The name of a glyph on the cycle.
        glyph: String,
    },
    /// A component transform is not finite or is singular.
    #[error(
        "glyph {glyph:?} has a component transform that is not finite or is singular \
         (|determinant| < 1e-6)"
    )]
    InvalidTransform {
        /// The name of the glyph with the component.
        glyph: String,
    },
    /// Kerning pairs are stored for a master the font does not have.
    #[error("kerning pairs are stored for the unknown master {0}")]
    KerningUnknownMaster(MasterId),
    /// A kerning group or pair refers to a glyph that does not exist.
    #[error("kerning refers to the unknown glyph {0}")]
    KerningUnknownGlyph(GlyphId),
    /// A kerning pair refers to a group that does not exist.
    #[error("a kerning pair refers to the unknown kerning group {0}")]
    KerningUnknownGroup(KerningGroupId),
    /// A kerning group is used on the other side of a pair than its own.
    #[error("kerning group {group:?} is used on the wrong side of a pair")]
    KerningGroupWrongSide {
        /// The group name.
        group: String,
    },
    /// A glyph is listed more than once in the groups of one side.
    #[error("glyph {glyph:?} is listed more than once in the {side}-side kerning groups")]
    GlyphInTwoGroups {
        /// The glyph name.
        glyph: String,
        /// The side of the groups.
        side: KernSide,
    },
    /// A kerning group name breaks the naming rules (those of glyph names).
    #[error("invalid kerning group name {0:?}: group names follow the glyph name rules")]
    InvalidGroupName(String),
    /// Two kerning groups of one side share a name.
    #[error("two {side}-side kerning groups are named {name:?}")]
    DuplicateGroupName {
        /// The side of the groups.
        side: KernSide,
        /// The shared name.
        name: String,
    },
    /// A kerning value is not finite, is 0 or exceeds twice the units per
    /// em in absolute value.
    #[error(
        "invalid kerning value {value}: it must be finite, not 0, and at most twice the units \
         per em in absolute value"
    )]
    InvalidKerningValue {
        /// The offending value.
        value: f64,
    },
    /// A kerning group is stored under a key that is not its own identifier.
    #[error("the kerning group with id {id} is stored under key {key}")]
    GroupKeyMismatch {
        /// The key in the group map.
        key: KerningGroupId,
        /// The identifier inside the group.
        id: KerningGroupId,
    },
}

/// Font-level naming and version information.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct FontInfo {
    /// The family name.
    pub family_name: String,
    /// The style name, such as `Regular`.
    pub style_name: String,
    /// Major version number.
    pub version_major: u16,
    /// Minor version number.
    pub version_minor: u16,
    /// Copyright notice.
    pub copyright: Option<String>,
    /// Designer name.
    pub designer: Option<String>,
}

/// Vertical metrics of a master, in font units.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct VerticalMetrics {
    /// Ascender height (positive).
    pub ascender: f64,
    /// Descender depth (negative).
    pub descender: f64,
    /// Height of lowercase letters such as `x`.
    pub x_height: f64,
    /// Height of capital letters.
    pub cap_height: f64,
}

/// One master (design) of the font.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Master {
    /// Stable identifier of the master.
    pub id: MasterId,
    /// The master name, such as `Regular`.
    pub name: String,
    /// The vertical metrics of the master.
    pub metrics: VerticalMetrics,
}

/// A font document.
///
/// Glyphs live behind [`Arc`]s so that cloning a font is cheap and revisions of
/// a document share every glyph that did not change; use [`Arc::make_mut`] to
/// edit them copy-on-write.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Font {
    /// Stable identifier of the document.
    pub id: FontId,
    /// Naming and version information.
    pub info: FontInfo,
    /// Units per em, within `16..=16384`.
    pub units_per_em: u16,
    /// The masters; the first one is the default master.
    pub masters: Vec<Master>,
    /// All glyphs, keyed by their identifier.
    pub glyphs: Arc<BTreeMap<GlyphId, Arc<Glyph>>>,
    /// The glyph order: every glyph exactly once.
    pub glyph_order: Vec<GlyphId>,
    /// The kerning groups and pairs; omitted when there are none.
    #[serde(default, skip_serializing_if = "Kerning::is_empty")]
    pub kerning: Kerning,
    /// Free-form data that the model does not interpret but must preserve.
    pub lib: Map<String, Value>,
}

impl Font {
    /// Creates a font with one `Regular` master, no glyphs and no kerning.
    ///
    /// The vertical metrics are proportional to `units_per_em`: ascender 80%,
    /// descender -20%, x-height 50% and cap height 70% of it (800, -200, 500
    /// and 700 for 1000 units per em), without rounding. The style name is
    /// `Regular` and the version is 1.0.
    #[must_use]
    pub fn new(family: &str, units_per_em: u16) -> Font {
        // Multiply first, then divide once, so each value is the correctly
        // rounded result of the exact proportion (2048 -> 1638.4, not 1638.4000000000001).
        let upm = f64::from(units_per_em);
        let metrics = VerticalMetrics {
            ascender: upm * 4.0 / 5.0,
            descender: -upm / 5.0,
            x_height: upm / 2.0,
            cap_height: upm * 7.0 / 10.0,
        };
        Font {
            id: FontId::new(),
            info: FontInfo {
                family_name: family.to_owned(),
                style_name: "Regular".to_owned(),
                version_major: 1,
                version_minor: 0,
                copyright: None,
                designer: None,
            },
            units_per_em,
            masters: vec![Master {
                id: MasterId::new(),
                name: "Regular".to_owned(),
                metrics,
            }],
            glyphs: Arc::new(BTreeMap::new()),
            glyph_order: Vec::new(),
            kerning: Kerning::default(),
            lib: Map::new(),
        }
    }

    /// Finds a glyph by name.
    #[must_use]
    pub fn glyph_by_name(&self, name: &str) -> Option<&Arc<Glyph>> {
        self.glyphs.values().find(|glyph| glyph.name == name)
    }

    /// The glyphs that use `base` as a component in some master, each once,
    /// in glyph order (then, in an invalid font, any glyph the order does
    /// not list, by id).
    #[must_use]
    pub fn components_using(&self, base: GlyphId) -> Vec<GlyphId> {
        let mut seen = HashSet::new();
        self.glyph_order
            .iter()
            .chain(self.glyphs.keys())
            .filter(|id| seen.insert(**id))
            .filter(|id| {
                self.glyphs
                    .get(id)
                    .is_some_and(|glyph| glyph.uses_component(base))
            })
            .copied()
            .collect()
    }

    /// The default master, which is the first one.
    ///
    /// # Panics
    ///
    /// Panics if the font has no masters, which [`Font::validate_invariants`]
    /// rejects.
    #[must_use]
    pub fn default_master(&self) -> &Master {
        &self.masters[0]
    }

    /// Checks the rules every well-formed font obeys, reporting the first
    /// violation found:
    ///
    /// - `units_per_em` is within the OpenType range `16..=16384`;
    /// - there is at least one master, and master ids are unique;
    /// - each glyph is stored under its own id, has a valid, unique name and
    ///   only layers for existing masters;
    /// - no code point is assigned more than once;
    /// - the glyph order lists exactly the glyphs of the font, each once;
    /// - every component refers to an existing glyph that has a layer in the
    ///   component's master, through a finite transform with
    ///   `|determinant| ≥ 1e-6`, and following components never leads back to
    ///   the same glyph (in any master);
    /// - every kerning group is stored under its own id, has a name that
    ///   follows the glyph name rules and is unique on its side, and lists
    ///   existing glyphs; no glyph is listed twice in the groups of one side;
    /// - kerning pairs are stored for existing masters, refer to existing
    ///   glyphs and to groups of the right side (first-side groups as
    ///   `first`, second-side groups as `second`), and have a finite, non-zero
    ///   value of at most twice the units per em in absolute value (a value of
    ///   0 is no pair: it is not stored).
    ///
    /// # Errors
    ///
    /// Returns the [`ModelError`] describing the first violated rule.
    pub fn validate_invariants(&self) -> Result<(), ModelError> {
        if !(MIN_UNITS_PER_EM..=MAX_UNITS_PER_EM).contains(&self.units_per_em) {
            return Err(ModelError::UnitsPerEmOutOfRange(self.units_per_em));
        }

        if self.masters.is_empty() {
            return Err(ModelError::NoMasters);
        }
        let mut master_ids = HashSet::new();
        for master in &self.masters {
            if !master_ids.insert(master.id) {
                return Err(ModelError::DuplicateMasterId(master.id));
            }
        }

        let mut names: HashMap<&str, GlyphId> = HashMap::new();
        let mut codepoints: HashMap<u32, GlyphId> = HashMap::new();
        for (&key, glyph) in self.glyphs.iter() {
            if key != glyph.id {
                return Err(ModelError::GlyphKeyMismatch { key, id: glyph.id });
            }
            if !is_valid_glyph_name(&glyph.name) {
                return Err(ModelError::InvalidGlyphName {
                    glyph: glyph.id,
                    name: glyph.name.clone(),
                });
            }
            if let Some(first) = names.insert(&glyph.name, glyph.id) {
                return Err(ModelError::DuplicateGlyphName {
                    name: glyph.name.clone(),
                    first,
                    second: glyph.id,
                });
            }
            for &codepoint in &glyph.unicodes {
                if let Some(first) = codepoints.insert(codepoint, glyph.id) {
                    return Err(ModelError::DuplicateUnicode {
                        codepoint,
                        first,
                        second: glyph.id,
                    });
                }
            }
            for &master in glyph.layers.keys() {
                if !master_ids.contains(&master) {
                    return Err(ModelError::UnknownMaster {
                        glyph: glyph.id,
                        master,
                    });
                }
            }
        }

        let mut listed = HashSet::new();
        for &id in &self.glyph_order {
            if !self.glyphs.contains_key(&id) {
                return Err(ModelError::GlyphOrderUnknownGlyph(id));
            }
            if !listed.insert(id) {
                return Err(ModelError::DuplicateInGlyphOrder(id));
            }
        }
        for &id in self.glyphs.keys() {
            if !listed.contains(&id) {
                return Err(ModelError::GlyphMissingFromOrder(id));
            }
        }

        check_components(self)?;
        self.kerning.check(self)
    }
}
