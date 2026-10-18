//! The FORMA benchmark: a deterministic corpus of images with ground truth
//! (levels S, M, M-hand, L and O) and the runner of mode C (the pipeline
//! without an agent). A development tool; it is not published.
//!
//! * [`corpus`] builds the cases, [`Level`] names the levels.
//! * [`render_font`] renders alphabets from the OFL fonts of
//!   `tests/fixtures`; [`hard_cases`] draws the generated cases;
//!   [`degrade`] applies the deterministic degradations.
//! * [`metrics`] measures shapes (IoU, holes) and [`modec`] runs mode C.

#![forbid(unsafe_code)]

pub mod corpus;
pub mod degrade;
pub mod det;
pub mod error;
pub mod hard_cases;
pub mod metrics;
pub mod modec;
pub mod organic;
pub mod render_font;
pub mod scene;
pub mod shapes;
pub mod specimen;

pub use corpus::{
    CorpusCase, L_IDS, Level, O_IDS, Truth, TruthGlyph, build_case, case_ids, corpus,
};
pub use degrade::Degradation;
pub use error::BenchError;
pub use modec::{run_case, run_level};
