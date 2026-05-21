use anyhow::{Context, Result};
use steam_vent::Connection;

use crate::steam::auth;

#[derive(Clone)]
pub struct AppState {
    pub connection: Connection,
}

impl AppState {
    pub async fn init() -> Result<Self> {
        let account = std::env::var("STEAM_USERNAME").context("STEAM_USERNAME not set")?;
        let password = std::env::var("STEAM_PASSWORD").context("STEAM_PASSWORD not set")?;

        let connection = auth::login(&account, &password).await?;

        tracing::info!(steam_id = %connection.steam_id().steam3(), "logged in");
        Ok(Self { connection })
    }
}
