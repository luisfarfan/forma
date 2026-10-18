//! The validation report: model checks, the compile step and the binary
//! checks.

mod common;

use common::{
    add_glyph, add_master, clean_font, clean_rect, default_layer, demo_font, edit_glyph, id_of,
    reverse, set_vertical_metrics,
};
use forma_compile::{OutputFormat, compile, preflight};
use forma_model::Font;
use forma_validate::{Finding, Severity, ValidationReport, check_binary, validate};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn named<'a>(report: &'a ValidationReport, check: &str) -> Vec<&'a Finding> {
    report
        .findings
        .iter()
        .filter(|f| f.check == check)
        .collect()
}

fn count(report: &ValidationReport, severity: Severity) -> usize {
    report
        .findings
        .iter()
        .filter(|f| f.severity == severity)
        .count()
}

/// `(severity, check, glyph)` of every finding, for comparing whole reports.
fn keys(report: &ValidationReport) -> Vec<(Severity, &str, Option<&str>)> {
    report
        .findings
        .iter()
        .map(|f| (f.severity, f.check.as_str(), f.glyph.as_deref()))
        .collect()
}

fn model_of(font: &Font) -> Font {
    preflight(font).expect("the font passes the preflight").0
}

fn binary_of(font: &Font) -> Vec<u8> {
    compile(font, OutputFormat::Ttf)
        .expect("the font compiles")
        .0
}

fn finding_for<'a>(findings: &'a [Finding], check: &str, glyph: &str) -> Option<&'a Finding> {
    findings
        .iter()
        .find(|f| f.check == check && f.glyph.as_deref() == Some(glyph))
}

/// A font with one problem of every severity, built so that its natural order
/// (glyph order, then check order) differs from the sorted one: wrong
/// direction in `B` then `A`, off-grid in `C`, no Unicode in `D`, and bad
/// vertical metrics.
fn font_with_every_severity() -> Font {
    let mut font = clean_font();
    for name in ["B", "A"] {
        add_glyph(
            &mut font,
            name,
            &[name.chars().next().map_or(0, |c| c as u32)],
            500.0,
            vec![clean_rect(0, 0, 0, 400, 700)],
        );
        let id = id_of(&font, name);
        default_layer(&mut font, id, |layer| reverse(&mut layer.paths[0]));
    }
    add_glyph(
        &mut font,
        "C",
        &['C' as u32],
        500.0,
        vec![clean_rect(0, 0, 0, 400, 700)],
    );
    let c = id_of(&font, "C");
    default_layer(&mut font, c, |layer| layer.paths[0].nodes[2].x = 400.5);
    add_glyph(
        &mut font,
        "D",
        &[],
        500.0,
        vec![clean_rect(0, 0, 0, 400, 700)],
    );
    set_vertical_metrics(&mut font, 100.0, 200.0);
    font
}

/// The bytes of an sfnt that declares no tables at all.
fn empty_sfnt() -> Vec<u8> {
    let mut bytes = vec![0, 1, 0, 0]; // sfnt version 1.0
    bytes.extend([0; 8]); // numTables, searchRange, entrySelector, rangeShift
    bytes
}

/// Points glyph `gid` of the `post` table of `binary` at the standard
/// Macintosh glyph name `name_index`.
fn set_post_name_index(binary: &mut [u8], gid: usize, name_index: u16) {
    let num_tables = usize::from(u16::from_be_bytes([binary[4], binary[5]]));
    let post = (0..num_tables)
        .map(|i| 12 + 16 * i)
        .find(|&record| &binary[record..record + 4] == b"post")
        .expect("the binary has a post table");
    let table = u32::from_be_bytes([
        binary[post + 8],
        binary[post + 9],
        binary[post + 10],
        binary[post + 11],
    ]) as usize;
    // Header (32 bytes), numGlyphs, then one u16 name index per glyph.
    let at = table + 34 + 2 * gid;
    binary[at..at + 2].copy_from_slice(&name_index.to_be_bytes());
}

// ---------------------------------------------------------------------------
// The report as a whole
// ---------------------------------------------------------------------------

