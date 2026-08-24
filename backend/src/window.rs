use tao::dpi::LogicalSize;
use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoop};
use tao::window::WindowBuilder;
use wry::WebViewBuilder;

/// tao/wry don't create a menu bar automatically; without one there's no
/// Cmd+Q and WKWebView copy/paste shortcuts don't work.
#[cfg(target_os = "macos")]
fn install_menu_bar() -> muda::Result<(muda::Menu, Vec<muda::Submenu>)> {
    use muda::about_metadata::AboutMetadataBuilder;
    use muda::{Menu, PredefinedMenuItem, Submenu};

    let app_menu = Submenu::with_items(
        "Steam Multiversion Viewer",
        true,
        &[
            &PredefinedMenuItem::about(
                None,
                Some(
                    AboutMetadataBuilder::new()
                        .name(Some("Steam Multiversion Viewer"))
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
    menu.append_items(&[&app_menu, &edit_menu, &window_menu])?;
    menu.init_for_nsapp();

    Ok((menu, vec![app_menu, edit_menu, window_menu]))
}

/// Opens a native webview window on `url` and runs the event loop.
/// Must be called on the main thread; exits the process when the
/// window is closed.
pub fn run(url: &str) -> ! {
    let event_loop = EventLoop::new();
    let window = WindowBuilder::new()
        .with_title("Steam Multiversion Viewer")
        .with_inner_size(LogicalSize::new(1280.0, 850.0))
        .build(&event_loop)
        .expect("failed to create window");
    let builder = WebViewBuilder::new()
        .with_url(url)
        .with_back_forward_navigation_gestures(true);
    // On Linux wry needs the GTK container; build(&window) via raw-window-handle
    // returns UnsupportedWindowHandle under the Wayland GDK backend.
    #[cfg(not(target_os = "linux"))]
    let webview = builder.build(&window).expect("failed to create webview");
    #[cfg(target_os = "linux")]
    let webview = {
        use tao::platform::unix::WindowExtUnix;
        use wry::WebViewBuilderExtUnix;
        let vbox = window
            .default_vbox()
            .expect("tao window has no default vbox");
        builder.build_gtk(vbox).expect("failed to create webview")
    };

    #[cfg(target_os = "macos")]
    let _menu = install_menu_bar().expect("failed to install menu bar");

    event_loop.run(move |event, _, control_flow| {
        let _ = &webview;
        *control_flow = ControlFlow::Wait;
        if let Event::WindowEvent {
            event: WindowEvent::CloseRequested,
            ..
        } = event
        {
            *control_flow = ControlFlow::Exit;
        }
    })
}
