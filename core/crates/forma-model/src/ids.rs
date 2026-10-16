//! Stable identifiers and entity keys.
//!
//! Every document, master and glyph carries a random UUID that never changes
//! for the life of the entity, so edits, history and external references stay
//! valid across renames and reordering. Contours use a small per-layer counter.

use std::fmt;
use std::str::FromStr;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use uuid::Uuid;

use crate::font::ModelError;

macro_rules! uuid_id {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(
            Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash,
            Serialize, Deserialize, JsonSchema,
        )]
        #[serde(transparent)]
        pub struct $name(pub Uuid);

        // No `Default`: a random identifier is not a meaningful default value.
        #[allow(clippy::new_without_default)]
        impl $name {
            /// Creates a fresh random (version 4) identifier.
            #[must_use]
            pub fn new() -> Self {
                Self(Uuid::new_v4())
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                fmt::Display::fmt(&self.0, f)
            }
        }

        impl FromStr for $name {
            type Err = uuid::Error;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Uuid::from_str(s).map(Self)
            }
        }
    };
}

uuid_id! {
    /// Stable identifier of a font document.
    FontId
}

uuid_id! {
    /// Stable identifier of a master.
    MasterId
}

uuid_id! {
    /// Stable identifier of a glyph. Unlike the glyph name, it never changes.
    GlyphId
}

/// Identifier of a contour, unique within its layer.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(transparent)]
pub struct PathId(pub u32);

/// The unit of optimistic concurrency: the part of a document that carries its
/// own revision number.
///
/// Its text form is `font_info`, `vertical_metrics:<master uuid>`,
/// `glyph_order` or `glyph:<glyph uuid>`; this is also its serde
/// representation, so it can be used as a JSON object key.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum EntityKey {
    /// The font-level naming and version information.
    FontInfo,
    /// The vertical metrics of one master.
    VerticalMetrics(MasterId),
    /// The ordering of the glyphs.
    GlyphOrder,
    /// One glyph: its name, encoding and outlines.
    Glyph(GlyphId),
}

impl fmt::Display for EntityKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FontInfo => f.write_str("font_info"),
            Self::VerticalMetrics(master) => write!(f, "vertical_metrics:{master}"),
            Self::GlyphOrder => f.write_str("glyph_order"),
            Self::Glyph(glyph) => write!(f, "glyph:{glyph}"),
        }
    }
}

/// Parses a UUID, accepting only the canonical lowercase hyphenated form so
/// that every key has exactly one textual representation.
fn parse_canonical_uuid(text: &str) -> Option<Uuid> {
    let uuid = Uuid::parse_str(text).ok()?;
    (uuid.hyphenated().to_string() == text).then_some(uuid)
}

impl FromStr for EntityKey {
    type Err = ModelError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let invalid = || ModelError::InvalidEntityKey(s.to_owned());
        if s == "font_info" {
            Ok(Self::FontInfo)
        } else if s == "glyph_order" {
            Ok(Self::GlyphOrder)
        } else if let Some(rest) = s.strip_prefix("vertical_metrics:") {
            parse_canonical_uuid(rest)
                .map(|uuid| Self::VerticalMetrics(MasterId(uuid)))
                .ok_or_else(invalid)
        } else if let Some(rest) = s.strip_prefix("glyph:") {
            parse_canonical_uuid(rest)
                .map(|uuid| Self::Glyph(GlyphId(uuid)))
                .ok_or_else(invalid)
        } else {
            Err(invalid())
        }
    }
}

impl Serialize for EntityKey {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for EntityKey {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        text.parse().map_err(D::Error::custom)
    }
}

impl JsonSchema for EntityKey {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "EntityKey".into()
    }

    fn json_schema(_generator: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "string",
            "description": "Identifies a revisioned part of a document: `font_info`, \
                            `vertical_metrics:<master uuid>`, `glyph_order` or \
                            `glyph:<glyph uuid>`.",
            "pattern": "^(font_info|glyph_order|(vertical_metrics|glyph):[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12})$"
        })
    }
}
