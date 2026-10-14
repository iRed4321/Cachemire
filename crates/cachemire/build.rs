fn main() {
    // EmbedFiles bundles the fonts and the compiled translations
    // (`resources/lang/<code>/LC_MESSAGES/cachemire.po`) into the executable.
    println!("cargo:rerun-if-changed=../../resources/lang");
    let config = slint_build::CompilerConfiguration::new()
        .embed_resources(slint_build::EmbedResourcesKind::EmbedFiles)
        .with_bundled_translations("../../resources/lang")
        .with_default_translation_context(slint_build::DefaultTranslationContext::None);
    slint_build::compile_with_config("ui/main.slint", config).unwrap();
    theme_colors();

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

/// Writes `theme_colors.rs` to OUT_DIR: a `Color` const per `Theme` color given
/// as a plain `#rrggbb` in theme.slint (`json-key` → `JSON_KEY`), for Rust code.
fn theme_colors() {
    const THEME: &str = "ui/components/theme.slint";
    println!("cargo:rerun-if-changed={THEME}");
    let source = std::fs::read_to_string(THEME).expect("read theme.slint");
    let mut out = String::new();
    for line in source.lines() {
        let Some(rest) = line.trim().strip_prefix("out property <color> ") else { continue };
        let Some((name, value)) = rest.split_once(": #") else { continue };
        let Some(hex) = value.strip_suffix(';').filter(|hex| hex.len() == 6) else { continue };
        let channel = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).expect("hex color");
        let name = name.replace('-', "_").to_uppercase();
        out += &format!("pub const {name}: slint::Color = slint::Color::from_rgb_u8({}, {}, {});\n", channel(0), channel(2), channel(4));
    }
    std::fs::write(std::path::Path::new(&std::env::var("OUT_DIR").unwrap()).join("theme_colors.rs"), out).expect("write theme_colors.rs");
}
