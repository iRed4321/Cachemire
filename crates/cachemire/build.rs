fn main() {
    // EmbedFiles bundles the fonts and the compiled translations
    // (`resources/lang/<code>/LC_MESSAGES/cachemire.po`) into the executable.
    println!("cargo:rerun-if-changed=../../resources/lang");
    let config = slint_build::CompilerConfiguration::new()
        .with_edition((1, 19))
        .embed_resources(slint_build::EmbedResourcesKind::EmbedFiles)
        .with_bundled_translations("../../resources/lang")
        .with_default_translation_context(slint_build::DefaultTranslationContext::None);
    slint_build::compile_with_config("ui/main.slint", config).expect("compile the Slint UI");

    // Windows executable resources: the app icon (Explorer, the installer's shortcut, and the
    // window's icons, loaded by its id in main.rs) plus version info from Cargo.toml.
    #[cfg(windows)]
    {
        println!("cargo:rerun-if-changed=../../resources/icon.ico");
        let mut res = winresource::WindowsResource::new();
        res.set_icon_with_id("../../resources/icon.ico", "1");
        res.set("ProductName", "Cachemire");
        res.set("FileDescription", "Cachemire");
        res.set("CompanyName", "Léo Parent");
        res.set("LegalCopyright", "© 2026 Léo Parent, GPL-3.0");
        res.compile().expect("failed to compile the Windows resources (icon / version info)");
    }
}
