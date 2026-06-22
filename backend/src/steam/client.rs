//! Authenticated Steam connection

use std::sync::Arc;

use steam_depot_vfs::{SteamAuth, SteamSession, VfsError};
use steam_vent::Connection;
use steam_vent_depot::{CdnServer, DepotClient};
use tokio::sync::OnceCell;

pub struct SteamClient {
    /// Steam account name this connection was authenticated as. Surfaced
    /// by the auth status endpoint; the connection itself only exposes the
    /// numeric steam id.
    pub account: String,
    pub connection: Connection,
    pub depot: DepotClient,
    cdn_servers: OnceCell<Arc<[CdnServer]>>,
}

impl SteamClient {
    pub fn new(account: String, connection: Connection) -> Self {
        let depot = DepotClient::new(connection.clone());
        Self {
            account,
            connection,
            depot,
            cdn_servers: OnceCell::new(),
        }
    }
}

impl SteamAuth for SteamClient {
    async fn resolve(&self) -> steam_depot_vfs::Result<SteamSession> {
        let cdn_servers = self
            .cdn_servers
            .get_or_try_init(|| async {
                tracing::info!("discovering cdn servers");
                let servers = self.depot.cdn_servers().await?;
                Ok::<_, VfsError>(Arc::<[CdnServer]>::from(servers.into_boxed_slice()))
            })
            .await?
            .clone();

        Ok(SteamSession {
            client: self.depot.clone(),
            cdn_servers,
        })
    }
}
