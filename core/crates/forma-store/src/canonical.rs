//! Canonical JSON: the one text form of a JSON value, so that saving the same
//! document twice gives the same bytes and version control diffs show only
//! real changes.
//!
//! * object keys are sorted (by their UTF-8 bytes) at every level;
//! * arrays keep their order;
//! * two-space indentation, one member or element per line;
//! * exactly one `\n` at the end.

use serde::{Serialize, Serializer};
use serde_json::Value;

/// A JSON value that serializes with the keys of every object sorted.
///
/// Sorting happens here, explicitly, instead of relying on the iteration order
/// of [`serde_json::Map`], which depends on whether any crate in the build
/// enables serde_json's `preserve_order` feature.
struct Sorted<'a>(&'a Value);

impl Serialize for Sorted<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self.0 {
            Value::Array(items) => serializer.collect_seq(items.iter().map(Sorted)),
            Value::Object(members) => {
                let mut members: Vec<(&String, &Value)> = members.iter().collect();
                members.sort_unstable_by(|a, b| a.0.cmp(b.0));
                serializer.collect_map(members.into_iter().map(|(key, value)| (key, Sorted(value))))
            }
            scalar => scalar.serialize(serializer),
        }
    }
}

/// Renders a JSON value in canonical form.
pub(crate) fn canonical_string(value: &Value) -> Result<String, serde_json::Error> {
    let mut text = serde_json::to_string_pretty(&Sorted(value))?;
    text.push('\n');
    Ok(text)
}

/// Serializes `value` as canonical JSON, or reports why it cannot be.
///
/// # Errors
///
/// Fails when `value`'s `Serialize` implementation fails, for example because
/// it contains a map with non-string keys.
pub fn try_to_canonical_json<T: Serialize + ?Sized>(
    value: &T,
) -> Result<String, serde_json::Error> {
    canonical_string(&serde_json::to_value(value)?)
}

/// Serializes `value` as canonical JSON: keys sorted at every level, two-space
/// indentation and a final newline. Equal values always give equal text.
///
/// The FORMA model types always serialize, so for them this is total. For a
/// value whose `Serialize` implementation fails the result is the JSON
/// document `null`; use [`try_to_canonical_json`] to see such errors.
///
/// Note that JSON has no representation for NaN and infinities; `serde_json`
/// writes them as `null`. The store rejects such numbers before saving.
#[must_use]
pub fn to_canonical_json<T: Serialize + ?Sized>(value: &T) -> String {
    try_to_canonical_json(value).unwrap_or_else(|_| "null\n".to_owned())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn keys_are_sorted_at_every_level() {
        let value = json!({"b": {"z": 1, "y": 2}, "a": [{"d": 1, "c": 2}], "B": 0});

        assert_eq!(
            to_canonical_json(&value),
            "{\n  \"B\": 0,\n  \"a\": [\n    {\n      \"c\": 2,\n      \"d\": 1\n    }\n  ],\n  \"b\": {\n    \"y\": 2,\n    \"z\": 1\n  }\n}\n"
        );
    }

    #[test]
    fn sorting_is_by_utf8_bytes() {
        let value = json!({"\u{e9}": 1, "z": 2, "Z": 3, "_": 4});

        // 'Z' (0x5A) < '_' (0x5F) < 'z' (0x7A) < 'é' (0xC3 0xA9)
        let text = to_canonical_json(&value);
        let order: Vec<usize> = ["\"Z\"", "\"_\"", "\"z\"", "\"\u{e9}\""]
            .iter()
            .map(|key| text.find(key).unwrap())
            .collect();
        assert!(order.windows(2).all(|pair| pair[0] < pair[1]), "{text}");
    }

    #[test]
    fn arrays_keep_their_order() {
        assert_eq!(
            to_canonical_json(&json!([3, 1, 2])),
            "[\n  3,\n  1,\n  2\n]\n"
        );
    }

    #[test]
    fn empty_containers_and_scalars() {
        assert_eq!(to_canonical_json(&json!({})), "{}\n");
        assert_eq!(to_canonical_json(&json!([])), "[]\n");
        assert_eq!(
            to_canonical_json(&json!({"a": [], "b": {}})),
            "{\n  \"a\": [],\n  \"b\": {}\n}\n"
        );
        assert_eq!(to_canonical_json(&"text"), "\"text\"\n");
        assert_eq!(to_canonical_json(&1.5f64), "1.5\n");
    }

    #[test]
    fn struct_fields_come_out_sorted() {
        #[derive(Serialize)]
        struct Unsorted {
            zebra: u8,
            apple: u8,
        }

        assert_eq!(
            to_canonical_json(&Unsorted { zebra: 1, apple: 2 }),
            "{\n  \"apple\": 2,\n  \"zebra\": 1\n}\n"
        );
    }

    #[test]
    fn floats_keep_their_exact_value() {
        for value in [
            0.1,
            1.0 / 3.0,
            1e-300,
            123_456_789.012_345_68,
            f64::MAX,
            f64::MIN_POSITIVE,
        ] {
            let text = to_canonical_json(&value);
            let back: f64 = serde_json::from_str(&text).unwrap();
            assert_eq!(back.to_bits(), value.to_bits(), "{text}");
        }
    }

    #[test]
    fn unserializable_values_are_reported_by_the_fallible_form() {
        use std::collections::BTreeMap;

        let not_a_json_object: BTreeMap<(u8, u8), u8> = BTreeMap::from([((1, 2), 3)]);

        assert!(try_to_canonical_json(&not_a_json_object).is_err());
        assert_eq!(to_canonical_json(&not_a_json_object), "null\n");
    }
}