#[test]
fn clean_demo_font_has_zero_errors() {
    let report = validate(&demo_font());
    assert_eq!(report.errors, 0, "{:#?}", report.findings);
    assert_eq!(count(&report, Severity::Error), 0);
    assert!(report.compiled);
    assert!(
        report
            .findings
            .iter()
            .all(|f| !f.check.starts_with("binary.")),
        "binary checks pass: {:#?}",
        report.findings
    );
}

#[test]
fn wrong_direction_is_error() {
    let mut font = demo_font();
    let h = id_of(&font, "H");
    default_layer(&mut font, h, |layer| reverse(&mut layer.paths[0]));

    let report = validate(&font);
    let found = named(&report, "geometry.wrong_direction");
    assert_eq!(found.len(), 1, "{:#?}", report.findings);
    assert_eq!(found[0].severity, Severity::Error);
    assert_eq!(found[0].glyph.as_deref(), Some("H"));
    assert_eq!(report.errors, 1);
    assert!(report.compiled, "the font still compiles");
}

#[test]
fn bad_vertical_metrics_is_error() {
    let mut font = demo_font();
    set_vertical_metrics(&mut font, 100.0, 200.0);
    let report = validate(&font);
    let found = named(&report, "metrics.vertical");
    assert_eq!(found.len(), 1, "{:#?}", report.findings);
    assert_eq!(found[0].severity, Severity::Error);
    assert_eq!(found[0].glyph, None);
    assert_eq!(report.errors, 1);

    // An ascender equal to the descender is incoherent too.
    set_vertical_metrics(&mut font, -50.0, -50.0);
    let report = validate(&font);
    assert_eq!(named(&report, "metrics.vertical").len(), 1);

    // A sane pair is not reported.
    set_vertical_metrics(&mut font, 800.0, -200.0);
    assert!(named(&validate(&font), "metrics.vertical").is_empty());
}

#[test]
fn bad_vertical_metrics_name_the_master() {
    let mut font = demo_font();
    let bold = add_master(&mut font, "Bold");
    let master = font
        .masters
        .iter_mut()
        .find(|m| m.id == bold)
        .expect("the new master");
    master.metrics.ascender = 0.0;
    master.metrics.descender = 10.0;

    let report = validate(&font);
    let found = named(&report, "metrics.vertical");
    assert_eq!(found.len(), 1, "{:#?}", report.findings);
    assert!(
        found[0].message.contains("\"Bold\""),
        "{}",
        found[0].message
    );
}

#[test]
fn report_counts_match_findings() {
    let report = validate(&font_with_every_severity());
    let errors = count(&report, Severity::Error);
    let warnings = count(&report, Severity::Warning);
    let infos = count(&report, Severity::Info);
    assert_eq!(report.errors, errors);
    assert_eq!(report.warnings, warnings);
    assert!(
        errors > 0 && warnings > 0 && infos > 0,
        "{:#?}",
        report.findings
    );
    // Info findings are listed but not counted.
    assert_eq!(report.findings.len(), errors + warnings + infos);
    assert!(report.errors + report.warnings < report.findings.len());
}

#[test]
fn invariant_failure_skips_compile() {
    let mut font = clean_font();
    let i = id_of(&font, "I");
    edit_glyph(&mut font, i, |glyph| glyph.unicodes = vec!['H' as u32]);
    let expected = font
        .validate_invariants()
        .expect_err("two glyphs share a code point")
        .to_string();

    let report = validate(&font);
    assert_eq!(
        report.findings,
        [Finding {
            check: "invariants".to_owned(),
            severity: Severity::Error,
            glyph: None,
            message: expected,
        }]
    );
    assert_eq!(report.errors, 1);
    assert_eq!(report.warnings, 0);
    assert!(!report.compiled);
}

#[test]
fn compile_failure_is_error() {
    let mut font = clean_font();
    let h = id_of(&font, "H");
    // The model is valid, but 70 000 does not fit an `hmtx` advance.
    default_layer(&mut font, h, |layer| layer.advance_width = 70_000.0);
    let expected = preflight(&font)
        .expect_err("the advance is out of range")
        .to_string();

    let report = validate(&font);
    let found = named(&report, "compile");
    assert_eq!(found.len(), 1, "{:#?}", report.findings);
    assert_eq!(found[0].severity, Severity::Error);
    assert_eq!(found[0].glyph, None);
    assert_eq!(found[0].message, expected);
    assert!(!report.compiled);
    assert!(
        report
            .findings
            .iter()
            .all(|f| !f.check.starts_with("binary.")),
        "no binary checks without a binary"
    );
}

