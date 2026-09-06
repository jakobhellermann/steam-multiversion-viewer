// TODO(ai-review): review for style and correctness
use std::sync::Arc;

use tao::dpi::LogicalSize;
use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoop, EventLoopBuilder, EventLoopProxy};
use tao::window::{Window, WindowBuilder};
use wry::WebViewBuilder;

use steam_multiversion_viewer::state::AppState;

#[cfg(target_os = "windows")]
use crate::config::VibrancyEffect;

const TITLE: &str = "Steam Multiversion Viewer";

enum UserEvent {
    #[cfg(target_os = "macos")]
    Menu(muda::MenuEvent),
    #[cfg(target_os = "macos")]
    Library(Vec<crate::routes::library::OwnedGame>),
    /// The frontend requested a live backdrop change (settings page).
    #[cfg(target_os = "windows")]
    Vibrancy(VibrancyEffect),
}

#[cfg(target_os = "windows")]
fn parse_effect(s: &str) -> Option<VibrancyEffect> {
    Some(match s {
        "none" => VibrancyEffect::None,
        "mica" => VibrancyEffect::Mica,
        "acrylic" => VibrancyEffect::Acrylic,
        _ => return Option::None,
    })
}

/// Marks the native window for the frontend; `active` = backdrop live this
/// session (fixed at window creation, so toggling `none` needs a restart).
#[cfg(target_os = "windows")]
fn vibrancy_init_script(active: bool) -> String {
    format!("window.__vibrancy = {{ active: {active} }};")
}

#[cfg_attr(not(target_os = "windows"), allow(unused_variables))]
fn build_window(event_loop: &EventLoop<UserEvent>, transparent: bool) -> Window {
    let builder = WindowBuilder::new()
        .with_title(TITLE)
        .with_inner_size(LogicalSize::new(1280.0, 850.0));
    // Only transparent when an effect is on — it can't be toggled after creation.
    #[cfg(target_os = "windows")]
    let builder = builder.with_transparent(transparent);
    builder.build(event_loop).expect("failed to create window")
}

#[cfg_attr(not(target_os = "windows"), allow(unused_variables))]
fn build_webview(
    window: &Window,
    url: &str,
    proxy: EventLoopProxy<UserEvent>,
    transparent: bool,
) -> wry::WebView {
    #[cfg(target_os = "windows")]
    let mut web_context = wry::WebContext::new(Some(webview_data_dir()));
    #[cfg(target_os = "windows")]
    let builder = WebViewBuilder::new_with_web_context(&mut web_context)
        .with_url(url)
        .with_back_forward_navigation_gestures(true);
    #[cfg(not(target_os = "windows"))]
    let builder = WebViewBuilder::new()
        .with_url(url)
        .with_back_forward_navigation_gestures(true);

    #[cfg(target_os = "windows")]
    let builder = builder
        .with_transparent(transparent)
        .with_initialization_script(vibrancy_init_script(transparent))
        .with_ipc_handler(move |req| {
            if let Some(effect) = parse_effect(req.body()) {
                let _ = proxy.send_event(UserEvent::Vibrancy(effect));
            }
        });

    // On Linux wry needs the GTK container; build(&window) via raw-window-handle
    // returns UnsupportedWindowHandle under the Wayland GDK backend.
    #[cfg(target_os = "linux")]
    let webview = {
        use tao::platform::unix::WindowExtUnix;
        use wry::WebViewBuilderExtUnix;
        let vbox = window
            .default_vbox()
            .expect("tao window has no default vbox");
        builder.build_gtk(vbox).expect("failed to create webview")
    };
    #[cfg(not(target_os = "linux"))]
    let webview = builder.build(window).unwrap_or_else(|e| {
        tracing::error!(%e, "failed to create webview");
        panic!("failed to create webview: {e}");
    });

    webview
}

