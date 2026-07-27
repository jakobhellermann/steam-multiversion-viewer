use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use directories::ProjectDirs;
use serde::Serialize;
use steam_vent::auth::{
    AuthConfirmationHandler, ConfirmationAction, ConfirmationMethod, ConfirmationMethodClass,
    DeviceConfirmationHandler, FileGuardDataStore, UserProvidedAuthConfirmationHandler,
};
use steam_vent::{Connection, ServerList};
use tokio::io::{AsyncWriteExt, sink};
use tokio::sync::oneshot;
use tracing::Instrument;
use utoipa::ToSchema;

/// Where an in-progress interactive login currently is, surfaced to the UI
/// via the auth status endpoint so it can prompt for a code or tell the
/// user to confirm on their phone.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(tag = "phase", rename_all = "snake_case")]
pub enum LoginPhase {
    /// Login task started, allowed confirmations not yet known.
    Starting,
    /// Waiting for the user to approve the login in the Steam mobile app.
    WaitingDevice,
    /// Steam wants a Steam Guard code (email / TOTP) entered in the UI.
    NeedCode {
        /// Human-readable confirmation type, e.g. `"email"` / `"device code"`.
        code_type: String,
        /// Server-provided message (e.g. which email the code was sent to).
        details: String,
        /// Whether confirming in the mobile app is also an option.
        device_available: bool,
    },
    /// The login failed; `message` is the reason.
    Error { message: String },
}

/// In-progress interactive login. Lives in [`PendingLoginSlot`] for the
/// duration of one `/api/auth/login` attempt. `id` disambiguates attempts so
/// a superseded one can't clobber a newer one's state.
pub struct PendingLogin {
    id: u64,
    account: String,
    phase: LoginPhase,
    /// Set while the confirmation handler is parked waiting for a code;
    /// taken when `/api/auth/login/code` delivers one.
    code_tx: Option<oneshot::Sender<String>>,
}

impl PendingLogin {
    pub fn account(&self) -> &str {
        &self.account
    }
    pub fn phase(&self) -> &LoginPhase {
        &self.phase
    }
}

/// Shared handle to the (at most one) in-progress interactive login.
pub type PendingLoginSlot = Arc<Mutex<Option<PendingLogin>>>;

static NEXT_LOGIN_ID: AtomicU64 = AtomicU64::new(1);

/// Register a fresh pending login in `slot` and return the confirmation
/// handler to hand to [`login`] plus the attempt id (used by the caller to
/// scope completion/failure updates to this attempt).
pub fn begin_web_login(slot: PendingLoginSlot, account: String) -> (WebConfirmationHandler, u64) {
    let id = NEXT_LOGIN_ID.fetch_add(1, Ordering::Relaxed);
    let (code_tx, code_rx) = oneshot::channel();
    *slot.lock().expect("pending_login poisoned") = Some(PendingLogin {
        id,
        account,
        phase: LoginPhase::Starting,
        code_tx: Some(code_tx),
    });
    (
        WebConfirmationHandler {
            slot: slot.clone(),
            id,
            code_rx,
        },
        id,
    )
}

/// Deliver a Steam Guard code to a parked login. Errors if no login is
/// awaiting a code.
pub fn submit_code(slot: &PendingLoginSlot, code: String) -> Result<(), &'static str> {
    let mut guard = slot.lock().expect("pending_login poisoned");
    let pending = guard.as_mut().ok_or("no login in progress")?;
    let tx = pending
        .code_tx
        .take()
        .ok_or("login is not awaiting a code")?;
    tx.send(code).map_err(|_| "login task is no longer running")
}

/// Clear the pending slot once `id`'s login succeeded — but only if it
/// hasn't been superseded by a newer attempt.
pub fn clear_pending(slot: &PendingLoginSlot, id: u64) {
    let mut guard = slot.lock().expect("pending_login poisoned");
    if guard.as_ref().is_some_and(|p| p.id == id) {
        *guard = None;
    }
}