#[test]
fn model_findings_survive_a_compile_failure() {
    let mut font = clean_font();
    let h = id_of(&font, "H");
    default_layer(&mut font, h, |layer| {
        layer.advance_width = 70_000.0;
        reverse(&mut layer.paths[0]);
    });
    let report = validate(&font);
    assert_eq!(named(&report, "geometry.wrong_direction").len(), 1);
    assert_eq!(named(&report, "compile").len(), 1);
    assert_eq!(report.errors, 2);
    assert!(!report.compiled);
}

// ---------------------------------------------------------------------------
// Model checks
// ---------------------------------------------------------------------------

#[test]
fn geometry_finding_names_glyph_and_kind() {
    let mut font = clean_font();
    let h = id_of(&font, "H");
    default_layer(&mut font, h, |layer| layer.paths[0].closed = false);

    let report = validate(&font);
    let found = named(&report, "geometry.open_contour");
    assert_eq!(found.len(), 1, "{:#?}", report.findings);
    assert_eq!(found[0].severity, Severity::Error);
    assert_eq!(found[0].glyph.as_deref(), Some("H"));
    // One master: the message does not name it.
    assert_eq!(found[0].message, "contour 0: contour is open");
}

#[test]
fn geometry_finding_names_the_master_of_a_multi_master_font() {
    let mut font = clean_font();
    let bold = add_master(&mut font, "Bold");
    let h = id_of(&font, "H");
    edit_glyph(&mut font, h, |glyph| {
        let layer = glyph.layers.get_mut(&bold).expect("Bold layer");
        layer.paths[0].closed = false;
    });

    let report = validate(&font);
    let found = named(&report, "geometry.open_contour");
    assert_eq!(found.len(), 1, "only the Bold layer is open");
    assert_eq!(found[0].glyph.as_deref(), Some("H"));
    assert_eq!(
        found[0].message,
        "master \"Bold\": contour 0: contour is open"
    );
}

#[test]
fn geometry_runs_on_every_master_layer() {
    let mut font = clean_font();
    let bold = add_master(&mut font, "Bold");
    let h = id_of(&font, "H");
    default_layer(&mut font, h, |layer| reverse(&mut layer.paths[0]));
    edit_glyph(&mut font, h, |glyph| {
        let layer = glyph.layers.get_mut(&bold).expect("Bold layer");
        reverse(&mut layer.paths[0]);
    });

    let report = validate(&font);
    let found = named(&report, "geometry.wrong_direction");
    assert_eq!(found.len(), 2, "{:#?}", report.findings);
    assert!(found.iter().any(|f| f.message.contains("\"Regular\"")));
    assert!(found.iter().any(|f| f.message.contains("\"Bold\"")));
}

#[test]
fn geometry_severities_follow_the_check() {
    let mut font = clean_font();
    let h = id_of(&font, "H");
    // Off the 1.0 grid: a warning.
    default_layer(&mut font, h, |layer| layer.paths[0].nodes[2].x = 550.5);
    let report = validate(&font);
    let found = named(&report, "geometry.off_grid");
    assert_eq!(found.len(), 1, "{:#?}", report.findings);
    assert_eq!(found[0].severity, Severity::Warning);
    assert_eq!(found[0].glyph.as_deref(), Some("H"));
    assert_eq!(report.warnings, 1);
    assert_eq!(report.errors, 0);
}

#[test]
fn missing_unicode_is_info_and_not_counted() {
    let mut font = clean_font();
    // A glyph without a code point, such as an alternate...
    add_glyph(
        &mut font,
        "H.alt",
        &[],
        600.0,
        vec![clean_rect(0, 50, 0, 550, 700)],
    );
    // ...but `.notdef` never has one, and is not reported.
    add_glyph(
        &mut font,
        ".notdef",
        &[],
        500.0,
        vec![clean_rect(0, 50, 0, 450, 700)],
    );

    let report = validate(&font);
    assert_eq!(
        report.findings.len(),
        1,
        "only the alternate: {:#?}",
        report.findings
    );
    let finding = &report.findings[0];
    assert_eq!(finding.check, "unicode.missing");
    assert_eq!(finding.severity, Severity::Info);
    assert_eq!(finding.glyph.as_deref(), Some("H.alt"));
    assert_eq!(report.errors, 0);
    assert_eq!(report.warnings, 0);
    assert!(report.compiled);
}

