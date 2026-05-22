use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use directories::ProjectDirs;
use steam_vent::auth::{
    AuthConfirmationHandler, ConsoleAuthConfirmationHandler, DeviceConfirmationHandler,
    FileGuardDataStore,
};
use steam_vent::{Connection, ServerList};
use tracing::Instrument;

pub async fn login(account: &str, password: &str) -> Result<Connection> {
    let server_list = ServerList::discover()
        .instrument(tracing::debug_span!("discover_server_list").or_current())
        .await?;

    let refresh_token = load_refresh_token(account);
    let connection = match refresh_token {
        Some(token) => match Connection::access(&server_list, account, &token).await {
            Ok(conn) => {
                tracing::info!(steam_id = %conn.steam_id().steam3(), "logged in");
                conn
            }
            Err(err) => {
                tracing::warn!(%err, "cached refresh token rejected, falling back to password login");
                password_login(&server_list, account, password).await?
            }
        },
        None => password_login(&server_list, account, password).await?,
    };

    Ok(connection)
}

async fn password_login(
    server_list: &ServerList,
    account: &str,
    password: &str,
) -> Result<Connection> {
    let conn = Connection::login(
        server_list,
        account,
        password,
        FileGuardDataStore::user_cache(), // TODO: use steam-multiversion-viewer cache
        ConsoleAuthConfirmationHandler::default().or(DeviceConfirmationHandler), // TODO: improve user experience
    )
    .await?;
    if let Some(token) = conn.access_token() {
        if let Err(err) = save_refresh_token(account, token) {
            tracing::warn!(%err, "failed to persist refresh token");
        } else {
            tracing::info!("saved refresh token");
        }
    }
    Ok(conn)
}

fn refresh_token_path() -> Result<PathBuf> {
    // TODO: centralize cache dir access
    let dirs = ProjectDirs::from("", "", "steam-multiversion-viewer")
        .context("user cache dir not supported on this platform")?;
    Ok(dirs.cache_dir().join("refresh_tokens.json"))
}

fn load_refresh_token(account: &str) -> Option<String> {
    let path = refresh_token_path().ok()?;
    let raw = fs::read_to_string(path).ok()?;
    let map: HashMap<String, String> = serde_json::from_str(&raw).ok()?;
    map.get(account).cloned().filter(|t| !t.is_empty())
}

fn save_refresh_token(account: &str, token: &str) -> Result<()> {
    let path = refresh_token_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut map: HashMap<String, String> = fs::read_to_string(&path)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default();
    map.insert(account.into(), token.into());
    fs::write(&path, serde_json::to_string(&map)?)?;
    Ok(())
}
