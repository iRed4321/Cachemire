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
    messages();

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

/// The messages `t!` builds (`resources/lang/app.yml`) as `OUT_DIR/messages.rs`: `MESSAGES`, each key
/// (the English text) with its `(code, text)` translations, sorted by key for `i18n::translate`.
fn messages() {
    let source = "../../resources/lang/app.yml";
    println!("cargo:rerun-if-changed={source}");
    let text = std::fs::read_to_string(source).expect("read resources/lang/app.yml");
    let entries: std::collections::BTreeMap<String, serde_norway::Value> = serde_norway::from_str(&text).expect("parse app.yml");
    let mut code = String::from("static MESSAGES: &[(&str, &[(&str, &str)])] = &[\n");
    for (key, translations) in entries.iter().filter(|(key, _)| !key.starts_with('_')) {
        let translations = translations.as_mapping().unwrap_or_else(|| panic!("app.yml: {key:?} holds no translations"));
        let pairs: Vec<String> = translations
            .iter()
            .map(|(lang, text)| match (lang.as_str(), text.as_str()) {
                (Some(lang), Some(text)) => format!("({lang:?}, {text:?})"),
                _ => panic!("app.yml: {key:?} has a translation that isn't text"),
            })
            .collect();
        code.push_str(&format!("    ({key:?}, &[{}]),\n", pairs.join(", ")));
    }
    code.push_str("];\n");
    let out = std::path::Path::new(&std::env::var("OUT_DIR").expect("OUT_DIR is set")).join("messages.rs");
    std::fs::write(out, code).expect("write messages.rs");
}