#[test]
fn non_exported_glyphs_are_not_checked() {
    let mut font = clean_font();
    // Everything wrong with it, but it is not exported: wrong direction, an
    // open contour, no Unicode value.
    add_glyph(
        &mut font,
        "scratch",
        &[],
        500.0,
        vec![
            clean_rect(0, 0, 0, 400, 700),
            clean_rect(1, 100, 100, 300, 600),
        ],
    );
    let scratch = id_of(&font, "scratch");
    default_layer(&mut font, scratch, |layer| {
        reverse(&mut layer.paths[0]);
        layer.paths[1].closed = false;
    });
    edit_glyph(&mut font, scratch, |glyph| glyph.export = false);
    // A glyph with a code point that is left out of the font: the binary's
    // `cmap` must not be expected to have it.
    add_glyph(
        &mut font,
        "draft",
        &['Z' as u32],
        500.0,
        vec![clean_rect(0, 0, 0, 400, 700)],
    );
    let draft = id_of(&font, "draft");
    edit_glyph(&mut font, draft, |glyph| glyph.export = false);

    let report = validate(&font);
    assert_eq!(report.findings, [], "nothing is reported");
    assert_eq!(report.errors, 0);
    assert!(report.compiled);
}

#[test]
fn findings_are_sorted_deterministically() {
    let report = validate(&font_with_every_severity());
    assert_eq!(
        keys(&report),
        [
            (Severity::Error, "geometry.wrong_direction", Some("A")),
            (Severity::Error, "geometry.wrong_direction", Some("B")),
            (Severity::Error, "metrics.vertical", None),
            (Severity::Warning, "geometry.off_grid", Some("C")),
            (Severity::Info, "unicode.missing", Some("D")),
        ],
        "{:#?}",
        report.findings
    );
    assert_eq!(validate(&font_with_every_severity()), report);
}

#[test]
fn report_new_sorts_and_counts() {
    let finding = |severity, check: &str, glyph: Option<&str>, message: &str| Finding {
        check: check.to_owned(),
        severity,
        glyph: glyph.map(str::to_owned),
        message: message.to_owned(),
    };
    let shuffled = vec![
        finding(Severity::Info, "unicode.missing", Some("b"), "x"),
        finding(Severity::Warning, "geometry.off_grid", Some("a"), "x"),
        finding(Severity::Error, "metrics.vertical", None, "x"),
        finding(
            Severity::Error,
            "geometry.wrong_direction",
            Some("z"),
            "two",
        ),
        finding(
            Severity::Error,
            "geometry.wrong_direction",
            Some("z"),
            "one",
        ),
        finding(Severity::Error, "geometry.wrong_direction", Some("a"), "x"),
        finding(Severity::Info, "unicode.missing", Some("a"), "x"),
    ];
    let mut reversed = shuffled.clone();
    reversed.reverse();

    let report = ValidationReport::new(shuffled, true);
    assert_eq!(report, ValidationReport::new(reversed, true));
    assert_eq!(report.errors, 4);
    assert_eq!(report.warnings, 1);
    assert!(report.compiled);
    assert_eq!(
        keys(&report),
        [
            (Severity::Error, "geometry.wrong_direction", Some("a")),
            (Severity::Error, "geometry.wrong_direction", Some("z")),
            (Severity::Error, "geometry.wrong_direction", Some("z")),
            (Severity::Error, "metrics.vertical", None),
            (Severity::Warning, "geometry.off_grid", Some("a")),
            (Severity::Info, "unicode.missing", Some("a")),
            (Severity::Info, "unicode.missing", Some("b")),
        ]
    );
    // Equal keys fall back to the message, so the order never depends on the
    // order the checks ran in.
    assert_eq!(report.findings[1].glyph.as_deref(), Some("z"));
    assert_eq!(report.findings[1].message, "one");
    assert_eq!(report.findings[2].message, "two");
}

