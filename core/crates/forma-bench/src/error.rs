//! The error type of the benchmark crate.

/// An error raised while building the corpus or running a mode.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum BenchError {
    /// A font could not be read.
    #[error("font error: {0}")]
    Font(String),
    /// An outline could not be converted or rasterized.
    #[error("geometry error: {0}")]
    Geometry(String),
    /// An image could not be encoded or decoded.
    #[error("image error: {0}")]
    Image(String),
    /// A case id or level is unknown.
    #[error("unknown {0}")]
    Unknown(String),
    /// The engine refused a call.
    #[error("engine error: {0}")]
    Engine(String),
    /// A file could not be read or written.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// A JSON document could not be built or read.
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
}
