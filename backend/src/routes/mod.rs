// TODO(ai-review): review for style and correctness

use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::error::ApiError;
use crate::state::AppState;

pub mod config;
pub mod diff;
pub mod downloads;
pub mod extra_manifests;
pub mod files;
pub mod game_info;
pub mod library;
pub mod mount;
pub mod structured;

pub(crate) type Result<T, E = ApiError> = std::result::Result<T, E>;

pub(super) fn default_branch() -> String {
    "public".into()
}

pub fn register(router: OpenApiRouter<AppState>) -> OpenApiRouter<AppState> {
    router
        .routes(routes!(library::library))
        .routes(routes!(library::app_info))
        .routes(routes!(library::manifest_info))
        .routes(routes!(library::manifest_files))
        .routes(routes!(library::manifest_statuses))
        .routes(routes!(game_info::game_info))
        .routes(routes!(files::manifest_file))
        .routes(routes!(files::manifest_file_raw))
        .routes(routes!(files::manifest_file_transformed))
        .routes(routes!(structured::manifest_file_structured))
        .routes(routes!(structured::manifest_file_structured_node))
        .routes(routes!(diff::manifest_diff))
        .routes(routes!(diff::file_diff_targets))
        .routes(routes!(diff::manifest_file_diff))
        .routes(routes!(diff::manifest_file_structured_diff))
        .routes(routes!(diff::manifest_file_structured_diff_node))
        .routes(routes!(downloads::manifest_download))
        .routes(routes!(downloads::downloads_snapshot))
        .routes(routes!(downloads::downloads_cancel))
        .routes(routes!(config::get_config, config::patch_config))
        .routes(routes!(
            extra_manifests::get_extra_manifests,
            extra_manifests::put_extra_manifests
        ))
        .routes(routes!(extra_manifests::delete_extra_manifest))
        .routes(routes!(mount::start))
        .routes(routes!(mount::stop))
        .routes(routes!(mount::status))
}