#[test]
fn report_serializes_with_lowercase_severities() {
    let report = validate(&font_with_every_severity());
    let json = serde_json::to_value(&report).expect("serializes");
    assert_eq!(json["compiled"], true);
    assert_eq!(json["errors"], 3);
    assert_eq!(json["findings"][0]["severity"], "error");
    assert_eq!(json["findings"][0]["check"], "geometry.wrong_direction");
    assert_eq!(json["findings"][0]["glyph"], "A");
    assert_eq!(json["findings"][2]["glyph"], serde_json::Value::Null);
    let last = report.findings.len() - 1;
    assert_eq!(json["findings"][last]["severity"], "info");
    assert!(
        json["findings"]
            .as_array()
            .expect("findings array")
            .iter()
            .any(|f| f["severity"] == "warning")
    );

    let back: ValidationReport = serde_json::from_value(json).expect("deserializes");
    assert_eq!(back, report);

    // The schema is generated, not written by hand.
    let schema = serde_json::to_value(schemars::schema_for!(ValidationReport)).expect("schema");
    assert!(schema.to_string().contains("findings"));
}

// ---------------------------------------------------------------------------
// Binary checks
// ---------------------------------------------------------------------------

#[test]
fn binary_checks_pass_on_the_matching_binary() {
    let font = demo_font();
    assert_eq!(check_binary(&model_of(&font), &binary_of(&font)), []);
}

#[test]
fn several_and_non_bmp_code_points_validate_cleanly() {
    let mut font = clean_font();
    // Two code points for one glyph, one outside the Basic Multilingual Plane
    // (which needs a format 12 `cmap` subtable).
    add_glyph(&mut font, "space", &[0x20, 0xA0], 250.0, Vec::new());
    add_glyph(
        &mut font,
        "deseret",
        &[0x1_0400],
        500.0,
        vec![clean_rect(0, 50, 0, 450, 700)],
    );

    let report = validate(&font);
    assert!(report.compiled);
    assert_eq!(report.findings, [], "nothing to report");

    // The same font, with the non-BMP glyph encoded elsewhere, is caught.
    let mut other = font.clone();
    let deseret = id_of(&other, "deseret");
    edit_glyph(&mut other, deseret, |glyph| glyph.unicodes = vec![0x1_0401]);
    let findings = check_binary(&model_of(&font), &binary_of(&other));
    assert_eq!(findings.len(), 2, "{findings:#?}");
    assert!(findings.iter().all(|f| f.check == "binary.cmap"));
    assert!(findings.iter().any(|f| f.message.contains("U+10400")));
    assert!(findings.iter().any(|f| f.message.contains("U+10401")));
}

#[test]
fn binary_of_a_different_font_reports_hmtx_mismatches() {
    let model = demo_font();
    let mut other = demo_font();
    // `A` is 612.5 (613 rounded) in the model; `B` is 600.
    let a = id_of(&other, "A");
    default_layer(&mut other, a, |layer| layer.advance_width = 600.0);
    let b = id_of(&other, "B");
    default_layer(&mut other, b, |layer| layer.advance_width = 640.0);

    let findings = check_binary(&model_of(&model), &binary_of(&other));
    assert_eq!(findings.len(), 2, "{findings:#?}");
    for glyph in ["A", "B"] {
        let finding = finding_for(&findings, "binary.hmtx", glyph)
            .unwrap_or_else(|| panic!("an hmtx finding for {glyph}: {findings:#?}"));
        assert_eq!(finding.severity, Severity::Error);
    }
    let a = finding_for(&findings, "binary.hmtx", "A").expect("A");
    assert!(a.message.contains("600"), "{}", a.message);
    assert!(a.message.contains("613"), "{}", a.message);
}

