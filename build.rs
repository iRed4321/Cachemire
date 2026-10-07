fn main() {
    // EmbedFiles bundles the fonts and the compiled translations
    // (`lang/<code>/LC_MESSAGES/cachemire.po`) into the executable.
    println!("cargo:rerun-if-changed=lang");
    let config = slint_build::CompilerConfiguration::new()
        .embed_resources(slint_build::EmbedResourcesKind::EmbedFiles)
        .with_bundled_translations("lang")
        .with_default_translation_context(slint_build::DefaultTranslationContext::None);
    slint_build::compile_with_config("ui/main.slint", config).unwrap();
    theme_colors();

    // Windows executable resources: the app icon (shown in Explorer, the
    // taskbar and the installer's shortcut) plus version info from Cargo.toml.
    #[cfg(windows)]
    {
        println!("cargo:rerun-if-changed=assets/icon.ico");
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icon.ico");
        res.set("ProductName", "Cachemire");
        res.set("FileDescription", "Cachemire - Redis key explorer");
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