/// WebView2 defaults its user data folder to the exe's directory, which is
/// read-only under Program Files — environment creation then fails with
/// E_ACCESSDENIED. Keep the data in a per-user directory instead.
#[cfg(target_os = "windows")]
fn webview_data_dir() -> std::path::PathBuf {
    directories::ProjectDirs::from("", "", "steam-multiversion-viewer")
        .map(|d| d.cache_dir().join("webview"))
        .unwrap_or_else(|| std::env::temp_dir().join("steam-multiversion-viewer"))
}

/// Opens a native webview window on `url` and runs the event loop.
/// Must be called on the main thread; exits the process when the
/// window is closed.
pub fn run(url: &str, state: AppState, handle: tokio::runtime::Handle) -> ! {
    let event_loop: EventLoop<UserEvent> = EventLoopBuilder::with_user_event().build();

    // The backdrop effect is baked into the window at creation (transparency
    // can't be toggled afterward), so read the persisted choice up front.
    #[cfg(target_os = "windows")]
    let vibrancy = state.config.load().vibrancy_effect;
    #[cfg(target_os = "windows")]
    let vibrancy_active = vibrancy != VibrancyEffect::None;
    #[cfg(not(target_os = "windows"))]
    let vibrancy_active = false;

    let window = Arc::new(build_window(&event_loop, vibrancy_active));
    #[cfg_attr(not(target_os = "macos"), allow(unused_variables))]
    let webview = build_webview(&window, url, event_loop.create_proxy(), vibrancy_active);

    // Paints over the opaque white default so the backdrop shows (see windows_vibrancy).
    #[cfg(target_os = "windows")]
    let mut surface = vibrancy_active
        .then(|| windows_vibrancy::create_surface(&window))
        .flatten();
    #[cfg(target_os = "windows")]
    if vibrancy_active {
        windows_vibrancy::apply(&window, vibrancy);
    }
    #[cfg(target_os = "macos")]
    let base_url = url.to_string();

    #[cfg(target_os = "macos")]
    let (_menu, library_menu) =
        macos::install_menu_bar(&event_loop.create_proxy()).expect("failed to install menu bar");
    let teardown_state = state.clone();
    #[cfg(target_os = "macos")]
    macos::spawn_library_fetch(state, event_loop.create_proxy(), &handle);

    // `run` diverges, so the window, webview and menu are never dropped.
    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;

        match event {
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => *control_flow = ControlFlow::Exit,

            #[cfg(target_os = "macos")]
            Event::UserEvent(UserEvent::Menu(menu_event)) => {
                macos::handle_menu_event(&menu_event, &webview, &base_url);
            }

            #[cfg(target_os = "macos")]
            Event::UserEvent(UserEvent::Library(games)) => {
                macos::populate_library_menu(&library_menu, games);
            }

            #[cfg(target_os = "windows")]
            Event::UserEvent(UserEvent::Vibrancy(effect)) => {
                windows_vibrancy::apply(&window, effect);
            }

            #[cfg(target_os = "windows")]
            Event::RedrawRequested(_) => {
                if let Some(surface) = surface.as_mut() {
                    windows_vibrancy::draw_surface(&window, surface);
                }
            }

            // Closing the window ends the process without unwinding, so
            // an active mount has to come down here or it outlives us
            // and hangs every access to it.
            Event::LoopDestroyed => {
                handle.block_on(teardown_state.mount.stop_on_shutdown());
            }

            _ => {}
        }
    })
}

#[cfg(target_os = "macos")]
mod macos {
    use super::{TITLE, UserEvent};
    use crate::routes::library::{OwnedGame, fetch_owned_games};
    use crate::state::AppState;
    use muda::{Menu, MenuItem, PredefinedMenuItem, Submenu};
    use std::borrow::Cow;
    use std::time::Duration;
    use tao::event_loop::EventLoopProxy;

