//! Errors of the spacing functions.

/// What can go wrong while measuring or spacing glyphs.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum SpacingError {
    /// A glyph outline could not be expanded or converted (a component
    /// cycle, a malformed contour).
    #[error("geometry failed: {0}")]
    Geometry(String),
    /// A reference glyph of the spacing (`H`, `O` by default) does not exist
    /// or has no ink.
    #[error("missing reference glyph: {0}")]
    MissingReference(String),
    /// A glyph has no layer in the requested master.
    #[error("missing layer: {0}")]
    MissingLayer(String),
    /// An argument is outside its allowed range, or names a glyph or master
    /// that does not exist.
    #[error("invalid parameter: {0}")]
    InvalidParameter(String),
}

impl From<forma_geometry::GeometryError> for SpacingError {
    fn from(error: forma_geometry::GeometryError) -> Self {
        SpacingError::Geometry(error.to_string())
    }
}
