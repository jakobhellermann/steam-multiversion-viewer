// TODO(ai-review): review for style and correctness
//! Library face of the viewer. The HTTP server lives in `main.rs`; the
//! lib re-exports the modules that examples + future external callers
//! need to drive the same logic standalone (no axum, no state).
//!
//! Only the format builders + content dumpers are public here on
//! purpose — anything route-shaped (axum handlers, AppState, etc)
//! stays bin-only.

pub mod config;
pub mod dll;
pub mod structured;
pub mod transform;

#[cfg(feature = "unity")]
pub mod unity;
