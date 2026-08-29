// TODO(ai-review): review for style and correctness
//! Steam login / logout driven from the web UI.

use axum::Json;
use axum::extract::State;
use serde::{Deserialize, Serialize};
#[allow(unused_imports)]
use serde_json::json;
use utoipa::ToSchema;

use crate::http::ApiError;
use crate::state::AppState;
use crate::steam::SteamClient;
use crate::steam::auth::{self, LoginPhase};

use super::Result;

#[derive(Debug, Deserialize, ToSchema)]
pub struct LoginRequest {
    pub account: String,
    pub password: String,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct CodeRequest {
    /// The Steam Guard code (email / TOTP) to submit.
    pub code: String,
}

#[derive(Serialize, ToSchema)]
#[schema(example = json!({
    "authenticated": true,
    "account": "alice",
    "steamid": "[U:1:12345678]",
    "pending": null
}))]
pub struct AuthStatus {
    pub authenticated: bool,
    /// Steam account name, present while logged in or while a login is pending.
    pub account: Option<String>,
    /// Steam3 id (e.g. `[U:1:...]`), present only while logged in.
    pub steamid: Option<String>,
    /// Progress of an in-flight interactive login, if any.
    pub pending: Option<LoginPhase>,
}

impl AuthStatus {
    fn from_state(state: &AppState) -> Self {
        if let Some(client) = state.steam.load_full() {
            return Self::logged_in(&client);
        }
        let pending = state.pending_login.lock().expect("pending_login poisoned");
        match pending.as_ref() {
            Some(p) => Self {
                authenticated: false,
                account: Some(p.account().to_string()),
                steamid: None,
                pending: Some(p.phase().clone()),
            },
            None => Self {
                authenticated: false,
                account: None,
                steamid: None,
                pending: None,
            },
        }
    }

    fn logged_in(client: &SteamClient) -> Self {
        Self {
            authenticated: true,
            account: Some(client.account.clone()),
            steamid: Some(client.connection.steam_id().steam3().to_string()),
            pending: None,
        }
    }
}

/// Current Steam authentication status
#[utoipa::path(
    get,
    path = "/api/auth/status",
    tag = "auth",
    responses((status = 200, body = AuthStatus))
)]
pub async fn status(State(state): State<AppState>) -> Json<AuthStatus> {
    Json(AuthStatus::from_state(&state))
}

/// Start a Steam login.
///
/// Runs in the background. See GET /api/auth/status, POST /api/auth/login/code.
#[utoipa::path(
    post,
    path = "/api/auth/login",
    request_body = LoginRequest,
    tag = "auth",
    responses((status = 200, body = AuthStatus))
)]
pub async fn login(
    State(state): State<AppState>,
    Json(body): Json<LoginRequest>,
) -> Json<AuthStatus> {
    let (handler, id) = auth::begin_web_login(state.pending_login.clone(), body.account.clone());

    let task_state = state.clone();
    let account = body.account.clone();
    let password = body.password;
    tokio::spawn(async move {
        match auth::login(&account, &password, handler).await {
            Ok(connection) => {
                tracing::info!(account = %account, "logged in via web");
                task_state.set_steam(SteamClient::new(account, connection));
                auth::clear_pending(&task_state.pending_login, id);
            }
            Err(err) => {
                tracing::warn!(%err, "web login failed");
                auth::fail_pending(
                    &task_state.pending_login,
                    id,
                    account,
                    format!("Steam login failed: {err}"),
                );
            }
        }
    });

    Json(AuthStatus::from_state(&state))
}

/// Submit the Steam Guard code for a pending login
#[utoipa::path(
    post,
    path = "/api/auth/login/code",
    request_body = CodeRequest,
    tag = "auth",
    responses(
        (status = 200, body = AuthStatus),
        (status = 400, description = "No login is awaiting a code")
    )
)]
pub async fn login_code(
    State(state): State<AppState>,
    Json(body): Json<CodeRequest>,
) -> Result<Json<AuthStatus>> {
    if body.code.trim().is_empty() {
        return Err(ApiError::bad_request("code must not be empty"));
    }
    auth::submit_code(&state.pending_login, body.code).map_err(ApiError::bad_request)?;
    Ok(Json(AuthStatus::from_state(&state)))
}

/// Log out and delete forget saved session
#[utoipa::path(
    post,
    path = "/api/auth/logout",
    tag = "auth",
    responses((status = 200, body = AuthStatus))
)]
pub async fn logout(State(state): State<AppState>) -> Json<AuthStatus> {
    state.clear_steam();
    *state.pending_login.lock().expect("pending_login poisoned") = None;
    auth::forget_session();
    tracing::info!("logged out");
    Json(AuthStatus::from_state(&state))
}
