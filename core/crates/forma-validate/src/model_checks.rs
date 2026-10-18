//! Checks on the font document itself: geometry, Unicode coverage and
//! vertical metrics. They run on a font that satisfies the model invariants.

use forma_geometry::{IssueKind, check_paths};
use forma_model::{Font, Glyph, GlyphId, KernRef, MasterId, Review, Source};

use crate::{Finding, Severity};

/// The grid, in font units, that coordinates are expected to sit on.
const GRID: f64 = 1.0;

/// The glyph that is allowed to have no code point.
const NOTDEF: &str = ".notdef";

/// Runs every model check.
pub(crate) fn check_model(font: &Font) -> Vec<Finding> {
    let mut findings = check_geometry(font);
    findings.extend(check_unicode(font));
    findings.extend(check_vertical_metrics(font));
    findings.extend(check_mixed_glyphs(font));
    findings.extend(check_kerning_references(font));
    findings.extend(check_review(font));
    findings
}

/// The review state of generated glyphs:
///
/// * `glyph.pending_review` (warning): a traced or synthesized glyph that no
///   person has approved yet;
/// * `glyph.approval_stale` (warning): an approved glyph whose design no
///   longer matches the approved one (changed outside FORMA's editing, for
///   example by merging a `.formapackage` with git);
/// * `glyph.rejected_used_by_component` (error): an exported glyph draws a
///   rejected glyph through a component.
fn check_review(font: &Font) -> Vec<Finding> {
    let mut findings = Vec::new();
    for glyph in exported_glyphs(font) {
        if let Some(provenance) = &glyph.provenance {
            let generated = matches!(provenance.source, Source::Traced | Source::Synthesized);
            if generated && provenance.review == Review::Pending {
                findings.push(Finding::new(
                    "glyph.pending_review",
                    Severity::Warning,
                    Some(&glyph.name),
                    format!(
                        "the {} glyph has not been reviewed; approve or reject it with \
                         glyph_review",
                        source_word(provenance.source)
                    ),
                ));
            }
            if provenance.review == Review::Approved
                && provenance.approved_geometry.as_deref()
                    != Some(forma_geometry::design_hash(font, glyph).as_str())
            {
                findings.push(Finding::new(
                    "glyph.approval_stale",
                    Severity::Warning,
                    Some(&glyph.name),
                    "the design changed since it was approved; review it again".to_owned(),
                ));
            }
        }
        let rejected_bases: Vec<&str> = glyph
            .layers
            .values()
            .flat_map(|layer| &layer.components)
            .filter_map(|component| font.glyphs.get(&component.base))
            .filter(|base| {
                base.provenance
                    .as_ref()
                    .is_some_and(|p| p.review == Review::Rejected)
            })
            .map(|base| base.name.as_str())
            .collect();
        for base in rejected_bases {
            findings.push(Finding::new(
                "glyph.rejected_used_by_component",
                Severity::Error,
                Some(&glyph.name),
                format!("the glyph uses the rejected glyph {base:?} as a component"),
            ));
        }
    }
    findings
}

fn source_word(source: Source) -> &'static str {
    match source {
        Source::Traced => "traced",
        Source::Synthesized => "synthesized",
        Source::Composed => "composed",
        _ => "hand-drawn",
    }
}

/// The glyphs marked for export, in glyph order. The rest are left out of the
/// binary and are therefore not checked.
pub(crate) fn exported_glyphs(font: &Font) -> impl Iterator<Item = &Glyph> {
    font.glyph_order
        .iter()
        .filter_map(|id| font.glyphs.get(id))
        .map(|glyph| &**glyph)
        .filter(|glyph| glyph.is_exported())
}

/// The check id of a geometry issue kind.
///
/// These strings are a contract with whoever reads the report, so they are
/// spelled out instead of derived. A kind added to `forma-geometry` later is
/// reported as `geometry.other` until it is added here.
fn geometry_check_id(kind: IssueKind) -> &'static str {
    match kind {
        IssueKind::OpenContour => "geometry.open_contour",
        IssueKind::WrongDirection => "geometry.wrong_direction",
        IssueKind::DuplicatePoints => "geometry.duplicate_points",
        IssueKind::TinySegment => "geometry.tiny_segment",
        IssueKind::OffGrid => "geometry.off_grid",
        IssueKind::MissingExtrema => "geometry.missing_extrema",
        IssueKind::InvalidContour => "geometry.invalid_contour",
        _ => "geometry.other",
    }
}

/// Contours that are open, run the wrong way or cannot be read make the font
/// wrong; everything else is a quality remark.
fn geometry_severity(kind: IssueKind) -> Severity {
    match kind {
        IssueKind::OpenContour | IssueKind::WrongDirection | IssueKind::InvalidContour => {
            Severity::Error
        }
        _ => Severity::Warning,
    }
}

