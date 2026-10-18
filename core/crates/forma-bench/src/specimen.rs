//! Level S: the specimen of Milestone 2 (`tests/fixtures/hito2`), an own
//! drawing of eight letters, with its true outlines.

use std::collections::BTreeMap;

use forma_geometry::{CoordinateSystem, paths_from_svg, to_bezpath};
use kurbo::{Affine, BezPath};
use serde_json::Value;

use crate::corpus::{CorpusCase, Level, S_ID, Truth, TruthGlyph};
use crate::error::BenchError;
use crate::metrics::{bounds, count_holes};

const IMAGE: &[u8] = include_bytes!("../../../../tests/fixtures/hito2/specimen.png");
const TRUTH_JSON: &str = include_str!("../../../../tests/fixtures/hito2/truth.json");

/// The letters in image order: row 0, then row 1.
const ORDER: [&str; 8] = ["A", "B", "C", "H", "O", "F", "M", "R"];
/// Pixels per font unit of the generator of the specimen.
const SCALE: f64 = 0.16;
/// Baselines of the two rows and the x of the four columns, in pixels (the
/// constants of the generator, `forma-trace/tests/common`).
const BASELINES: [f64; 2] = [230.0, 450.0];
const COLUMNS: [f64; 4] = [120.0, 470.0, 820.0, 1170.0];

/// The only case of level S.
///
/// # Errors
///
/// [`BenchError::Json`] or [`BenchError::Geometry`] if the fixture is
/// malformed.
pub fn case() -> Result<CorpusCase, BenchError> {
    let json: Value = serde_json::from_str(TRUTH_JSON)?;
    let list = json["glyphs"]
        .as_array()
        .ok_or_else(|| BenchError::Geometry("truth.json has no glyphs".into()))?;
    let mut glyphs = BTreeMap::new();
    for (i, name) in ORDER.iter().enumerate() {
        let entry = list
            .iter()
            .find(|g| g["name"] == *name)
            .ok_or_else(|| BenchError::Geometry(format!("truth.json lacks {name}")))?;
        let svg = entry["svg_path"]
            .as_str()
            .ok_or_else(|| BenchError::Geometry(format!("{name} has no svg_path")))?;
        let paths = paths_from_svg(svg, CoordinateSystem::Font)
            .map_err(|e| BenchError::Geometry(e.to_string()))?;
        let (row, col) = (i / 4, i % 4);
        let to_px = Affine::new([SCALE, 0.0, 0.0, -SCALE, COLUMNS[col], BASELINES[row]]);
        let mut path = BezPath::new();
        for p in &paths {
            let bez = to_bezpath(p).map_err(|e| BenchError::Geometry(e.to_string()))?;
            path.extend((to_px * bez).elements().iter().copied());
        }
        let shapes = [path.clone()];
        let b = bounds(&shapes).unwrap_or_default();
        glyphs.insert(
            (*name).to_owned(),
            TruthGlyph {
                holes: count_holes(&shapes)?,
                px_rect: [b.x0, b.y0, b.x1, b.y1],
                pieces: 1,
                origin_px: Some([COLUMNS[col], BASELINES[row]]),
                advance_px: entry["advance_width"].as_f64().map(|a| a * SCALE),
                path,
            },
        );
    }
    Ok(CorpusCase {
        id: S_ID.to_owned(),
        level: Level::S,
        image: IMAGE.to_vec(),
        extension: "png",
        layout: vec!["ABCH".to_owned(), "OFMR".to_owned()],
        truth: Some(Truth {
            glyphs,
            irregularity_amplitude_px: None,
            scale_px_per_unit: Some(SCALE),
            cap_height_px: Some(700.0 * SCALE),
            stray_pieces: 0,
        }),
    })
}