    /// tao/wry don't create a menu bar automatically; without one there's no
    /// Cmd+Q and WKWebView copy/paste shortcuts don't work.
    ///
    /// The returned `Menu` owns every submenu, so only the library submenu —
    /// which is repopulated later — is handed back separately.
    pub fn install_menu_bar(proxy: &EventLoopProxy<UserEvent>) -> muda::Result<(Menu, Submenu)> {
        use muda::about_metadata::AboutMetadataBuilder;

        let app_menu = Submenu::with_items(
            TITLE,
            true,
            &[
                &PredefinedMenuItem::about(
                    None,
                    Some(
                        AboutMetadataBuilder::new()
                            .name(Some(TITLE))
                            .version(Some(env!("CARGO_PKG_VERSION")))
                            .build(),
                    ),
                ),
                &PredefinedMenuItem::separator(),
                &PredefinedMenuItem::services(None),
                &PredefinedMenuItem::separator(),
                &PredefinedMenuItem::hide(None),
                &PredefinedMenuItem::hide_others(None),
                &PredefinedMenuItem::show_all(None),
                &PredefinedMenuItem::separator(),
                &MenuItem::with_id("open-in-browser", "Open in Browser", true, None),
                &PredefinedMenuItem::separator(),
                &PredefinedMenuItem::quit(None),
            ],
        )?;

        let edit_menu = Submenu::with_items(
            "Edit",
            true,
            &[
                &PredefinedMenuItem::undo(None),
                &PredefinedMenuItem::redo(None),
                &PredefinedMenuItem::separator(),
                &PredefinedMenuItem::cut(None),
                &PredefinedMenuItem::copy(None),
                &PredefinedMenuItem::paste(None),
                &PredefinedMenuItem::select_all(None),
            ],
        )?;

        let library_menu =
            Submenu::with_items("Library", true, &[&MenuItem::new("Loading…", false, None)])?;

        let window_menu = Submenu::with_items(
            "Window",
            true,
            &[
                &PredefinedMenuItem::minimize(None),
                &PredefinedMenuItem::fullscreen(None),
                &PredefinedMenuItem::separator(),
                &PredefinedMenuItem::bring_all_to_front(None),
            ],
        )?;

        let menu = Menu::new();
        menu.append_items(&[&app_menu, &edit_menu, &library_menu, &window_menu])?;
        menu.init_for_nsapp();

        let proxy = proxy.clone();
        muda::MenuEvent::set_event_handler(Some(move |event| {
            let _ = proxy.send_event(UserEvent::Menu(event));
        }));

        Ok((menu, library_menu))
    }

    /// Polls the Steam library until the client is logged in, then sends the
    /// game list to the main thread.
    pub fn spawn_library_fetch(
        state: AppState,
        proxy: EventLoopProxy<UserEvent>,
        handle: &tokio::runtime::Handle,
    ) {
        const FIRST_RETRY: Duration = Duration::from_secs(2);
        // The user may log in at any point, so keep retrying forever, but back
        // off so a permanent failure doesn't hammer Steam every two seconds.
        const MAX_RETRY: Duration = Duration::from_secs(60);

        handle.spawn(async move {
            let mut retry_in = FIRST_RETRY;
            loop {
                match fetch_owned_games(&state).await {
                    Ok(games) => {
                        let _ = proxy.send_event(UserEvent::Library(games));
                        return;
                    }
                    Err(err) => {
                        tracing::debug!(
                            status = %err.status,
                            error = %err.message,
                            retry_in_secs = retry_in.as_secs(),
                            "menu bar library fetch failed"
                        );
                        tokio::time::sleep(retry_in).await;
                        retry_in = (retry_in * 2).min(MAX_RETRY);
                    }
                }
            }
        });
    }

