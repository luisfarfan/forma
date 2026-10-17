//! `Steps`: the commands of one call, measured on a scratch copy of the
//! document's font before the transaction is committed.

use std::collections::BTreeSet;
use std::sync::Arc;

use forma_edit::{Command, ContourAction, GlyphRef, apply};
use forma_model::{EntityKey, Font, GlyphId, KernSide, Layer, MasterId};

use crate::errors::{ToolError, from_edit_error};
use crate::registry::Effects;
use crate::tools::limits::{check_nodes, command_nodes};
use crate::tools::plans::is_affected;

/// The absolute determinant below which a contour transform is singular: it
/// collapses the contour onto a line or a point.
const SINGULAR_DETERMINANT: f64 = 1e-9;

/// The commands of one call, applied in order to a scratch copy of the
/// document's font as they are added.
///
/// Applying them measures, before anything is committed, what the
/// transaction will do: the state after each command, the distinct glyphs,
/// kerning pairs and kerning groups the commands touch and whether any of
/// them destroys data. Each command is judged against the state the previous
/// ones left (a glyph renamed by one command and replaced by the next is the
/// same glyph).
#[derive(Clone, Debug)]
pub(crate) struct Steps {
    /// The document's font the commands start from.
    pub before: Arc<Font>,
    /// The scratch state after the commands so far. Glyphs created by the
    /// call have scratch ids here; the committed transaction gives them new
    /// ones.
    pub after: Font,
    /// The commands so far, in order.
    pub cmds: Vec<Command>,
    /// The distinct entities the commands touch that count toward the mass
    /// threshold: glyphs (by their id in `after`, deleted glyphs by their id
    /// in `before`), kerning pairs and kerning groups (see
    /// [`is_affected`]).
    pub affected: BTreeSet<EntityKey>,
    /// Every entity the commands touch (glyphs the call creates by their
    /// scratch id, which no revision has seen), for the conflict check.
    pub touched: BTreeSet<EntityKey>,
    /// Whether some command, at its turn, destroys data that existed before
    /// the call: it deletes glyphs or a kerning group, replaces the non-empty
    /// layer or the non-empty component list of a glyph that existed before
    /// the call, removes the last contour of a layer that had contours when
    /// the call started, applies a singular transform to a contour, or
    /// removes a kerning pair that existed before the call.
    pub destructive: bool,
    /// The outline nodes the commands bring so far.
    nodes: usize,
}

impl Steps {
    /// No commands yet, starting from `font` (the document's current state).
    #[must_use]
    pub(crate) fn new(font: &Arc<Font>) -> Steps {
        Steps {
            before: Arc::clone(font),
            after: Font::clone(font),
            cmds: Vec::new(),
            affected: BTreeSet::new(),
            touched: BTreeSet::new(),
            destructive: false,
            nodes: 0,
        }
    }

    /// `cmds`, applied in order from `font`.
    ///
    /// # Errors
    ///
    /// The error of the first command that [`push`](Self::push) refuses.
    pub(crate) fn of(font: &Arc<Font>, cmds: Vec<Command>) -> Result<Steps, ToolError> {
        let mut steps = Steps::new(font);
        for cmd in cmds {
            steps.push(cmd)?;
        }
        Ok(steps)
    }

    /// Applies `cmd` to the scratch state and records its effects.
    ///
    /// # Errors
    ///
    /// `LIMIT_EXCEEDED` when the commands would bring more than
    /// [`MAX_NODES`](crate::limits::MAX_NODES) nodes in total (checked before
    /// applying), and the mapped error of [`apply`]. A refused command leaves
    /// the steps unchanged.
    pub(crate) fn push(&mut self, cmd: Command) -> Result<(), ToolError> {
        let nodes = self.nodes.saturating_add(command_nodes(&self.after, &cmd));
        check_nodes(nodes)?;
        let destructive = self.destroys(&cmd);
        let (after, touched) = apply(&self.after, &cmd).map_err(from_edit_error)?;
        let destructive = destructive || self.empties_drawn_layer(&after, &touched);
        self.affected
            .extend(touched.iter().copied().filter(is_affected));
        self.touched.extend(touched);
        self.destructive |= destructive;
        self.nodes = nodes;
        self.after = after;
        self.cmds.push(cmd);
        Ok(())
    }