#[test]
fn binary_cmap_mismatch_names_the_glyph() {
    let model = demo_font();
    let mut other = demo_font();
    // Swap the code points of `A` and `B`.
    let a = id_of(&other, "A");
    edit_glyph(&mut other, a, |glyph| glyph.unicodes = vec!['B' as u32]);
    let b = id_of(&other, "B");
    edit_glyph(&mut other, b, |glyph| glyph.unicodes = vec!['A' as u32]);

    let findings = check_binary(&model_of(&model), &binary_of(&other));
    assert_eq!(findings.len(), 2, "{findings:#?}");
    for glyph in ["A", "B"] {
        let finding = finding_for(&findings, "binary.cmap", glyph)
            .unwrap_or_else(|| panic!("a cmap finding for {glyph}: {findings:#?}"));
        assert_eq!(finding.severity, Severity::Error);
    }
}

#[test]
fn binary_cmap_reports_missing_and_unexpected_code_points() {
    let model = demo_font();
    let mut other = demo_font();
    // In the binary, `n` is encoded as `N` instead of `n`.
    let n = id_of(&other, "n");
    edit_glyph(&mut other, n, |glyph| glyph.unicodes = vec!['N' as u32]);

    let findings = check_binary(&model_of(&model), &binary_of(&other));
    assert_eq!(findings.len(), 2, "{findings:#?}");
    assert!(findings.iter().all(|f| f.check == "binary.cmap"));
    assert!(findings.iter().all(|f| f.glyph.as_deref() == Some("n")));
    assert!(findings.iter().any(|f| f.message.contains("U+006E")));
    assert!(findings.iter().any(|f| f.message.contains("U+004E")));
}

#[test]
fn glyph_missing_from_the_binary_is_reported() {
    let model = demo_font();
    let mut other = demo_font();
    let o = id_of(&other, "o");
    edit_glyph(&mut other, o, |glyph| glyph.export = false);

    let findings = check_binary(&model_of(&model), &binary_of(&other));
    let keys: Vec<_> = findings
        .iter()
        .map(|f| (f.check.as_str(), f.glyph.as_deref()))
        .collect();
    assert_eq!(
        keys,
        [("binary.cmap", Some("o")), ("binary.tables", Some("o"))],
        "{findings:#?}"
    );
}

#[test]
fn glyph_the_model_does_not_export_is_reported() {
    let mut model = demo_font();
    let other = demo_font();
    let o = id_of(&model, "o");
    edit_glyph(&mut model, o, |glyph| glyph.export = false);

    let findings = check_binary(&model_of(&model), &binary_of(&other));
    let keys: Vec<_> = findings
        .iter()
        .map(|f| (f.check.as_str(), f.glyph.as_deref()))
        .collect();
    assert_eq!(
        keys,
        [("binary.cmap", Some("o")), ("binary.tables", Some("o"))],
        "{findings:#?}"
    );
}

#[test]
fn missing_tables_are_reported_one_each() {
    let findings = check_binary(&model_of(&demo_font()), &empty_sfnt());
    assert_eq!(findings.len(), 10, "{findings:#?}");
    for tag in [
        "cmap", "head", "hhea", "hmtx", "maxp", "name", "OS/2", "post", "glyf", "loca",
    ] {
        assert!(
            findings
                .iter()
                .any(|f| f.check == "binary.tables" && f.message.contains(tag)),
            "a finding for {tag}: {findings:#?}"
        );
    }
    assert!(findings.iter().all(|f| f.severity == Severity::Error));
    assert!(findings.iter().all(|f| f.glyph.is_none()));
}

#[test]
fn unreadable_binary_is_one_error() {
    let findings = check_binary(&model_of(&demo_font()), b"this is not a font");
    assert_eq!(findings.len(), 1, "{findings:#?}");
    assert_eq!(findings[0].check, "binary.tables");
    assert_eq!(findings[0].severity, Severity::Error);
}

#[test]
fn glyph_zero_must_be_notdef() {
    let font = demo_font();
    let mut binary = binary_of(&font);
    // Glyph 0 gets the standard name `.null` (Macintosh glyph name 1).
    set_post_name_index(&mut binary, 0, 1);

    let findings = check_binary(&model_of(&font), &binary);
    assert_eq!(findings.len(), 1, "{findings:#?}");
    assert_eq!(findings[0].check, "binary.notdef");
    assert_eq!(findings[0].severity, Severity::Error);
    assert_eq!(findings[0].glyph.as_deref(), Some(".null"));
}

