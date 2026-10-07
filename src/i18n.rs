//! The interface language: `.slint` texts and Rust's `t!` messages are both
//! compiled in (bundled translations / `rust-i18n`), nothing loaded at
//! runtime. Setting "" follows the system language; English is the fallback.

/// The languages the app is translated to: code and name, the name in that
/// language itself (it isn't translated).
pub const LANGUAGES: [(&str, &str); 2] = [("en", "English"), ("fr", "Français")];

const FALLBACK: &str = "en";

/// The language setting: follow the system, or one of the languages.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Language {
    #[default]
    Automatic,
    English,
    French,
}

impl Language {
    /// How the setting is saved: "" for the system's, else a language code.
    pub fn code(self) -> &'static str {
        match self {
            Language::Automatic => "",
            Language::English => "en",
            Language::French => "fr",
        }
    }

    /// The setting a saved code stands for; the system's for one we don't know
    /// (a file from a newer version, say).
    pub fn from_code(code: &str) -> Language {
        match code {
            "en" => Language::English,
            "fr" => Language::French,
            _ => Language::Automatic,
        }
    }

    /// The code of the language this setting means right now.
    fn resolve(self) -> &'static str {
        match self {
            Language::Automatic => system_language(),
            Language::English => "en",
            Language::French => "fr",
        }
    }
}

/// The base language of a locale name (`fr_CA.UTF-8` and `fr-CA` are `fr`), if the
/// app is translated to it.
fn supported(locale: &str) -> Option<&'static str> {
    let base = locale.split(['-', '_', '.', '@']).next().unwrap_or_default().to_ascii_lowercase();
    LANGUAGES.iter().map(|(code, _)| *code).find(|code| *code == base)
}

/// The language the system is set to, when the app speaks it; English otherwise.
pub fn system_language() -> &'static str {
    sys_locale::get_locale().as_deref().and_then(supported).unwrap_or(FALLBACK)
}

