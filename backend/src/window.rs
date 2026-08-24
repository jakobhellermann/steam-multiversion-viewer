// TODO(ai-review): review for style and correctness
use tao::dpi::LogicalSize;
use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoop, EventLoopBuilder};
use tao::window::{Window, WindowBuilder};
use wry::WebViewBuilder;

use crate::state::AppState;

const TITLE: &str = "Steam Multiversion Viewer";

enum UserEvent {
    #[cfg(target_os = "macos")]
    Menu(muda::MenuEvent),
    #[cfg(target_os = "macos")]
    Library(Vec<crate::routes::library::OwnedGame>),
}

fn build_window(event_loop: &EventLoop<UserEvent>) -> Window {
    WindowBuilder::new()
        .with_title(TITLE)
        .with_inner_size(LogicalSize::new(1280.0, 850.0))
        .build(event_loop)
        .expect("failed to create window")
}

fn build_webview(window: &Window, url: &str) -> wry::WebView {
    let builder = WebViewBuilder::new()
        .with_url(url)
        .with_back_forward_navigation_gestures(true);

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
    let webview = builder.build(window).expect("failed to create webview");

    webview
}

/// Opens a native webview window on `url` and runs the event loop.
/// Must be called on the main thread; exits the process when the
/// window is closed.
#[cfg_attr(not(target_os = "macos"), allow(unused_variables))]
pub fn run(url: &str, state: AppState, handle: tokio::runtime::Handle) -> ! {
    let event_loop: EventLoop<UserEvent> = EventLoopBuilder::with_user_event().build();
    let window = build_window(&event_loop);
    let webview = build_webview(&window, url);
    let base_url = url.to_string();

    #[cfg(target_os = "macos")]
    let (_menu, library_menu) =
        macos::install_menu_bar(&event_loop.create_proxy()).expect("failed to install menu bar");
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