#[test]
fn a_notdef_the_model_does_not_export_is_not_compared() {
    // The compiler generates its own `.notdef` when the font's is not
    // exported, so the binary has one the model knows nothing about.
    let mut font = demo_font();
    add_glyph(
        &mut font,
        ".notdef",
        &[],
        777.0,
        vec![clean_rect(0, 50, 0, 727, 700)],
    );
    let notdef = id_of(&font, ".notdef");
    edit_glyph(&mut font, notdef, |glyph| glyph.export = false);

    let report = validate(&font);
    assert!(report.compiled);
    assert_eq!(report.errors, 0, "{:#?}", report.findings);
}

// ---------------------------------------------------------------------------
// Components and kerning
// ---------------------------------------------------------------------------

use forma_model::{Component, KernRef, Transform};

/// `clean_font` plus `Aacute` (`H` and a hidden mark) and the pair `H`/`I`.
fn composite_font() -> Font {
    let mut font = clean_font();
    let mark = add_glyph(
        &mut font,
        "acutecomb",
        &[],
        0.0,
        vec![clean_rect(0, 0, 0, 100, 100)],
    );
    let h = id_of(&font, "H");
    let i = id_of(&font, "I");
    let aacute = add_glyph(&mut font, "Aacute", &[0xC1], 600.0, Vec::new());
    default_layer(&mut font, aacute, |layer| {
        layer.components = vec![
            Component {
                base: h,
                transform: Transform::default(),
            },
            Component {
                base: mark,
                transform: Transform {
                    dx: 200.0,
                    dy: 750.0,
                    ..Transform::default()
                },
            },
        ];
    });
    let master = font.default_master().id;
    font.kerning
        .set_pair(master, KernRef::Glyph(h), KernRef::Glyph(i), Some(-30.0));
    font
}

#[test]
fn validate_accepts_composites_and_kerning() {
    let report = validate(&composite_font());
    assert!(report.compiled);
    assert_eq!(report.errors, 0, "{:#?}", report.findings);
    assert!(named(&report, "glyph.mixed_contours_components").is_empty());
    assert!(named(&report, "kerning.unexported_reference").is_empty());
}

#[test]
fn validate_warns_on_mixed_glyph() {
    let mut font = composite_font();
    let aacute = id_of(&font, "Aacute");
    default_layer(&mut font, aacute, |layer| {
        layer.paths = vec![clean_rect(0, 0, 0, 100, 100)];
    });
    let report = validate(&font);
    let found = named(&report, "glyph.mixed_contours_components");
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].severity, Severity::Warning);
    assert_eq!(found[0].glyph.as_deref(), Some("Aacute"));
}

#[test]
fn validate_warns_on_kerning_with_unexported_glyph() {
    let mut font = composite_font();
    let mark = id_of(&font, "acutecomb");
    let h = id_of(&font, "H");
    let master = font.default_master().id;
    font.kerning
        .set_pair(master, KernRef::Glyph(mark), KernRef::Glyph(h), Some(-10.0));
    edit_glyph(&mut font, mark, |glyph| glyph.export = false);
    let report = validate(&font);
    let found = named(&report, "kerning.unexported_reference");
    assert_eq!(found.len(), 1, "{:#?}", report.findings);
    assert_eq!(found[0].severity, Severity::Warning);
    assert_eq!(found[0].glyph.as_deref(), Some("acutecomb"));
    assert_eq!(report.errors, 0, "{:#?}", report.findings);
}

#[test]
fn validate_errors_when_gpos_missing() {
    // A binary compiled from a font without kerning, checked against the
    // model of a font with it.
    let with_kerning = composite_font();
    let mut without = with_kerning.clone();
    let master = without.default_master().id;
    without.kerning.pairs_mut().remove(&master);

    let model = model_of(&with_kerning);
    let findings = check_binary(&model, &binary_of(&without));
    let gpos: Vec<_> = findings
        .iter()
        .filter(|f| f.check == "binary.gpos")
        .collect();
    assert_eq!(gpos.len(), 1, "{findings:#?}");
    assert_eq!(gpos[0].severity, Severity::Error);

    // And nothing to report when the binary has it.
    let findings = check_binary(&model, &binary_of(&with_kerning));
    assert!(
        findings.iter().all(|f| f.check != "binary.gpos"),
        "{findings:#?}"
    );
}
