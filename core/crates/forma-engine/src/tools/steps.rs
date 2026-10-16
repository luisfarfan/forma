//! `Steps`: the commands of one call, measured on a scratch copy of the
//! document's font before the transaction is committed.

use std::collections::BTreeSet;
use std::sync::Arc;

use forma_edit::{Command, apply};
use forma_model::{EntityKey, Font, GlyphId};

use crate::errors::{ToolError, from_edit_error};
use crate::registry::Effects;
use crate::tools::limits::{check_nodes, command_nodes};

/// The commands of one call, applied in order to a scratch copy of the
/// document's font as they are added.
///
/// Applying them measures, before anything is committed, what the
/// transaction will do: the state after each command, the distinct glyphs
/// the commands touch and whether any of them destroys data. Each command is
/// judged against the state the previous ones left (a glyph renamed by one
/// command and replaced by the next is the same glyph).
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
    /// The distinct glyphs the commands touch, by their id in `after`
    /// (deleted glyphs by their id in `before`).
    pub affected: BTreeSet<GlyphId>,
    /// Whether some command, at its turn, deletes glyphs or replaces the
    /// non-empty layer of a glyph that existed before the call.
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
        let nodes = self.nodes.saturating_add(command_nodes(&cmd));
        check_nodes(nodes)?;
        let destructive = self.destroys(&cmd);
        let (after, touched) = apply(&self.after, &cmd).map_err(from_edit_error)?;
        self.affected
            .extend(touched.into_iter().filter_map(|key| match key {
                EntityKey::Glyph(id) => Some(id),
                _ => None,
            }));
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
    /// that existed before the call: deleting glyphs always does; replacing
    /// a layer does when the layer has contours and its glyph existed before
    /// the call (a glyph the call created holds nothing older than the call).
    fn destroys(&self, cmd: &Command) -> bool {
        match cmd {
            Command::DeleteGlyphs { .. } => true,
            Command::SetOutline { glyph, master, .. } => {
                let font = &self.after;
                let Some(id) = glyph.resolve(font) else {
                    return false;
                };
                if !self.before.glyphs.contains_key(&id) {
                    return false;
                }
                let master = master.or_else(|| font.masters.first().map(|master| master.id));
                font.glyphs
                    .get(&id)
                    .zip(master)
                    .and_then(|(glyph, master)| glyph.layers.get(&master))
                    .is_some_and(|layer| !layer.paths.is_empty())
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use forma_edit::GlyphRef;
    use forma_model::{Node, NodeKind, Path, PathId};

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
        assert_eq!(steps.affected, BTreeSet::from([a, c]));
        assert_eq!(steps.cmds.len(), 7);
        assert_eq!(steps.effects(), Effects::WRITE);
        assert!(Arc::ptr_eq(&steps.before, &drawn));
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