    pub fn populate_library_menu(menu: &Submenu, mut games: Vec<OwnedGame>) {
        while let Some(item) = menu.remove_at(0) {
            drop(item);
        }

        if games.is_empty() {
            menu.append(&MenuItem::new("No games found", false, None))
                .ok();
            return;
        }

        games.sort_by(|a, b| {
            b.playtime_minutes
                .cmp(&a.playtime_minutes)
                .then_with(|| a.name.cmp(&b.name))
        });
        for game in games {
            let item = MenuItem::with_id(
                format!("app:{}", game.appid),
                escape_mnemonics(&game.name),
                true,
                None,
            );
            menu.append(&item).ok();
        }
    }

    /// muda treats `&` as a mnemonic marker and strips it from item labels
    /// (`&&` renders as one `&`), which would mangle names like "Sam & Max".
    fn escape_mnemonics(name: &str) -> Cow<'_, str> {
        if name.contains('&') {
            Cow::Owned(name.replace('&', "&&"))
        } else {
            Cow::Borrowed(name)
        }
    }

    pub fn handle_menu_event(event: &muda::MenuEvent, webview: &wry::WebView, base_url: &str) {
        let id = event.id().as_ref();
        if let Some(appid) = id.strip_prefix("app:") {
            // Route through the SPA router when it is up; assigning `location`
            // would re-bootstrap the whole frontend on every menu pick.
            let js = format!(
                r"(window.__navigate ?? ((to) => {{ window.location.href = to; }}))('/apps/{appid}')"
            );
            if let Err(err) = webview.evaluate_script(&js) {
                tracing::warn!(error = %err, "menu bar navigation failed");
            }
        } else if id == "open-in-browser" {
            let url = webview
                .url()
                .ok()
                .filter(|url| url.starts_with(base_url))
                .unwrap_or_else(|| base_url.to_string());
            if let Err(err) = open::that_detached(&url) {
                tracing::warn!(error = %err, "failed to open browser");
            }
        }
    }
}

#[cfg(target_os = "windows")]
mod windows_vibrancy {
    use std::sync::Arc;

    use super::VibrancyEffect;
    use tao::window::Window;
    use window_vibrancy::{apply_acrylic, apply_mica, clear_acrylic, clear_mica};

    // Legacy acrylic color for pre-22H2 Windows; the modern backdrop ignores it
    // (there the darkening comes from the frontend tint instead).
    const TINT: (u8, u8, u8, u8) = (15, 23, 42, 125);

    type Surface = softbuffer::Surface<Arc<Window>, Arc<Window>>;

    /// Owns the window's surface and clears it to transparent — a transparent
    /// window otherwise keeps an opaque white redirection bitmap that the
    /// WebView2 composites over, hiding the backdrop. Repaint on `RedrawRequested`.
    pub fn create_surface(window: &Arc<Window>) -> Option<Surface> {
        let context = softbuffer::Context::new(window.clone()).ok()?;
        let mut surface = softbuffer::Surface::new(&context, window.clone()).ok()?;
        draw_surface(window, &mut surface);
        Some(surface)
    }

    pub fn draw_surface(window: &Window, surface: &mut Surface) {
        let size = window.inner_size();
        let (Some(width), Some(height)) = (
            std::num::NonZeroU32::new(size.width),
            std::num::NonZeroU32::new(size.height),
        ) else {
            return;
        };
        if surface.resize(width, height).is_err() {
            return;
        }
        let Ok(mut buffer) = surface.buffer_mut() else {
            return;
        };
        buffer.fill(0);
        let _ = buffer.present();
    }

    pub fn apply(window: &Window, effect: VibrancyEffect) {
        // Clear first so switching between effects is idempotent; clearing an
        // inactive effect is a no-op on Windows.
        let _ = clear_acrylic(window);
        let _ = clear_mica(window);

        let result = match effect {
            VibrancyEffect::None => Ok(()),
            VibrancyEffect::Mica => apply_mica(window, Some(true)),
            VibrancyEffect::Acrylic => apply_acrylic(window, Some(TINT)),
        };
        if let Err(err) = result {
            tracing::warn!(?effect, error = %err, "failed to apply window vibrancy");
        }
    }
}