/// Record a terminal error for `id`'s login so the UI can show it — unless a
/// newer attempt has already taken over the slot.
pub fn fail_pending(slot: &PendingLoginSlot, id: u64, account: String, message: String) {
    let mut guard = slot.lock().expect("pending_login poisoned");
    if guard.as_ref().is_none_or(|p| p.id == id) {
        *guard = Some(PendingLogin {
            id,
            account,
            phase: LoginPhase::Error { message },
            code_tx: None,
        });
    }
}

/// Confirmation handler that routes Steam Guard prompts to the web UI.
///
/// Whenever Steam offers a code method it parks waiting for one from the UI,
/// then delegates to [`UserProvidedAuthConfirmationHandler`] (which owns the
/// otherwise un-constructible `SteamGuardToken`) by feeding the code over an
/// in-memory pipe. The mobile-app path needs no action here: `Connection`
/// polls for out-of-band approval concurrently with this handler, so if the
/// user confirms in the app the login completes and this future is dropped.
pub struct WebConfirmationHandler {
    slot: PendingLoginSlot,
    id: u64,
    code_rx: oneshot::Receiver<String>,
}

impl AuthConfirmationHandler for WebConfirmationHandler {
    async fn handle_confirmation(
        self,
        allowed_confirmations: &[ConfirmationMethod],
    ) -> Option<ConfirmationAction> {
        let WebConfirmationHandler { slot, id, code_rx } = self;

        let set_phase = |phase: LoginPhase| {
            if let Some(p) = slot.lock().expect("pending_login poisoned").as_mut()
                && p.id == id
            {
                p.phase = phase;
            }
        };

        let device_available = allowed_confirmations
            .iter()
            .any(|m| m.class() == ConfirmationMethodClass::Confirmation);
        let code = allowed_confirmations.iter().find_map(|m| {
            m.token_type().map(|_| {
                (
                    m.confirmation_type().to_string(),
                    m.confirmation_details().to_string(),
                )
            })
        });

        // A code is offered: surface a field and wait for it. We don't have
        // to decide between app and code up front — `Connection` polls for
        // mobile-app approval in parallel and wins (dropping us) if the user
        // confirms there instead.
        if let Some((code_type, details)) = code {
            set_phase(LoginPhase::NeedCode {
                code_type,
                details,
                device_available,
            });
            match code_rx.await {
                Ok(code) => {
                    set_phase(LoginPhase::WaitingDevice);
                    // Hand the code to the library handler over a pipe; it
                    // builds the `SteamGuardToken` we can't construct here.
                    let (mut writer, reader) = tokio::io::duplex(64);
                    let _ = writer.write_all(format!("{code}\n").as_bytes()).await;
                    drop(writer);
                    UserProvidedAuthConfirmationHandler::new(reader, sink())
                        .handle_confirmation(allowed_confirmations)
                        .await
                }
                // Sender dropped (login completed elsewhere or was
                // superseded): nothing to submit, let polling resolve it.
                Err(_) => Some(ConfirmationAction::None),
            }
        } else if device_available {
            // App confirmation only — just wait for the concurrent poll.
            set_phase(LoginPhase::WaitingDevice);
            Some(ConfirmationAction::None)
        } else {
            None
        }
    }
}

/// Log in resolving 2FA out-of-band via the Steam mobile app
/// ([`DeviceConfirmationHandler`]). Used for the optional env-var startup
/// path, where there is no UI to route a code to.
pub async fn login_device(account: &str, password: &str) -> Result<Connection> {
    login(account, password, DeviceConfirmationHandler).await
}

/// Establish a connection, preferring the cached refresh token and falling
/// back to a password login that resolves 2FA via `confirmation`.
pub async fn login(
    account: &str,
    password: &str,
    confirmation: impl AuthConfirmationHandler + Send + Sync,
) -> Result<Connection> {
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
                password_login(&server_list, account, password, confirmation).await?
            }
        },
        None => password_login(&server_list, account, password, confirmation).await?,
    };

    Ok(connection)
}

async fn password_login(
    server_list: &ServerList,
    account: &str,
    password: &str,
    confirmation: impl AuthConfirmationHandler + Send + Sync,
) -> Result<Connection> {
    let conn = Connection::login(
        server_list,
        account,
        password,
        FileGuardDataStore::user_cache(), // TODO: use steam-multiversion-viewer cache
        confirmation,
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
