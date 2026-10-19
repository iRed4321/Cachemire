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
