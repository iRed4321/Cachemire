fn main() {
    // the version shown and set on the exe: CACHEMIRE_VERSION, as `cargo xtask build-installer-test` passes it
    println!("cargo:rerun-if-env-changed=CACHEMIRE_VERSION");
    #[cfg(windows)]
    {
        println!("cargo:rerun-if-changed=../assets/icon.ico");
        let version = std::env::var("CACHEMIRE_VERSION").unwrap_or_else(|_| env!("CARGO_PKG_VERSION").to_string());
        let mut res = winresource::WindowsResource::new();
        res.set_icon_with_id("../assets/icon.ico", "1");
        res.set("ProductName", "Cachemire");
        res.set("FileDescription", "Cachemire - installer test");
        res.set("FileVersion", &version);
        res.set("ProductVersion", &version);
        res.compile().expect("failed to compile the Windows resources (icon / version info)");
    }
}
