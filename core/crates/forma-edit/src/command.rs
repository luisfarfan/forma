//! The typed edit commands.
//!
//! Every mutation of a font is one of these commands. They are plain data
//! (serializable, with JSON schemas) so that the editor, the CLI and AI agents
//! all describe edits the same way; [`apply`](crate::apply) executes them.
//!
//! On the wire a command is a JSON object tagged by `op` with a `snake_case`
//! name, for example:
//!
//! ```json
//! { "op": "set_metrics", "glyph": "A", "rsb": 40 }
//! ```

use std::fmt;
use std::str::FromStr;

use forma_geometry::IssueKind;
use forma_model::{Font, GlyphId, KernRef, KernSide, MasterId, Path, PathId, Transform};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A reference to a glyph, by id or by name.
///
/// Serialized as a bare string: text that parses as a UUID is an [`Id`]
/// (`GlyphRef::Id`), any other text is a [`Name`] (`GlyphRef::Name`). When no
/// glyph has the id, a glyph whose *name* is that same UUID text is used, so a
/// glyph that happens to be named like a UUID can still be addressed.
///
/// [`Id`]: GlyphRef::Id
/// [`Name`]: GlyphRef::Name
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum GlyphRef {
    /// The stable identifier of the glyph.
    Id(GlyphId),
    /// The name of the glyph.
    Name(String),
}

impl fmt::Display for GlyphRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Id(id) => fmt::Display::fmt(id, f),
            Self::Name(name) => f.write_str(name),
        }
    }
}

impl GlyphRef {
    /// The id of the glyph this reference points at in `font`, if any.
    ///
    /// An id is looked up first; when no glyph has it, a glyph whose name is
    /// the same UUID text is used. A name is only ever matched against glyph
    /// names. Every command resolves its references this way.
    #[must_use]
    pub fn resolve(&self, font: &Font) -> Option<GlyphId> {
        match self {
            Self::Id(id) => {
                if font.glyphs.contains_key(id) {
                    Some(*id)
                } else {
                    font.glyphs
                        .values()
                        .find(|candidate| candidate.name.parse::<GlyphId>().ok() == Some(*id))
                        .map(|candidate| candidate.id)
                }
            }
            Self::Name(name) => font.glyph_by_name(name).map(|candidate| candidate.id),
        }
    }
}

impl From<GlyphId> for GlyphRef {
    fn from(id: GlyphId) -> Self {
        Self::Id(id)
    }
}

impl FromStr for GlyphRef {
    type Err = std::convert::Infallible;

    /// Parses reference text the way deserialization does: UUID text is an
    /// id, anything else is a name.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(s.parse::<GlyphId>()
            .map_or_else(|_| Self::Name(s.to_owned()), Self::Id))
    }
}

/// A component to place in a glyph layer: another glyph, drawn through an
/// affine transform.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ComponentInput {
    /// The glyph to draw. It must have a layer in the master being edited and
    /// must not lead back to the glyph through its own components.
    pub base: GlyphRef,
    /// How the base glyph is placed; the identity when omitted. All six
    /// numbers must be finite and `|xx·yy − xy·yx| ≥ 1e-6`.
    #[serde(default)]
    pub transform: Transform,
}

/// One side of a kerning pair, as a command names it.
///
/// On the wire it is an object with one key: `{"glyph": "<glyph>"}` or
/// `{"group": "<group name>"}`. A group is looked up among the groups of the
/// side it stands on: first-side groups as `first`, second-side groups as
/// `second`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum KernRefInput {
    /// One glyph.
    Glyph(GlyphRef),
    /// The kerning group with this name on the side of the pair it stands on.
    Group(String),
}

impl KernRefInput {
    /// What this side refers to in `font` when it stands on `side` of a
    /// pair, if it exists: a glyph is resolved as [`GlyphRef::resolve`]
    /// does, a group is looked up by name among the groups of `side`.
    #[must_use]
    pub fn resolve(&self, font: &Font, side: KernSide) -> Option<KernRef> {
        match self {
            Self::Glyph(glyph) => glyph.resolve(font).map(KernRef::Glyph),
            Self::Group(name) => font
                .kerning
                .group_by_name(side, name)
                .map(|group| KernRef::Group(group.id)),
        }
    }
}

/// An edit of the contours of one glyph layer.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContourAction {
    /// Appends a contour. It gets the id `max existing id + 1` (0 for an empty
    /// layer); the id in `path` is ignored.
    Add {
        /// The contour to add.
        path: Path,
    },
    /// Removes a contour.
    Delete {
        /// The contour to remove.
        path_id: PathId,
    },
    /// Reverses the drawing direction of a contour without changing its shape.
    Reverse {
        /// The contour to reverse.
        path_id: PathId,
    },
    /// Applies an affine transform to every node of a contour.
    ///
    /// The six values `[a, b, c, d, e, f]` map a point to
    /// `(a*x + c*y + e, b*x + d*y + f)`. A transform that mirrors the contour
    /// also flips its direction; the direction is not corrected.
    Transform {
        /// The contour to transform.
        path_id: PathId,
        /// The affine coefficients `[a, b, c, d, e, f]`; all must be finite.
        affine: [f64; 6],
    },
}

