use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::Serialize;
use steam_vent::ConnectionTrait;
use steam_vent_proto::steammessages_player_steamclient::CPlayer_GetOwnedGames_Request;
use utoipa::ToSchema;

use crate::state::AppState;
use crate::steam::AppId;

#[derive(Serialize, ToSchema)]
pub struct OwnedGame {
    pub appid: AppId,
    pub name: String,
    pub playtime_minutes: u32,
}

#[utoipa::path(get, path = "/api/library")]
pub async fn library(
    State(state): State<AppState>,
) -> Result<Json<Vec<OwnedGame>>, (StatusCode, String)> {
    let req = CPlayer_GetOwnedGames_Request {
        steamid: Some(state.connection.steam_id().into()),
        include_appinfo: Some(true),
        include_played_free_games: Some(true),
        ..Default::default()
    };

    let resp = state
        .connection
        .service_method(req)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let games = resp
        .games
        .into_iter()
        .map(|g| OwnedGame {
            appid: AppId(g.appid()),
            name: g.name().to_string(),
            playtime_minutes: g.playtime_forever() as u32,
        })
        .collect();

    Ok(Json(games))
}

#[derive(Serialize, ToSchema)]
pub struct AppInfo {
    pub appid: AppId,
    pub name: String,
    pub r#type: String,
    pub developer: String,
    pub publisher: String,
    pub homepage: Option<String>,
    pub logo_url: Option<String>,
    pub icon_url: String,
}

#[utoipa::path(get, path = "/api/apps/{appid}")]
pub async fn app_info(
    State(state): State<AppState>,
    Path(appid): Path<i32>,
) -> Result<Json<AppInfo>, (StatusCode, String)> {
    let info = state
        .depot
        .app_info(appid as u32)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let asset_url = |hash: &str, ext: &str| {
        format!(
            "https://cdn.cloudflare.steamstatic.com/steamcommunity/public/images/apps/{appid}/{hash}.{ext}"
        )
    };

    Ok(Json(AppInfo {
        appid: AppId(appid),
        name: info.common.name,
        r#type: info.common.r#type,
        developer: info.extended.developer,
        publisher: info.extended.publisher,
        homepage: info.extended.homepage,
        logo_url: info.common.logo.as_deref().map(|h| asset_url(h, "jpg")),
        icon_url: asset_url(&info.common.icon, "jpg"),
    }))
}
