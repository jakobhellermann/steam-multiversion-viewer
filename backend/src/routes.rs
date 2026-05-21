use axum::Json;
use axum::extract::State;
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
