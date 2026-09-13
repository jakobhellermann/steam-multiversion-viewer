fn main() {
    // Allow handling non-enabled ProjFS
    if std::env::var_os("CARGO_CFG_WINDOWS").is_some() {
        println!("cargo:rustc-link-arg=/DELAYLOAD:ProjectedFSLib.dll");
        println!("cargo:rustc-link-arg=delayimp.lib");
    }

    // Embed the app icon into the Windows exe — taskbar, explorer and the
    // native window all fall back to it (tao gets no explicit window icon).
    if std::env::var_os("CARGO_CFG_WINDOWS").is_some() {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("../frontend/public/favicon.ico");
        res.compile().expect("failed to embed windows icon");
    }
    println!("cargo:rerun-if-changed=../frontend/public/favicon.ico");
}
