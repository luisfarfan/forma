//! Spacing for FORMA (Milestone 2, spec §6.2).
//!
//! * [`profile`] measures the horizontal ink extent of a glyph on equally
//!   spaced horizontal lines of a [`Zone`], with non-zero fill and with
//!   components expanded.
//! * [`auto_spacing`] computes sidebearings that give every glyph the same
//!   amount of white on each side as the reference `H` has with a margin of
//!   `tightness ×` its stem width.
//! * [`collisions`] finds kerned pairs whose ink comes closer than a minimum
//!   distance.
//!
//! The algorithms are original implementations of published spacing
//! principles (white area between a depth-limited profile and the
//! sidebearing edge); no code of other spacing tools is used. Every function
//! is deterministic: the same font and arguments give the same result.
#![forbid(unsafe_code)]

mod collision;
mod error;
mod profile;
mod spacing;

pub use collision::{Collision, MAX_CHECK_PAIRS, collisions};
pub use error::SpacingError;
pub use profile::{MAX_LINES, MIN_LINES, Profile, Zone, measurement_zone, profile};
pub use spacing::{Sidebearings, SpacingParams, auto_spacing};
