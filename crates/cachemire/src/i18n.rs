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
        tracing::warn!("couldn't select the {code} translation of the interface: {error:?}");
    }
}

#[cfg(test)]
mod tests;
