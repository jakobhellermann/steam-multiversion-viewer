use tao::dpi::LogicalSize;
use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoop};
use tao::window::WindowBuilder;
use wry::WebViewBuilder;

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
