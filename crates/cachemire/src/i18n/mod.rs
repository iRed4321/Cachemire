//! The interface language: `.slint` texts and Rust's `t!` messages are both
//! compiled in (bundled translations / `rust-i18n`), nothing loaded at runtime.

use cachemire_core::i18n::Language;

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
