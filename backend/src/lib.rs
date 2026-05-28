// TODO(ai-review): review for style and correctness
//! Library face of the viewer. The HTTP server lives in `main.rs`;
//! the `transform` crate houses the format builders + dumpers and is
//! consumed both here and by the standalone examples.
//!
//! Only the configuration façade is public here — anything
//! route-shaped (axum handlers, AppState, etc.) stays bin-only, and
//! format-handling lives in the `transform` crate which examples can
//! depend on directly.

pub mod config;
