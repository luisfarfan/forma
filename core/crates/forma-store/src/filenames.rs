//! File names for the glyph files of a `.formapackage`.
//!
//! Glyph names are free-form printable ASCII, but file names are not: they
//! cannot hold some characters, some are reserved on Windows, and on macOS and
//! Windows `a.json` and `A.json` are the same file. [`glyph_file_name`] turns
//! a glyph name into a file name that is safe everywhere and, among the glyphs
//! of one font, never the same as another's.

use std::collections::HashSet;

/// Device names that Windows reserves, with or without an extension.
const WINDOWS_RESERVED: [&str; 22] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Whether Windows would treat a file called `name` as a device: the part
/// before the first period is a reserved name, in any case.
fn is_windows_reserved(name: &str) -> bool {
    let base = name.split('.').next().unwrap_or(name);
    WINDOWS_RESERVED
        .iter()
        .any(|reserved| base.eq_ignore_ascii_case(reserved))
}

/// The file name stem for a glyph name, before any collision handling.
///
/// * every ASCII capital is followed by `_` (`A` becomes `A_`), so the stems of
///   names that differ only in case differ in more than case;
/// * `" * + / : < > ? [ \ ] |` and control characters become `_`;
/// * a leading `.` becomes `_`, so no file is hidden;
/// * a Windows device name gets a `_` in front.
fn stem(name: &str) -> String {
    let mut stem = String::with_capacity(name.len() + 8);
    if is_windows_reserved(name) {
        stem.push('_');
    }
    for (index, ch) in name.chars().enumerate() {
        match ch {
            'A'..='Z' => {
                stem.push(ch);
                stem.push('_');
            }
            '"' | '*' | '+' | '/' | ':' | '<' | '>' | '?' | '[' | '\\' | ']' | '|' => {
                stem.push('_');
            }
            '.' if index == 0 => stem.push('_'),
            ch if ch.is_control() => stem.push('_'),
            ch => stem.push(ch),
        }
    }
    if stem.is_empty() {
        stem.push('_');
    }
    stem
}

/// The file name for a glyph name, avoiding the names `is_taken` reports.
///
/// `is_taken` is given complete candidate file names and must compare
/// case-insensitively. The first candidate is `<stem>.json`, then
/// `<stem>#2.json`, `<stem>#3.json` and so on.
pub(crate) fn unique_file_name(name: &str, is_taken: impl Fn(&str) -> bool) -> String {
    let stem = stem(name);
    let first = format!("{stem}.json");
    if !is_taken(&first) {
        return first;
    }
    let mut number: u64 = 2;
    loop {
        let candidate = format!("{stem}#{number}.json");
        if !is_taken(&candidate) {
            return candidate;
        }
        number += 1;
    }
}

/// The key under which a file name is remembered: lower case, because file
/// systems that ignore case treat names that differ only in case as one.
pub(crate) fn case_key(file_name: &str) -> String {
    file_name.to_lowercase()
}

