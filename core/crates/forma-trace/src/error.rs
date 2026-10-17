//! Errors of the tracing pipeline.

/// What can go wrong while turning an image into glyph candidates.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum TraceError {
    /// The bytes are not a valid PNG or JPEG image.
    #[error("cannot decode image: {0}")]
    Decode(String),
    /// The image exceeds the size limits.
    #[error("image too large: {bytes} bytes, {pixels} pixels")]
    TooLarge {
        /// Encoded size in bytes.
        bytes: u64,
        /// Declared pixel count (0 if not read yet).
        pixels: u64,
    },
    /// More candidates than [`crate::MAX_CANDIDATES`].
    #[error("too many candidates: {0}")]
    TooManyCandidates(usize),
    /// A candidate index is outside the analysis.
    #[error("candidate {index} out of range (count {count})")]
    CandidateOutOfRange {
        /// The requested index.
        index: usize,
        /// How many candidates exist.
        count: usize,
    },
    /// An argument is outside its allowed range (e.g. a cap height that is
    /// not a positive number).
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
    /// Vectorization failed.
    #[error("vectorization failed: {0}")]
    Vectorize(String),
    /// Rendering failed.
    #[error("render failed: {0}")]
    Render(String),
    /// A geometry operation failed.
    #[error("geometry failed: {0}")]
    Geometry(String),
}