    /// What the commands do: [`Effects::DESTRUCTIVE`] when one of them
    /// destroys data, [`Effects::WRITE`] otherwise.
    #[must_use]
    pub(crate) fn effects(&self) -> Effects {
        if self.destructive {
            Effects::DESTRUCTIVE
        } else {
            Effects::WRITE
        }
    }

    /// Whether `cmd`, applied to the current scratch state, destroys data
    /// that existed before the call: deleting glyphs or a kerning group
    /// always does; replacing a layer does when the layer has contours, and
    /// replacing components does when the layer has components, provided
    /// the glyph existed before the call (a glyph the call created holds
    /// nothing older than the call); a transform with a singular matrix
    /// (`|a·d − b·c| < 1e-9`) always does, since it collapses the contour;
    /// setting a kerning pair to 0 does when the pair is there and was there
    /// before the call.
    fn destroys(&self, cmd: &Command) -> bool {
        match cmd {
            Command::DeleteGlyphs { .. } | Command::DeleteKerningGroup { .. } => true,
            Command::ContourEdit {
                action: ContourAction::Transform { affine, .. },
                ..
            } => {
                let [a, b, c, d, _, _] = *affine;
                let determinant = (a * d - b * c).abs();
                // A NaN determinant counts as singular too.
                determinant.is_nan() || determinant < SINGULAR_DETERMINANT
            }
            Command::SetOutline { glyph, master, .. } => self
                .older_layer(glyph, *master)
                .is_some_and(|layer| !layer.paths.is_empty()),
            Command::SetComponents { glyph, master, .. } => self
                .older_layer(glyph, *master)
                .is_some_and(|layer| !layer.components.is_empty()),
            Command::SetKerningPair {
                master,
                first,
                second,
                value,
            } if *value == 0.0 => {
                let font = &self.after;
                let master = master.or_else(|| font.masters.first().map(|master| master.id));
                let first = first.resolve(font, KernSide::First);
                let second = second.resolve(font, KernSide::Second);
                let (Some(master), Some(first), Some(second)) = (master, first, second) else {
                    return false;
                };
                font.kerning.pair(master, first, second).is_some()
                    && self.before.kerning.pair(master, first, second).is_some()
            }
            _ => false,
        }
    }

    /// The layer in `master` (the default one when `None`) of the glyph
    /// `glyph` in the current scratch state, if the glyph existed before the
    /// call.
    fn older_layer(&self, glyph: &GlyphRef, master: Option<MasterId>) -> Option<&Layer> {
        let font = &self.after;
        let id = glyph.resolve(font)?;
        if !self.before.glyphs.contains_key(&id) {
            return None;
        }
        let master = master.or_else(|| font.masters.first().map(|master| master.id))?;
        font.glyphs.get(&id)?.layers.get(&master)
    }

    /// Whether going from the current scratch state to `next` (the touched
    /// entities being `touched`) leaves without contours a layer that had
    /// contours both when the call started and right before this step, in a
    /// glyph that existed before the call. Deleting the glyph counts too.
    fn empties_drawn_layer(&self, next: &Font, touched: &BTreeSet<EntityKey>) -> bool {
        let drawn = |font: &Font, id: &GlyphId, master| {
            font.glyphs
                .get(id)
                .and_then(|glyph| glyph.layers.get(master))
                .is_some_and(|layer| !layer.paths.is_empty())
        };
        touched.iter().any(|key| {
            let EntityKey::Glyph(id) = key else {
                return false;
            };
            let Some(start) = self.before.glyphs.get(id) else {
                return false;
            };
            start
                .layers
                .iter()
                .filter(|(_, layer)| !layer.paths.is_empty())
                .any(|(master, _)| drawn(&self.after, id, master) && !drawn(next, id, master))
        })
    }
}

#[cfg(test)]
mod tests {
    use forma_edit::{ComponentInput, ContourAction, KernRefInput};
    use forma_model::{Node, NodeKind, Path, PathId, Transform};

    use super::*;

    fn triangle() -> Path {
        let node = |x: f64, y: f64| Node {
            x,
            y,
            kind: NodeKind::Line,
            smooth: false,
        };
        Path {
            id: PathId(0),
            closed: true,
            nodes: vec![node(0.0, 0.0), node(10.0, 0.0), node(10.0, 10.0)],
        }
    }