/// Geometry of every master layer of every exported glyph.
fn check_geometry(font: &Font) -> Vec<Finding> {
    let several_masters = font.masters.len() > 1;
    let mut findings = Vec::new();
    for glyph in exported_glyphs(font) {
        for (master, layer) in &glyph.layers {
            let prefix = if several_masters {
                format!("master {:?}: ", master_name(font, *master))
            } else {
                String::new()
            };
            for issue in check_paths(&layer.paths, GRID) {
                findings.push(Finding::new(
                    geometry_check_id(issue.kind),
                    geometry_severity(issue.kind),
                    Some(&glyph.name),
                    format!("{prefix}contour {}: {}", issue.path.0, issue.message),
                ));
            }
        }
    }
    findings
}

/// The name of a master, or its id if the font does not have it.
fn master_name(font: &Font, id: MasterId) -> String {
    font.masters
        .iter()
        .find(|master| master.id == id)
        .map_or_else(|| id.to_string(), |master| master.name.clone())
}

/// Exported glyphs that no code point leads to. That is normal for alternates
/// and ligatures, so it is only information.
fn check_unicode(font: &Font) -> Vec<Finding> {
    exported_glyphs(font)
        .filter(|glyph| glyph.name != NOTDEF && glyph.unicodes.is_empty())
        .map(|glyph| {
            Finding::new(
                "unicode.missing",
                Severity::Info,
                Some(&glyph.name),
                "the glyph has no Unicode code point",
            )
        })
        .collect()
}

/// Masters whose ascender is not above their descender.
fn check_vertical_metrics(font: &Font) -> Vec<Finding> {
    let several_masters = font.masters.len() > 1;
    font.masters
        .iter()
        .filter(|master| master.metrics.ascender <= master.metrics.descender)
        .map(|master| {
            let prefix = if several_masters {
                format!("master {:?}: ", master.name)
            } else {
                String::new()
            };
            Finding::new(
                "metrics.vertical",
                Severity::Error,
                None,
                format!(
                    "{prefix}the ascender {} must be above the descender {}",
                    master.metrics.ascender, master.metrics.descender
                ),
            )
        })
        .collect()
}

/// Exported glyphs with contours of their own and components in the same
/// layer. TrueType cannot hold a simple part in a composite glyph, so such a
/// glyph is rarely what was intended.
fn check_mixed_glyphs(font: &Font) -> Vec<Finding> {
    let several_masters = font.masters.len() > 1;
    let mut findings = Vec::new();
    for glyph in exported_glyphs(font) {
        for (master, layer) in &glyph.layers {
            if layer.paths.is_empty() || layer.components.is_empty() {
                continue;
            }
            let prefix = if several_masters {
                format!("master {:?}: ", master_name(font, *master))
            } else {
                String::new()
            };
            findings.push(Finding::new(
                "glyph.mixed_contours_components",
                Severity::Warning,
                Some(&glyph.name),
                format!(
                    "{prefix}the layer has {} contour(s) and {} component(s); \
                     a glyph should be made of one or the other",
                    layer.paths.len(),
                    layer.components.len()
                ),
            ));
        }
    }
    findings
}

/// Kerning that mentions glyphs which are not exported: they are left out of
/// the binary, so their pairs and group memberships are dropped from it.
fn check_kerning_references(font: &Font) -> Vec<Finding> {
    let unexported = |id: GlyphId| {
        font.glyphs
            .get(&id)
            .filter(|glyph| !glyph.is_exported())
            .map(|glyph| glyph.name.as_str())
    };
    let mut findings = Vec::new();
    let mut warn = |glyph: &str, message: String| {
        findings.push(Finding::new(
            "kerning.unexported_reference",
            Severity::Warning,
            Some(glyph),
            message,
        ));
    };
    for group in font.kerning.groups.values() {
        for name in group.members.iter().filter_map(|&id| unexported(id)) {
            warn(
                name,
                format!(
                    "the glyph is in the kerning group {:?} but is not exported",
                    group.name
                ),
            );
        }
    }
    for (master, pairs) in font.kerning.pairs.iter() {
        for &(first, second) in pairs.keys() {
            for reference in [first, second] {
                if let KernRef::Glyph(id) = reference
                    && let Some(name) = unexported(id)
                {
                    warn(
                        name,
                        format!(
                            "a kerning pair of master {:?} uses the glyph, which is not exported",
                            master_name(font, *master)
                        ),
                    );
                }
            }
        }
    }
    findings
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_ids_and_severities_are_stable() {
        let expected = [
            (
                IssueKind::OpenContour,
                "geometry.open_contour",
                Severity::Error,
            ),
            (
                IssueKind::WrongDirection,
                "geometry.wrong_direction",
                Severity::Error,
            ),
            (
                IssueKind::DuplicatePoints,
                "geometry.duplicate_points",
                Severity::Warning,
            ),
            (
                IssueKind::TinySegment,
                "geometry.tiny_segment",
                Severity::Warning,
            ),
            (IssueKind::OffGrid, "geometry.off_grid", Severity::Warning),
            (
                IssueKind::MissingExtrema,
                "geometry.missing_extrema",
                Severity::Warning,
            ),
            (
                IssueKind::InvalidContour,
                "geometry.invalid_contour",
                Severity::Error,
            ),
        ];
        for (kind, id, severity) in expected {
            assert_eq!(geometry_check_id(kind), id);
            assert_eq!(geometry_severity(kind), severity, "{id}");
        }
    }
}
