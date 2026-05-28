// TODO(ai-review): review for style and correctness

use crate::error::ApiError;

pub mod config;
pub mod diff;
pub mod downloads;
pub mod extra_manifests;
pub mod files;
pub mod library;
pub mod mount;
pub mod structured;

pub(crate) type Result<T, E = ApiError> = std::result::Result<T, E>;

pub(super) fn default_branch() -> String {
    "public".into()
}
