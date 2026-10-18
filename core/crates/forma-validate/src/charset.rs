//! Character-set profiles: which code points a font must cover, and how a
//! font covers them.

use forma_model::{Font, Glyph};

/// The code points of the `unicase-es` profile: space and no-break space, the
/// 52 basic Latin letters, the 14 accented Spanish letters, the 10 digits and
/// 41 punctuation marks and symbols.
const UNICASE_ES: [u32; 119] = [
    0x0020, 0x00A0, 0x0041, 0x0042, 0x0043, 0x0044, 0x0045, 0x0046, 0x0047, 0x0048, 0x0049, 0x004A,
    0x004B, 0x004C, 0x004D, 0x004E, 0x004F, 0x0050, 0x0051, 0x0052, 0x0053, 0x0054, 0x0055, 0x0056,
    0x0057, 0x0058, 0x0059, 0x005A, 0x0061, 0x0062, 0x0063, 0x0064, 0x0065, 0x0066, 0x0067, 0x0068,
    0x0069, 0x006A, 0x006B, 0x006C, 0x006D, 0x006E, 0x006F, 0x0070, 0x0071, 0x0072, 0x0073, 0x0074,
    0x0075, 0x0076, 0x0077, 0x0078, 0x0079, 0x007A, 0x00C1, 0x00C9, 0x00CD, 0x00D1, 0x00D3, 0x00DA,
    0x00DC, 0x00E1, 0x00E9, 0x00ED, 0x00F1, 0x00F3, 0x00FA, 0x00FC, 0x0030, 0x0031, 0x0032, 0x0033,
    0x0034, 0x0035, 0x0036, 0x0037, 0x0038, 0x0039, 0x002E, 0x002C, 0x003A, 0x003B, 0x0021, 0x00A1,
    0x003F, 0x00BF, 0x0027, 0x0022, 0x2018, 0x2019, 0x201C, 0x201D, 0x002D, 0x2013, 0x2014, 0x0028,
    0x0029, 0x005B, 0x005D, 0x007B, 0x007D, 0x002F, 0x005C, 0x0026, 0x0040, 0x0023, 0x0024, 0x0025,
    0x002A, 0x002B, 0x003D, 0x005F, 0x003C, 0x003E, 0x2026, 0x00B7, 0x00AB, 0x00BB, 0x20AC,
];

/// The code points of the profile called `name`, in the order of its table, or
/// `None` if there is no such profile.
///
/// The profiles are `"unicase-es"` (119 code points).
#[must_use]
pub fn profile(name: &str) -> Option<&'static [u32]> {
    match name {
        "unicase-es" => Some(&UNICASE_ES),
        _ => None,
    }
}

/// How well a font covers a set of code points.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CharsetReport {
    /// How many distinct code points were required.
    pub required: usize,
    /// How many of them an exported glyph encodes.
    pub covered: usize,
    /// The required code points no exported glyph encodes, ascending.
    pub missing: Vec<u32>,
    /// The names of the exported glyphs that encode a required code point and
    /// are made of components only (in the default master), in glyph order.
    pub composite: Vec<String>,
    /// The names of the exported glyphs that encode a required code point and
    /// have contours of their own (in the default master), in glyph order.
    pub outlined: Vec<String>,
}

/// Checks which of `codepoints` the exported glyphs of `font` encode.
///
/// Glyphs that are not exported are not in the binary, so they do not count.
/// A glyph with neither contours nor components (a space) is in neither
/// `composite` nor `outlined`; one with both is `outlined`.
#[must_use]
pub fn check_charset(font: &Font, codepoints: &[u32]) -> CharsetReport {
    let mut required: Vec<u32> = codepoints.to_vec();
    required.sort_unstable();
    required.dedup();

    let master = font.default_master().id;
    let mut found = std::collections::BTreeSet::new();
    let mut composite = Vec::new();
    let mut outlined = Vec::new();
    let glyphs = font
        .glyph_order
        .iter()
        .filter_map(|id| font.glyphs.get(id))
        .map(|glyph| &**glyph)
        .filter(|glyph: &&Glyph| glyph.is_exported());
    for glyph in glyphs {
        let covers: Vec<u32> = glyph
            .unicodes
            .iter()
            .copied()
            .filter(|codepoint| required.binary_search(codepoint).is_ok())
            .collect();
        if covers.is_empty() {
            continue;
        }
        found.extend(covers);
        if let Some(layer) = glyph.layers.get(&master) {
            if !layer.paths.is_empty() {
                outlined.push(glyph.name.clone());
            } else if !layer.components.is_empty() {
                composite.push(glyph.name.clone());
            }
        }
    }

    let missing: Vec<u32> = required
        .iter()
        .copied()
        .filter(|codepoint| !found.contains(codepoint))
        .collect();
    CharsetReport {
        required: required.len(),
        covered: required.len() - missing.len(),
        missing,
        composite,
        outlined,
    }
}