/// An edit of the nodes of one contour.
///
/// Nodes are addressed by their index in the contour's `nodes` list.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum NodeAction {
    /// Moves exactly the listed nodes by `(dx, dy)`. Handles do not follow
    /// their on-curve node: list off-curve nodes explicitly to move them.
    Move {
        /// Indices of the nodes to move; each must exist.
        indices: Vec<usize>,
        /// Horizontal offset in font units.
        dx: f64,
        /// Vertical offset in font units.
        dy: f64,
    },
    /// Splits a segment at parameter `t` with an on-curve node, keeping the
    /// shape.
    ///
    /// Segment `k` is the segment that ends at the `k`-th on-curve node in
    /// `nodes` order. In a closed contour segment 0 ends at the first on-curve
    /// node and starts at the last one; in an open contour the first on-curve
    /// node is the starting point, so segment 0 ends at the second on-curve
    /// node. The new node has the kind of the segment (line, curve or qcurve)
    /// and is smooth when the segment is a curve.
    Insert {
        /// The segment to split.
        segment_index: usize,
        /// Where to split, strictly between 0 and 1.
        t: f64,
    },
    /// Deletes an on-curve node and its handles; the two segments around it
    /// merge into one (a cubic if both were cubic, otherwise a line).
    Delete {
        /// Index of the on-curve node to delete.
        index: usize,
    },
    /// Sets whether an on-curve node is smooth. Only the flag changes; handles
    /// are not realigned.
    SetSmooth {
        /// Index of the on-curve node.
        index: usize,
        /// The new value of the flag.
        smooth: bool,
    },
}