    fn create(name: &str, paths: Vec<Path>) -> Command {
        Command::CreateGlyph {
            name: name.into(),
            unicodes: Vec::new(),
            advance_width: 100.0,
            paths,
        }
    }

    fn font_with(name: &str, paths: Vec<Path>) -> Arc<Font> {
        let font = Font::new("Test", 1000);
        Arc::new(apply(&font, &create(name, paths)).expect("create").0)
    }

    fn by_name(name: &str) -> GlyphRef {
        GlyphRef::Name(name.into())
    }

    fn set_outline(name: &str) -> Command {
        Command::SetOutline {
            glyph: by_name(name),
            master: None,
            paths: vec![triangle()],
        }
    }

    fn rename(from: &str, to: &str) -> Command {
        Command::UpdateGlyph {
            glyph: by_name(from),
            name: Some(to.into()),
            unicodes: None,
            export: None,
        }
    }

    fn advance(name: &str, width: f64) -> Command {
        Command::SetMetrics {
            glyph: by_name(name),
            master: None,
            advance_width: Some(width),
            lsb: None,
            rsb: None,
        }
    }

    fn effects(font: &Arc<Font>, cmds: Vec<Command>) -> Effects {
        Steps::of(font, cmds).expect("valid commands").effects()
    }

    #[test]
    fn replacing_a_drawn_layer_or_deleting_is_destructive() {
        let empty = font_with("a", Vec::new());
        let drawn = font_with("a", vec![triangle()]);
        assert_eq!(effects(&empty, vec![set_outline("a")]), Effects::WRITE);
        assert_eq!(
            effects(&drawn, vec![set_outline("a")]),
            Effects::DESTRUCTIVE
        );
        let delete = Command::DeleteGlyphs {
            glyphs: vec![by_name("a")],
        };
        assert_eq!(effects(&empty, vec![delete]), Effects::DESTRUCTIVE);
        assert_eq!(effects(&drawn, vec![rename("a", "b")]), Effects::WRITE);
    }

    #[test]
    fn effects_are_judged_on_the_state_each_command_meets() {
        let drawn = font_with("a", vec![triangle()]);
        // Renaming first does not hide that the outline of `a` is replaced.
        assert_eq!(
            effects(&drawn, vec![rename("a", "b"), set_outline("b")]),
            Effects::DESTRUCTIVE
        );
        // A glyph created by the call holds nothing older than the call, even
        // when it takes the name of a glyph the call renamed.
        assert_eq!(
            effects(
                &drawn,
                vec![
                    rename("a", "b"),
                    create("a", vec![triangle()]),
                    set_outline("a")
                ]
            ),
            Effects::WRITE
        );
        // Deleting stays destructive when a new glyph takes the name.
        let delete = Command::DeleteGlyphs {
            glyphs: vec![by_name("a")],
        };
        assert_eq!(
            effects(&drawn, vec![delete, create("a", Vec::new())]),
            Effects::DESTRUCTIVE
        );
    }

    fn contour(glyph: &str, action: ContourAction) -> Command {
        Command::ContourEdit {
            glyph: by_name(glyph),
            master: None,
            action,
        }
    }

    fn delete_contour(glyph: &str, id: u32) -> Command {
        contour(
            glyph,
            ContourAction::Delete {
                path_id: PathId(id),
            },
        )
    }

    fn transform(glyph: &str, affine: [f64; 6]) -> Command {
        contour(
            glyph,
            ContourAction::Transform {
                path_id: PathId(0),
                affine,
            },
        )
    }

    #[test]
    fn removing_the_last_contour_of_a_drawn_layer_is_destructive() {
        let drawn = font_with("a", vec![triangle()]);
        assert_eq!(
            effects(&drawn, vec![delete_contour("a", 0)]),
            Effects::DESTRUCTIVE
        );
        // Drawing the layer again later in the call does not hide it.
        assert_eq!(
            effects(&drawn, vec![delete_contour("a", 0), set_outline("a")]),
            Effects::DESTRUCTIVE
        );
        // Removing one of two contours is not; removing the second is.
        let mut second = triangle();
        second.id = PathId(1);
        let two = font_with("a", vec![triangle(), second]);
        assert_eq!(effects(&two, vec![delete_contour("a", 0)]), Effects::WRITE);
        assert_eq!(
            effects(&two, vec![delete_contour("a", 0), delete_contour("a", 1)]),
            Effects::DESTRUCTIVE
        );
        // A glyph the call created holds nothing older than the call.
        let empty = Arc::new(Font::new("Test", 1000));
        assert_eq!(
            effects(
                &empty,
                vec![create("b", vec![triangle()]), delete_contour("b", 0)]
            ),
            Effects::WRITE
        );
    }

