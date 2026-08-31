fn main() {
    // Allow handling non-enabled ProjFS
    if std::env::var_os("CARGO_CFG_WINDOWS").is_some() {
        println!("cargo:rustc-link-arg=/DELAYLOAD:ProjectedFSLib.dll");
        println!("cargo:rustc-link-arg=delayimp.lib");
    }
}