/// A mutation of a font.
///
/// Wherever `master` is optional, leaving it out means the default (first)
/// master of the font.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Command {
    /// Creates a glyph with a layer for every master, all with the same
    /// contours and advance width, and appends it to the glyph order.
    ///
    /// The contours are numbered from 0 in the given order (their ids are
    /// ignored) and keep their direction as given.
    CreateGlyph {
        /// The glyph name: 1 to 63 printable ASCII characters without spaces,
        /// not starting with a digit or a period (or exactly `.notdef`). It
        /// must be unique in the font.
        name: String,
        /// Unicode code points the glyph encodes; none may belong to another
        /// glyph.
        #[serde(default)]
        unicodes: Vec<u32>,
        /// Advance width in font units; finite and not negative.
        advance_width: f64,
        /// The contours of the glyph.
        #[serde(default)]
        paths: Vec<Path>,
    },
    /// Changes the name, the encoding or the export flag of a glyph. At least
    /// one of the fields must be given.
    UpdateGlyph {
        /// The glyph to change.
        glyph: GlyphRef,
        /// The new name; same rules as in `create_glyph`.
        name: Option<String>,
        /// The new code points, replacing the old ones.
        unicodes: Option<Vec<u32>>,
        /// Whether the glyph is exported with the font.
        export: Option<bool>,
    },
    /// Deletes glyphs and removes them from the glyph order. The list must not
    /// be empty and every glyph must exist. A glyph that another glyph not
    /// deleted with it uses as a component cannot be deleted (decompose that
    /// glyph first). The deleted glyphs leave their kerning groups, and the
    /// kerning pairs that name them go too.
    DeleteGlyphs {
        /// The glyphs to delete.
        glyphs: Vec<GlyphRef>,
    },
    /// Replaces all contours of a glyph in one master. The contours are
    /// numbered from 0 in the given order (their ids are ignored) and keep
    /// their direction as given.
    SetOutline {
        /// The glyph to change.
        glyph: GlyphRef,
        /// The master to change.
        master: Option<MasterId>,
        /// The new contours.
        paths: Vec<Path>,
    },
    /// Adds, deletes, reverses or transforms a contour of a glyph.
    ContourEdit {
        /// The glyph to change.
        glyph: GlyphRef,
        /// The master to change.
        master: Option<MasterId>,
        /// What to do.
        action: ContourAction,
    },
    /// Edits the nodes of one contour of a glyph.
    NodesEdit {
        /// The glyph to change.
        glyph: GlyphRef,
        /// The master to change.
        master: Option<MasterId>,
        /// The contour to edit.
        path_id: PathId,
        /// What to do.
        action: NodeAction,
    },
    /// Sets the advance width or the side bearings of a glyph in one master.
    ///
    /// The outline is the contours together with the components. `lsb`
    /// shifts all contours and components horizontally so that the left edge
    /// of the outline sits at `lsb`, moving the advance width along (the
    /// right side bearing is kept). `rsb` sets the advance width to the right
    /// edge of the outline plus `rsb`. `advance_width` sets the advance width
    /// directly. At least one must be given; `advance_width` and `rsb` cannot
    /// be combined; `lsb` and `rsb` need a glyph with an outline. `lsb` is
    /// applied first.
    SetMetrics {
        /// The glyph to change.
        glyph: GlyphRef,
        /// The master to change.
        master: Option<MasterId>,
        /// The new advance width.
        advance_width: Option<f64>,
        /// The new left side bearing.
        lsb: Option<f64>,
        /// The new right side bearing.
        rsb: Option<f64>,
    },
    /// Sets font-level naming and version fields. At least one field must be
    /// given. `units_per_em` only changes the number; outlines are not scaled.
    SetFontInfo {
        /// The family name; not empty.
        family_name: Option<String>,
        /// The style name; not empty.
        style_name: Option<String>,
        /// Major version number.
        version_major: Option<u16>,
        /// Minor version number.
        version_minor: Option<u16>,
        /// Copyright notice; an empty string removes it.
        copyright: Option<String>,
        /// Designer name; an empty string removes it.
        designer: Option<String>,
        /// Units per em, within 16 to 16384.
        units_per_em: Option<u16>,
    },
    /// Sets vertical metrics of a master. At least one field must be given.
    SetVerticalMetrics {
        /// The master to change.
        master: Option<MasterId>,
        /// Ascender height.
        ascender: Option<f64>,
        /// Descender depth (negative).
        descender: Option<f64>,
        /// Height of lowercase letters such as `x`.
        x_height: Option<f64>,
        /// Height of capital letters.
        cap_height: Option<f64>,
    },
    /// Applies the safe automatic geometry fixes to glyphs, in every master.
    ApplyGeometryFixes {
        /// The glyphs to fix; empty means every glyph of the font.
        #[serde(default)]
        glyphs: Vec<GlyphRef>,
        /// The kinds of problem to fix.
        kinds: Vec<IssueKind>,
        /// The grid, in font units, for the off-grid fix; not negative.
        grid: f64,
    },
    /// Replaces the components of a glyph in one master; an empty list
    /// removes them. The contours and the advance width stay as they are. A
    /// glyph without a layer in the master gets one, without contours and
    /// with the advance width of its default layer.
    SetComponents {
        /// The glyph to change.
        glyph: GlyphRef,
        /// The master to change.
        #[serde(default)]
        master: Option<MasterId>,
        /// The new components, drawn in this order after the contours.
        components: Vec<ComponentInput>,
    },
    /// Replaces the components of glyphs, in one master, with the contours
    /// they draw, moved by their transforms (nested components included).
    /// The glyph's own contours keep their ids; the new ones are numbered
    /// after them. Glyphs without components in the master are left alone.
    /// The list must not be empty.
    Decompose {
        /// The glyphs to decompose.
        glyphs: Vec<GlyphRef>,
        /// The master to change.
        #[serde(default)]
        master: Option<MasterId>,
    },
    /// Unites the overlapping closed contours of glyphs, in one master
    /// (non-zero fill rule; open contours are kept as they are). Components
    /// are left as they are: decompose first to unite them with the contours.
    /// Glyphs whose contours do not overlap are left alone. The list must not
    /// be empty.
    RemoveOverlap {
        /// The glyphs to clean.
        glyphs: Vec<GlyphRef>,
        /// The master to change.
        #[serde(default)]
        master: Option<MasterId>,
    },
    /// Sets the kerning pair `first`/`second` of one master; a value of 0
    /// removes it. A pair whose value does not change is not touched.
    SetKerningPair {
        /// The master to change.
        #[serde(default)]
        master: Option<MasterId>,
        /// The left side of the pair (in a left-to-right script): a glyph,
        /// or a first-side group.
        first: KernRefInput,
        /// The right side of the pair: a glyph, or a second-side group.
        second: KernRefInput,
        /// The adjustment in font units, negative to bring the sides
        /// closer; finite and at most twice the units per em in absolute
        /// value. 0 removes the pair.
        value: f64,
    },
    /// Creates the kerning group `name` on `side`, or replaces the members
    /// of the existing one. A glyph belongs to at most one group per side,
    /// so no member may be in another group of that side. A group whose
    /// members do not change is not touched.
    SetKerningGroup {
        /// The group name, unique on its side; it follows the glyph name
        /// rules.
        name: String,
        /// The side of a pair the group stands on.
        side: KernSide,
        /// The members, each listed once; may be empty.
        members: Vec<GlyphRef>,
    },
    /// Deletes the kerning group `name` of `side`, together with every
    /// kerning pair, in every master, that uses it.
    DeleteKerningGroup {
        /// The group name.
        name: String,
        /// The side of the group.
        side: KernSide,
    },
}
