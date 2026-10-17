//! Helpers shared by the document and history tests.

use forma_edit::{Command, ConflictInfo, Document, EditError, TxOutcome};
use forma_model::{Actor, EntityKey};

use super::*;

pub fn user() -> Actor {
    Actor::User
}

pub fn agent() -> Actor {
    Actor::Agent("claude".to_owned())
}

pub fn advance(name: &str, width: f64) -> Command {
    Command::SetMetrics {
        glyph: by_name(name),
        master: None,
        advance_width: Some(width),
        lsb: None,
        rsb: None,
    }
}

pub fn set_unicodes(name: &str, unicodes: &[u32]) -> Command {
    Command::UpdateGlyph {
        glyph: by_name(name),
        name: None,
        unicodes: Some(unicodes.to_vec()),
        export: None,
    }
}

pub fn rename(name: &str, to: &str) -> Command {
    Command::UpdateGlyph {
        glyph: by_name(name),
        name: Some(to.to_owned()),
        unicodes: None,
        export: None,
    }
}

pub fn delete(names: &[&str]) -> Command {
    Command::DeleteGlyphs {
        glyphs: names.iter().map(|name| by_name(name)).collect(),
    }
}

pub fn outline(name: &str, size: f64) -> Command {
    Command::SetOutline {
        glyph: by_name(name),
        master: None,
        paths: vec![square(0, 0.0, 0.0, size, size, true)],
    }
}

pub fn version_minor(minor: u16) -> Command {
    Command::SetFontInfo {
        family_name: None,
        style_name: None,
        version_major: None,
        version_minor: Some(minor),
        copyright: None,
        designer: None,
        units_per_em: None,
    }
}

pub fn units_per_em(units: u16) -> Command {
    Command::SetFontInfo {
        family_name: None,
        style_name: None,
        version_major: None,
        version_minor: None,
        copyright: None,
        designer: None,
        units_per_em: Some(units),
    }
}

pub fn x_height(value: f64) -> Command {
    Command::SetVerticalMetrics {
        master: None,
        ascender: None,
        descender: None,
        x_height: Some(value),
        cap_height: None,
    }
}

/// A document over `base_font()` (`A`, `B`) plus a glyph `C`, at revision 0.
pub fn new_doc() -> Document {
    let mut font = base_font();
    add_glyph(
        &mut font,
        "C",
        &[0x43],
        650.0,
        vec![square(0, 50.0, 0.0, 600.0, 700.0, true)],
    );
    Document::new(font)
}

/// Executes a transaction based on the current revision; it must succeed.
pub fn commit(doc: &mut Document, actor: &Actor, label: &str, cmds: Vec<Command>) -> TxOutcome {
    let base = doc.rev();
    doc.execute(actor, base, label, &cmds)
        .unwrap_or_else(|e| panic!("{label:?} failed: {e}"))
}

pub fn conflict(err: EditError) -> ConflictInfo {
    match err {
        EditError::Conflict(info) => info,
        other => panic!("expected a revision conflict, got {other:?}"),
    }
}

pub fn key(doc: &Document, name: &str) -> EntityKey {
    EntityKey::Glyph(id_of(doc.font(), name))
}

pub fn width(doc: &Document, name: &str) -> f64 {
    layer(doc.font(), name).advance_width
}

pub fn names_in_order(doc: &Document) -> Vec<String> {
    let font = doc.font();
    font.glyph_order
        .iter()
        .map(|id| font.glyphs[id].name.clone())
        .collect()
}

pub fn not_allowed(err: EditError) -> String {
    match err {
        EditError::NotAllowed(message) => message,
        other => panic!("expected NotAllowed, got {other:?}"),
    }
}
