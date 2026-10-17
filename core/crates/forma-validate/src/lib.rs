//! Validation for FORMA: a machine-readable report on a font document.
//!
//! [`validate`] checks the model first (invariants, outline geometry, Unicode
//! coverage, vertical metrics), then compiles the font to TrueType with
//! [`forma_compile`] and checks the binary against the model with read-fonts.
//! The result is a [`ValidationReport`] whose findings carry stable check
//! identifiers, so that a tool or an AI agent can tell exactly what is wrong
//! and where.
//!
//! # Checks
//!
//! | Check id | Severity | What it reports |
//! | --- | --- | --- |
//! | `invariants` | error | the font breaks a model rule; nothing else is checked |
//! | `geometry.<kind>` | error or warning | a [`forma_geometry::IssueKind`] in an exported glyph |
//! | `unicode.missing` | info | an exported glyph has no code point |
//! | `metrics.vertical` | error | a master's ascender is not above its descender |
//! | `compile` | error | the font cannot be compiled; no binary checks run |
//! | `binary.tables` | error | a required table or a glyph is missing, or an extra glyph is present |
//! | `binary.cmap` | error | the `cmap` differs from the code points of the model |
//! | `binary.hmtx` | error | an advance width differs from the model's |
//! | `binary.notdef` | error | glyph 0 is not `.notdef` |
//!
//! The geometry kinds that are errors are `open_contour`, `wrong_direction`
//! and `invalid_contour`; the others (`duplicate_points`, `tiny_segment`,
//! `off_grid`, `missing_extrema`) are warnings.

#![forbid(unsafe_code)]

mod binary_checks;
mod model_checks;

use forma_compile::{OutputFormat, compile, preflight};
use forma_model::Font;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub use binary_checks::check_binary;

/// How serious a finding is.
///
/// The variants are declared from most to least serious, and findings are
/// sorted by that order.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// The font is wrong or cannot be exported correctly.
    Error,
    /// The font works but has a quality problem.
    Warning,
    /// A remark; it is listed in the report but not counted.
    Info,
}

/// One result of a check.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Finding {
    /// The stable identifier of the check that produced the finding, such as
    /// `geometry.wrong_direction` or `binary.hmtx`.
    pub check: String,
    /// How serious it is.
    pub severity: Severity,
    /// The name of the glyph the finding is about, when it is about one.
    pub glyph: Option<String>,
    /// A human-readable description. In a font with several masters it names
    /// the master a geometry finding was found in.
    pub message: String,
}

impl Finding {
    pub(crate) fn new(
        check: impl Into<String>,
        severity: Severity,
        glyph: Option<&str>,
        message: impl Into<String>,
    ) -> Finding {
        Finding {
            check: check.into(),
            severity,
            glyph: glyph.map(str::to_owned),
            message: message.into(),
        }
    }
}

/// Sorts findings by severity (errors first), then check id, then glyph name
/// (findings without a glyph first), then message. The last key only breaks
/// ties, so that the order never depends on the order the checks ran in.
pub(crate) fn sort_findings(findings: &mut [Finding]) {
    findings.sort_by(|a, b| {
        (a.severity, &a.check, &a.glyph, &a.message)
            .cmp(&(b.severity, &b.check, &b.glyph, &b.message))
    });
}

/// The outcome of [`validate`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ValidationReport {
    /// Every finding, sorted by severity (errors first), check id and glyph
    /// name.
    pub findings: Vec<Finding>,
    /// How many findings have [`Severity::Error`]. A font is fit to export
    /// when this is zero.
    pub errors: usize,
    /// How many findings have [`Severity::Warning`].
    pub warnings: usize,
    /// Whether the font compiled to a binary, which is what the `binary.*`
    /// checks ran on. `false` when the model is invalid or the compiler failed.
    pub compiled: bool,
}

impl ValidationReport {
    /// Builds a report from `findings`, sorting them and counting the errors
    /// and warnings (informational findings are not counted).
    #[must_use]
    pub fn new(mut findings: Vec<Finding>, compiled: bool) -> ValidationReport {
        sort_findings(&mut findings);
        let count = |severity| {
            findings
                .iter()
                .filter(|finding| finding.severity == severity)
                .count()
        };
        ValidationReport {
            errors: count(Severity::Error),
            warnings: count(Severity::Warning),
            findings,
            compiled,
        }
    }
}

/// Validates `font`: the model first, then the binary it compiles to.
///
/// 1. If the model invariants ([`Font::validate_invariants`]) do not hold, the
///    report has that one `invariants` error and nothing else is checked.
/// 2. Otherwise the geometry of every master layer of every exported glyph
///    (on a grid of 1.0), the Unicode coverage and the vertical metrics are
///    checked. Glyphs that are not exported are not checked.
/// 3. Then the font is compiled to TrueType. If that fails, the report gets a
///    `compile` error and `compiled` is `false`. If it succeeds, the binary
///    is compared with the font as the compiler sees it (see
///    [`check_binary`]) and `compiled` is `true`.
///
/// Compiling takes a moment; the model checks alone are cheap.
#[must_use]
pub fn validate(font: &Font) -> ValidationReport {
    if let Err(error) = font.validate_invariants() {
        let finding = Finding::new("invariants", Severity::Error, None, error.to_string());
        return ValidationReport::new(vec![finding], false);
    }

    let mut findings = model_checks::check_model(font);
    let compiled = match preflight(font) {
        Ok((preflighted, _)) => match compile(font, OutputFormat::Ttf) {
            Ok((binary, _)) => {
                findings.extend(check_binary(&preflighted, &binary));
                true
            }
            Err(error) => {
                findings.push(compile_failure(&error));
                false
            }
        },
        Err(error) => {
            findings.push(compile_failure(&error));
            false
        }
    };
    ValidationReport::new(findings, compiled)
}

fn compile_failure(error: &forma_compile::CompileError) -> Finding {
    Finding::new("compile", Severity::Error, None, error.to_string())
}
