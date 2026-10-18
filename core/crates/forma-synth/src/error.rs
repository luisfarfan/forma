//! Errors of style synthesis.

/// An error of style synthesis.
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
#[non_exhaustive]
pub enum SynthError {
    /// The font has no traced glyphs to learn the style from.
    #[error("no traced glyphs to learn the style from: {0}")]
    NoSources(String),
    /// The recipe file is malformed.
    #[error("invalid recipes: {0}")]
    Recipes(String),
    /// A geometry operation failed.
    #[error("geometry: {0}")]
    Geometry(String),
}