    #[test]
    fn a_singular_transform_is_destructive() {
        let drawn = font_with("a", vec![triangle()]);
        for affine in [
            [1.0, 0.0, 0.0, 0.0, 0.0, 0.0],
            [2.0, 4.0, 1.0, 2.0, 5.0, 5.0],
            [1e-6, 0.0, 0.0, 1e-6, 0.0, 0.0],
            [0.0; 6],
        ] {
            assert_eq!(
                effects(&drawn, vec![transform("a", affine)]),
                Effects::DESTRUCTIVE,
                "{affine:?}"
            );
        }
        for affine in [
            [1.0, 0.0, 0.0, 1.0, 10.0, 0.0],
            [0.0, 1.0, -1.0, 0.0, 0.0, 0.0],
            [-1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
        ] {
            assert_eq!(
                effects(&drawn, vec![transform("a", affine)]),
                Effects::WRITE,
                "{affine:?}"
            );
        }
    }

    #[test]
    fn touched_entities_are_recorded() {
        let drawn = font_with("a", vec![triangle()]);
        let a = drawn.glyph_by_name("a").expect("a").id;
        let steps = Steps::of(&drawn, vec![advance("a", 200.0), rename("a", "b")]).expect("valid");
        assert_eq!(steps.touched, BTreeSet::from([EntityKey::Glyph(a)]));
    }

    #[test]
    fn affected_glyphs_are_counted_once_each() {
        let drawn = font_with("a", vec![triangle()]);
        let a = drawn.glyph_by_name("a").expect("a").id;
        let steps = Steps::of(
            &drawn,
            vec![
                advance("a", 200.0),
                advance("a", 300.0),
                rename("a", "b"),
                advance("b", 400.0),
                create("c", Vec::new()),
                set_outline("c"),
                Command::SetFontInfo {
                    family_name: Some("New".into()),
                    style_name: None,
                    version_major: None,
                    version_minor: None,
                    copyright: None,
                    designer: None,
                    units_per_em: None,
                },
            ],
        )
        .expect("valid commands");
        let c = steps.after.glyph_by_name("c").expect("c").id;
        assert_eq!(
            steps.affected,
            BTreeSet::from([EntityKey::Glyph(a), EntityKey::Glyph(c)])
        );
        assert_eq!(steps.cmds.len(), 7);
        assert_eq!(steps.effects(), Effects::WRITE);
        assert!(Arc::ptr_eq(&steps.before, &drawn));
    }

    fn kern(first: &str, second: &str, value: f64) -> Command {
        Command::SetKerningPair {
            master: None,
            first: KernRefInput::Glyph(by_name(first)),
            second: KernRefInput::Glyph(by_name(second)),
            value,
        }
    }

    /// A font with the empty glyphs `g0` to `g{count - 1}`.
    fn glyphs(count: usize) -> Arc<Font> {
        let mut font = Font::new("Test", 1000);
        for index in 0..count {
            font = apply(&font, &create(&format!("g{index}"), Vec::new()))
                .expect("create")
                .0;
        }
        Arc::new(font)
    }

    #[test]
    fn removing_a_pair_is_destructive() {
        let font = glyphs(2);
        let kerned = Arc::new(apply(&font, &kern("g0", "g1", -50.0)).expect("kern").0);
        assert_eq!(
            effects(&kerned, vec![kern("g0", "g1", 0.0)]),
            Effects::DESTRUCTIVE
        );
        // Changing its value, or removing a pair that is not there, is not.
        assert_eq!(
            effects(&kerned, vec![kern("g0", "g1", -60.0)]),
            Effects::WRITE
        );
        assert_eq!(
            effects(&kerned, vec![kern("g1", "g0", 0.0)]),
            Effects::WRITE
        );
        // A pair the call added holds nothing older than the call.
        assert_eq!(
            effects(&font, vec![kern("g0", "g1", -50.0), kern("g0", "g1", 0.0)]),
            Effects::WRITE
        );
        // Deleting a group always is.
        let group = |members: Vec<GlyphRef>| Command::SetKerningGroup {
            name: "O".into(),
            side: KernSide::First,
            members,
        };
        let grouped = Arc::new(apply(&font, &group(vec![by_name("g0")])).expect("group").0);
        let delete = Command::DeleteKerningGroup {
            name: "O".into(),
            side: KernSide::First,
        };
        assert_eq!(effects(&grouped, vec![delete]), Effects::DESTRUCTIVE);
        assert_eq!(
            effects(&grouped, vec![group(vec![by_name("g1")])]),
            Effects::WRITE
        );
    }

    #[test]
    fn twenty_one_pairs_is_mass() {
        let font = glyphs(22);
        let cmds: Vec<Command> = (0..21)
            .map(|index| kern(&format!("g{index}"), &format!("g{}", index + 1), -10.0))
            .collect();
        let steps = Steps::of(&font, cmds).expect("valid commands");
        assert_eq!(steps.affected.len(), 21);
        assert!(steps.affected.len() > crate::limits::MASS_THRESHOLD);
        assert!(
            steps
                .affected
                .iter()
                .all(|key| matches!(key, EntityKey::KerningPair(..)))
        );
        assert_eq!(steps.effects(), Effects::WRITE);
    }

    #[test]
    fn replacing_components_is_destructive() {
        let font = glyphs(3);
        let set = |glyph: &str, bases: &[&str]| Command::SetComponents {
            glyph: by_name(glyph),
            master: None,
            components: bases
                .iter()
                .map(|base| ComponentInput {
                    base: by_name(base),
                    transform: Transform::IDENTITY,
                })
                .collect(),
        };
        // Into an empty list: not destructive.
        assert_eq!(effects(&font, vec![set("g2", &["g0"])]), Effects::WRITE);
        let composite = Arc::new(apply(&font, &set("g2", &["g0"])).expect("set").0);
        for replacement in [&["g1"][..], &[], &["g0", "g1"]] {
            assert_eq!(
                effects(&composite, vec![set("g2", replacement)]),
                Effects::DESTRUCTIVE,
                "{replacement:?}"
            );
        }
        // Decomposing and removing overlaps are not.
        let decompose = Command::Decompose {
            glyphs: vec![by_name("g2")],
            master: None,
        };
        let overlap = Command::RemoveOverlap {
            glyphs: vec![by_name("g2")],
            master: None,
        };
        assert_eq!(
            effects(&composite, vec![decompose, overlap]),
            Effects::WRITE
        );
    }

    #[test]
    fn remove_overlap_counts_the_nodes_of_its_glyphs() {
        let mut second = triangle();
        second.id = PathId(1);
        let font = font_with("a", vec![triangle(), second]);
        let font = Arc::new(apply(&font, &create("b", vec![triangle()])).expect("b").0);
        let overlap = |names: &[&str]| Command::RemoveOverlap {
            glyphs: names.iter().map(|name| by_name(name)).collect(),
            master: None,
        };
        assert_eq!(command_nodes(&font, &overlap(&["a"])), 6);
        assert_eq!(command_nodes(&font, &overlap(&["a", "b", "zed"])), 9);
        assert_eq!(command_nodes(&font, &kern("a", "b", -5.0)), 0);
        assert_eq!(command_nodes(&font, &set_outline("a")), 3);
    }

    #[test]
    fn a_refused_command_leaves_the_steps_unchanged() {
        let drawn = font_with("a", vec![triangle()]);
        let mut steps = Steps::new(&drawn);
        steps.push(advance("a", 200.0)).expect("valid");
        let error = steps.push(advance("zed", 1.0)).expect_err("unknown glyph");
        assert_eq!(error.code, crate::errors::ErrorCode::NotFound);
        let error = steps
            .push(Command::DeleteGlyphs { glyphs: Vec::new() })
            .expect_err("empty list");
        assert_eq!(error.code, crate::errors::ErrorCode::InvalidArgument);
        assert_eq!(steps.cmds.len(), 1);
        assert_eq!(steps.effects(), Effects::WRITE);
        assert_eq!(steps.affected.len(), 1);
    }
}
