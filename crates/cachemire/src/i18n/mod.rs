//! The interface language: `.slint` texts and Rust's `t!` messages are both
//! compiled in (bundled translations / the table `build.rs` makes of
//! `resources/lang/app.yml`), nothing loaded at runtime.

use std::borrow::Cow;
use std::fmt::{Display, Write};

use cachemire_core::i18n::Language;
use parking_lot::RwLock;

// MESSAGES: every key of app.yml (the English text), sorted, with its `(code, text)` translations
include!(concat!(env!("OUT_DIR"), "/messages.rs"));

/// The language `t!` builds its messages in.
static LOCALE: RwLock<&'static str> = RwLock::new("en");

/// A message of `resources/lang/app.yml` (a literal key) in the language set, its `%{name}`
/// placeholders filled by the `name = value` arguments; a first `locale = "fr"` picks another
/// language. A key with no translation reads as itself, the English text.
#[macro_export]
macro_rules! t {
    ($key:literal $(,)?) => {
        ::std::borrow::Cow::<'static, str>::Borrowed($crate::i18n::translate($crate::i18n::locale(), $key))
    };
    ($key:literal, locale = $locale:expr $(, $name:ident = $value:expr)* $(,)?) => {
        $crate::i18n::fill(
            $crate::i18n::translate($locale, $key),
            &[$((stringify!($name), &$value as &dyn ::std::fmt::Display)),*],
        )
    };
    ($key:literal $(, $name:ident = $value:expr)+ $(,)?) => {
        $crate::i18n::fill(
            $crate::i18n::translate($crate::i18n::locale(), $key),
            &[$((stringify!($name), &$value as &dyn ::std::fmt::Display)),*],
        )
    };
}

/// The language `t!` builds its messages in (`en` until `set_locale`).
pub fn locale() -> &'static str {
    *LOCALE.read()
}

/// Makes `code` the language of the messages built from now on.
pub fn set_locale(code: &'static str) {
    *LOCALE.write() = code;
}

/// `key` in `locale`: its translation, else its `en` entry, else the key itself.
pub fn translate(locale: &str, key: &'static str) -> &'static str {
    let Ok(at) = MESSAGES.binary_search_by(|(known, _)| known.cmp(&key)) else { return key };
    let translations = MESSAGES[at].1;
    let find = |code: &str| translations.iter().find(|(known, _)| *known == code).map(|(_, text)| *text);
    find(locale).or_else(|| find("en")).unwrap_or(key)
}

/// `text` with each `%{name}` replaced by the value `args` gives `name`; one it doesn't stays as written.
pub fn fill(text: &'static str, args: &[(&str, &dyn Display)]) -> Cow<'static, str> {
    if !text.contains("%{") {
        return Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("%{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find('}') else {
            rest = &rest[start..];
            break;
        };
        match args.iter().find(|(name, _)| *name == &after[..end]) {
            Some((_, value)) => {
                let _ = write!(out, "{value}");
            }
            None => out.push_str(&rest[start..start + 2 + end + 1]),
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    Cow::Owned(out)
}

/// Switches to `language`: the messages built from now on, and the UI's texts
/// (which follow at once). Call it once the first window exists, which is when
/// Slint's translations become available.
pub fn apply(language: Language) {
    let code = language.resolve();
    set_locale(code);
    if let Err(error) = slint::select_bundled_translation(code) {
        tracing::warn!("couldn't select the {code} translation of the interface: {error:?}");
    }
}

#[cfg(test)]
mod tests;
