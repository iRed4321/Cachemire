use super::{glob_match, id_matches, search_pattern};

fn matches(pattern: &str, text: &str) -> bool {
    glob_match(pattern.as_bytes(), text.as_bytes())
}

#[test]
fn stars_and_question_marks_match_like_redis() {
    assert!(matches("*", ""));
    assert!(matches("*", "anything"));
    assert!(matches("user:*", "user:42"));
    assert!(!matches("user:*", "users:42"));
    assert!(matches("*:42", "user:42"));
    assert!(matches("*a*b*", "xxaxxbxx"));
    assert!(!matches("*a*b*", "xxbxxaxx"));
    assert!(matches("a*a*a", "aaaaa"));
    assert!(!matches("a*a*a", "aa"));
    assert!(matches("h?llo", "hello"));
    assert!(!matches("h?llo", "hllo"));
    assert!(!matches("", "x"));
    assert!(matches("", ""));
}

#[test]
fn classes_take_ranges_negation_and_escapes() {
    assert!(matches("id:[0-9]", "id:7"));
    assert!(!matches("id:[0-9]", "id:x"));
    assert!(matches("id:[9-0]", "id:5"), "a reversed range still covers its span");
    assert!(matches("[^a]bc", "xbc"));
    assert!(!matches("[^a]bc", "abc"));
    assert!(matches("[abc]", "b"));
    assert!(matches("[a-]", "-"), "a dash before ] is itself");
    assert!(matches(r"[\]]", "]"));
}

#[test]
fn a_backslash_takes_the_next_character_literally() {
    assert!(matches(r"a\*b", "a*b"));
    assert!(!matches(r"a\*b", "axxb"));
    assert!(matches(r"\?", "?"));
    assert!(!matches(r"\?", "x"));
}

#[test]
fn a_pattern_still_being_typed_never_panics() {
    // the search box searches as each character is typed
    for pattern in ["[", "a[", "a[b", "user:[0-", "[^", "\\", "*[", "[]"] {
        for text in ["", "a", "ab", "user:0", "["] {
            let _ = matches(pattern, text);
        }
    }
    assert!(matches("a[b", "ab"), "an unclosed class ends with the pattern");
}

#[test]
fn keys_that_arent_utf8_match_on_their_bytes() {
    // `bin:\xff` is the id of the four bytes `bin:` and one byte 0xff (see KeyName::id)
    assert!(id_matches(b"bin:?", r"bin:\xff"), "the 0xff byte is one character");
    assert!(!id_matches(b"bin:????", r"bin:\xff"), "not the four characters of its escape");
    assert!(id_matches(br"*\\*", r"a\\b"), "a backslash in the name is one byte");
}

#[test]
fn plain_text_searches_contain_globs_are_used_as_is() {
    assert_eq!(search_pattern("session"), "*session*");
    assert_eq!(search_pattern("user:42"), "*user:42*");
    assert_eq!(search_pattern("user:*"), "user:*");
    assert_eq!(search_pattern("h?llo"), "h?llo");
    assert_eq!(search_pattern("id:[0-9]"), "id:[0-9]");
    assert_eq!(search_pattern(r"a\*b"), r"a\*b");
}