/// Returns the file name (with the `.json` extension) for the glyph called
/// `name`, given the file names `taken` by the other glyphs of the font.
///
/// The rules:
///
/// * every ASCII capital is followed by `_` (`A` -> `A_`);
/// * `"`, `*`, `+`, `/`, `:`, `<`, `>`, `?`, `[`, `\`, `]`, `|` and control
///   characters become `_`;
/// * a leading `.` becomes `_` (`.notdef` -> `_notdef`);
/// * the Windows device names `CON`, `PRN`, `AUX`, `NUL`, `COM1`-`COM9` and
///   `LPT1`-`LPT9` (in any case, also followed by an extension such as
///   `CON.alt`) get a `_` in front;
/// * if the result is in `taken`, compared without regard to case, `#2`,
///   `#3`, ... is appended to the name until it is not;
/// * the name ends in `.json`.
///
/// `taken` holds complete file names, `.json` included.
#[must_use]
pub fn glyph_file_name(name: &str, taken: &HashSet<String>) -> String {
    let taken: HashSet<String> = taken.iter().map(|taken| case_key(taken)).collect();
    unique_file_name(name, |candidate| taken.contains(&case_key(candidate)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn name(glyph: &str) -> String {
        glyph_file_name(glyph, &HashSet::new())
    }

    #[test]
    fn plain_names_only_gain_the_extension() {
        assert_eq!(name("a"), "a.json");
        assert_eq!(name("space"), "space.json");
        assert_eq!(name("a-b_c.d"), "a-b_c.d.json");
        assert_eq!(name("a#2"), "a#2.json");
    }

    #[test]
    fn capitals_are_followed_by_an_underscore() {
        assert_eq!(name("A"), "A_.json");
        assert_eq!(name("AE"), "A_E_.json");
        assert_eq!(name("Aacute"), "A_acute.json");
        assert_eq!(name("a_"), "a_.json");
        assert_eq!(name("A_"), "A__.json");
    }

    #[test]
    fn forbidden_characters_and_controls_become_underscores() {
        assert_eq!(name("a\"b"), "a_b.json");
        assert_eq!(name("a*b+c/d:e"), "a_b_c_d_e.json");
        assert_eq!(name("a<b>c?d"), "a_b_c_d.json");
        assert_eq!(name("a[b\\c]d|e"), "a_b_c_d_e.json");
        assert_eq!(name("a\u{0}b\u{7f}c\n"), "a_b_c_.json");
        // Other punctuation is fine.
        assert_eq!(name("a!#$%&'()-=@^`{}~b"), "a!#$%&'()-=@^`{}~b.json");
    }

    #[test]
    fn only_a_leading_period_is_replaced() {
        assert_eq!(name(".notdef"), "_notdef.json");
        assert_eq!(name("..x"), "_.x.json");
        assert_eq!(name("a.b"), "a.b.json");
        assert_eq!(name("a."), "a..json");
    }

    #[test]
    fn windows_device_names_get_a_prefix_in_any_case() {
        assert_eq!(name("con"), "_con.json");
        assert_eq!(name("Con"), "_C_on.json");
        assert_eq!(name("CON"), "_C_O_N_.json");
        assert_eq!(name("CON.alt"), "_C_O_N_.alt.json");
        assert_eq!(name("con.alt.sc"), "_con.alt.sc.json");
        assert_eq!(name("prn"), "_prn.json");
        assert_eq!(name("aux"), "_aux.json");
        assert_eq!(name("nul"), "_nul.json");
        assert_eq!(name("com1"), "_com1.json");
        assert_eq!(name("COM9"), "_C_O_M_9.json");
        assert_eq!(name("lpt1.sc"), "_lpt1.sc.json");
        assert_eq!(name("LPT9"), "_L_P_T_9.json");
    }

    #[test]
    fn names_that_merely_contain_a_device_name_are_left_alone() {
        assert_eq!(name("icon"), "icon.json");
        assert_eq!(name("conx"), "conx.json");
        assert_eq!(name("com0"), "com0.json");
        assert_eq!(name("com10"), "com10.json");
        assert_eq!(name("lpt"), "lpt.json");
        assert_eq!(name("x.con"), "x.con.json");
    }

    #[test]
    fn collisions_are_numbered_without_regard_to_case() {
        let taken = |names: &[&str]| -> HashSet<String> {
            names.iter().map(|name| (*name).to_owned()).collect()
        };

        // `A` is `A_.json`, which is `a_.json` on a case-insensitive disk.
        assert_eq!(glyph_file_name("a_", &taken(&["A_.json"])), "a_#2.json");
        assert_eq!(glyph_file_name("A", &taken(&["a_.json"])), "A_#2.json");
        assert_eq!(
            glyph_file_name("a_", &taken(&["a_.json", "A_#2.json"])),
            "a_#3.json"
        );
        assert_eq!(
            glyph_file_name("a_", &taken(&["a_.json", "a_#2.json", "a_#3.json"])),
            "a_#4.json"
        );
        // Unrelated names do not matter.
        assert_eq!(glyph_file_name("a_", &taken(&["b.json"])), "a_.json");
    }

    #[test]
    fn a_name_that_already_looks_numbered_still_gets_a_unique_file() {
        let taken: HashSet<String> = ["a.json".to_owned(), "a#2.json".to_owned()].into();

        // The glyph `a#2` is a different glyph from the second `a`.
        assert_eq!(glyph_file_name("a#2", &taken), "a#2#2.json");
        assert_eq!(glyph_file_name("a", &taken), "a#3.json");
    }

    #[test]
    fn degenerate_names_still_give_a_file_name() {
        assert_eq!(name(""), "_.json");
        assert_eq!(name("."), "_.json");
    }

    #[test]
    fn results_are_valid_on_every_platform() {
        for glyph in [
            "a", "A", "con", "CON.alt", ".notdef", "a/b", "x:y", "q?", "<>", "a|b", "\"", "*",
        ] {
            let file = name(glyph);
            assert!(file.ends_with(".json"), "{file}");
            assert!(!file.starts_with('.'), "{file}");
            assert!(
                !file
                    .chars()
                    .any(|c| c.is_control() || "\"*/:<>?\\|".contains(c)),
                "{file}"
            );
            assert!(!is_windows_reserved(&file), "{file}");
        }
    }
}