/// Switches to `language`: the messages built from now on, and the UI's texts
/// (which follow at once). Call it once the first window exists, which is when
/// Slint's translations become available.
pub fn apply(language: Language) {
    let code = language.resolve();
    rust_i18n::set_locale(code);
    if let Err(error) = slint::select_bundled_translation(code) {
        eprintln!("couldn't select the {code} translation of the interface: {error:?}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_locale_name_is_reduced_to_its_language() {
        assert_eq!(supported("fr"), Some("fr"));
        assert_eq!(supported("fr-FR"), Some("fr"));
        assert_eq!(supported("fr_CA.UTF-8"), Some("fr"));
        assert_eq!(supported("FR"), Some("fr"));
        assert_eq!(supported("en-US"), Some("en"));
        assert_eq!(supported("de-DE"), None);
        assert_eq!(supported(""), None);
    }

    /// `(msgid, msgstr)` of every message of a gettext file (the header, whose
    /// msgid is empty, left out).
    fn po_messages(text: &str) -> Vec<(String, String)> {
        fn unquote(line: &str) -> String {
            let inner = line.trim().trim_start_matches(|c: char| c.is_ascii_alphabetic()).trim();
            inner.trim_matches('"').replace("\\\"", "\"").replace("\\n", "\n")
        }
        let (mut messages, mut id, mut string, mut in_string) = (Vec::new(), String::new(), String::new(), false);
        for line in text.lines().chain(std::iter::once("")) {
            if line.starts_with("msgid ") || line.is_empty() {
                if !id.is_empty() {
                    messages.push((std::mem::take(&mut id), std::mem::take(&mut string)));
                }
                id.clear();
                string.clear();
                in_string = false;
                if line.starts_with("msgid ") {
                    id = unquote(line);
                }
            } else if line.starts_with("msgstr ") {
                in_string = true;
                string = unquote(line);
            } else if line.starts_with('"') {
                let part = line.trim().trim_matches('"').replace("\\\"", "\"").replace("\\n", "\n");
                if in_string { string.push_str(&part) } else { id.push_str(&part) }
            }
        }
        messages
    }

    #[test]
    fn every_message_of_the_interface_is_translated() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("lang");
        let template = po_messages(&std::fs::read_to_string(root.join("cachemire.pot")).unwrap());
        assert!(template.len() > 50, "the template lists the messages of the .slint files");
        for (code, _) in LANGUAGES.iter().filter(|(code, _)| *code != FALLBACK) {
            let path = root.join(code).join("LC_MESSAGES").join("cachemire.po");
            let translated = po_messages(&std::fs::read_to_string(&path).unwrap_or_else(|_| panic!("{} exists", path.display())));
            for (id, _) in &template {
                let Some((_, string)) = translated.iter().find(|(known, _)| known == id) else {
                    panic!("{code}: no entry for {id:?}; run cargo xtask i18n");
                };
                assert!(!string.is_empty(), "{code}: {id:?} is not translated");
                assert_eq!(id.matches("{}").count(), string.matches("{}").count(), "{code}: {id:?} and its translation take different arguments");
            }
        }
    }

    /// `(key, translations)` of every entry of locales/app.yml: the `fr:` line under each key.
    fn yaml_messages(text: &str) -> Vec<(String, std::collections::BTreeMap<String, String>)> {
        let mut messages: Vec<(String, std::collections::BTreeMap<String, String>)> = Vec::new();
        for line in text.lines() {
            if line.starts_with('"') {
                messages.push((line.trim_end_matches(':').trim_matches('"').to_string(), Default::default()));
            } else if let Some((code, string)) = line.trim().split_once(": \"")
                && line.starts_with("  ")
                && let Some(last) = messages.last_mut()
            {
                last.1.insert(code.to_string(), string.trim_end_matches('"').to_string());
            }
        }
        messages
    }

    /// The `%{name}` placeholders of a message.
    fn placeholders(text: &str) -> Vec<String> {
        let mut found: Vec<String> = text.split("%{").skip(1).filter_map(|rest| rest.split('}').next()).map(str::to_string).collect();
        found.sort();
        found
    }

    #[test]
    fn every_message_built_in_rust_is_translated() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let messages = yaml_messages(&std::fs::read_to_string(root.join("locales/app.yml")).unwrap());
        assert!(messages.len() > 50);
        for (key, translations) in &messages {
            for (code, _) in LANGUAGES.iter().filter(|(code, _)| *code != FALLBACK) {
                let string = translations.get(*code).unwrap_or_else(|| panic!("{code}: {key:?} is not translated"));
                assert_eq!(placeholders(key), placeholders(string), "{code}: {key:?} and its translation take different arguments");
            }
        }

        // and every message a `t!("...")` of the sources asks for is in the file
        let mut sources = Vec::new();
        let mut folders = vec![root.join("src")];
        while let Some(folder) = folders.pop() {
            for entry in std::fs::read_dir(folder).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    folders.push(path);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    sources.push(path);
                }
            }
        }
        for path in sources {
            let text = std::fs::read_to_string(&path).unwrap();
            for (at, _) in text.match_indices("t!(\"") {
                // (not the end of another macro's name, like format!)
                if text[..at].chars().next_back().is_some_and(|c| c.is_alphanumeric() || c == '_') {
                    continue;
                }
                let chunk = &text[at + 4..];
                let mut key = String::new();
                let mut chars = chunk.chars();
                while let Some(c) = chars.next() {
                    match c {
                        '"' => break,
                        '\\' => key.push(chars.next().unwrap_or_default()),
                        c => key.push(c),
                    }
                }
                // (the test of this very file writes its own keys)
                if path.ends_with("i18n.rs") {
                    continue;
                }
                assert!(messages.iter().any(|(known, _)| *known == key), "{}: t!({key:?}) has no entry in locales/app.yml", path.display());
            }
        }
    }

    #[test]
    fn english_is_what_a_missing_translation_shows() {
        use rust_i18n::t;
        assert_eq!(t!("Copied", locale = "fr"), "Copié");
        assert_eq!(t!("Copied", locale = "en"), "Copied");
        assert_eq!(t!("%{count} keys scanned", locale = "fr", count = 3), "3 clés analysées");
        assert_eq!(t!("A message nobody translated", locale = "fr"), "A message nobody translated");
    }

    #[test]
    fn a_setting_is_saved_as_a_code_and_read_back() {
        for language in [Language::Automatic, Language::English, Language::French] {
            assert_eq!(Language::from_code(language.code()), language);
        }
        assert_eq!(Language::from_code("de"), Language::Automatic, "a language we don't know follows the system");
        assert_eq!(Language::French.resolve(), "fr");
        assert!(LANGUAGES.iter().any(|(code, _)| *code == Language::Automatic.resolve()), "the system's language is one we speak");
        assert!(LANGUAGES.iter().all(|(code, _)| Language::from_code(code).resolve() == *code), "every language has a setting");
    }
}
