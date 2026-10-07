//! Autocompletion of the filter bar's path from the loaded values: `records.`
//! lists the keys under `records` (plus `*` and array indexes), what follows the dot
//! narrows it, and `..` lists keys at any depth below.

use std::sync::{Arc, Mutex};

use serde_json::Value;

use crate::backend::LockExt;
use crate::json_path::{self, JsonPath};

use super::field::Field;

/// The most items an array offers as indexes (`*` covers the rest).
const MAX_INDEXES: usize = 25;
/// The most candidates kept for one position, and shown.
const MAX_CANDIDATES: usize = 2000;
const MAX_SHOWN: usize = 200;
/// The most values looked at when listing keys at any depth.
const MAX_VISITS: usize = 200_000;

/// One suggestion: the key as the values spell it, with the part of it that
/// the typed text matches.
#[derive(Debug, PartialEq)]
pub(super) struct Item {
    pub(super) key: String,
    /// byte range of `key` that matches what was typed (empty when nothing was)
    pub(super) matched: (usize, usize),
}

/// The list for the caret, and what accepting one of its items does to the text.
#[derive(Debug)]
pub(super) struct Completion {
    pub(super) items: Vec<Item>,
    /// the text the accepted item replaces: the word being typed, whole
    word: (usize, usize),
    /// the word follows a single `.` (not `..`), which a quoted key drops
    after_dot: bool,
}

/// What accepting an item makes of the filter text, and where the caret goes.
#[derive(Debug, PartialEq)]
pub(super) struct Edit {
    pub(super) text: String,
    pub(super) cursor: usize,
}

impl Completion {
    pub(super) fn accept(&self, text: &str, index: usize) -> Option<Edit> {
        let item = self.items.get(index)?;
        let (mut start, end) = self.word;
        let inserted = match quoted(&item.key) {
            Quoted::Plain => item.key.clone(),
            Quoted::Bracket(quoted) => {
                // `a.b` → `a['b.c']`: the bracket form takes the place of the dot
                if self.after_dot {
                    start -= 1;
                }
                quoted
            }
            Quoted::Impossible => return None,
        };
        let mut edited = String::with_capacity(text.len() + inserted.len());
        edited.push_str(&text[..start]);
        edited.push_str(&inserted);
        let cursor = edited.len();
        edited.push_str(&text[end..]);
        Some(Edit { text: edited, cursor })
    }
}

enum Quoted {
    /// can be typed as it is
    Plain,
    Bracket(String),
    /// holds both kinds of quote
    Impossible,
}

/// How a key is written in a path: plainly, unless it holds what ends a name.
fn quoted(key: &str) -> Quoted {
    let plain = key == "*" || !(key.is_empty() || key.starts_with('$') || key.trim() != key || key.contains(['.', '[', ']']));
    if plain {
        Quoted::Plain
    } else if !key.contains('"') {
        Quoted::Bracket(format!("[\"{key}\"]"))
    } else if !key.contains('\'') {
        Quoted::Bracket(format!("['{key}']"))
    } else {
        Quoted::Impossible
    }
}

/// Where the caret is in a path being typed.
struct Position {
    parent: String,
    descend: bool,
    /// the word being typed: the text before the caret, then up to the end of the word
    word: (usize, usize),
    typed: (usize, usize),
    after_dot: bool,
}

fn position(text: &str, cursor: usize) -> Option<Position> {
    let cursor = (0..=cursor.min(text.len())).rev().find(|i| text.is_char_boundary(*i))?;
    let before = &text[..cursor];
    let end = cursor + text[cursor..].find(['.', '[', ']']).unwrap_or(text.len() - cursor);
    match before.rfind(['.', '[', ']']) {
        // the first segment; `$` must be followed by a separator
        None if before.trim().is_empty() || before.starts_with('$') => None,
        None => Some(Position { parent: String::new(), descend: false, word: (0, end), typed: (0, cursor), after_dot: false }),
        Some(at) if before[at..].starts_with('.') => {
            let head = &before[..at];
            let descend = head.ends_with('.');
            let parent = if descend { &head[..head.len() - 1] } else { head };
            Some(Position { parent: parent.to_string(), descend, word: (at + 1, end), typed: (at + 1, cursor), after_dot: !descend })
        }
        Some(_) => None,
    }
}

/// A previous `candidates()` answer: the position it was asked for (parent
/// path, descend flag) and what it found, kept in case the next position
/// asked is the same one.
type KeptCandidates = (String, bool, Arc<Vec<String>>);

/// The keys under `parent` in every field, in the order they are met. Asked
/// for again with the same position, the answer is kept.
#[derive(Default)]
pub(super) struct Suggester {
    kept: Mutex<Option<KeptCandidates>>,
}

impl Suggester {
    /// The fields changed: what was kept is of the previous ones.
    pub(super) fn forget(&self) {
        *self.kept.lock_recover() = None;
    }

    pub(super) fn complete(&self, fields: &[Arc<Field>], text: &str, cursor: usize) -> Option<Completion> {
        let position = position(text, cursor)?;
        let candidates = self.candidates(fields, &position)?;
        let typed = text[position.typed.0..position.typed.1].trim_start();
        let items = narrow(&candidates, typed);
        // a list of just what is already typed has nothing to add
        if items.is_empty() || (items.len() == 1 && items[0].key == typed) {
            return None;
        }
        Some(Completion { items, word: position.word, after_dot: position.after_dot })
    }

    fn candidates(&self, fields: &[Arc<Field>], position: &Position) -> Option<Arc<Vec<String>>> {
        let mut kept = self.kept.lock_recover();
        if let Some((parent, descend, keys)) = kept.as_ref()
            && *parent == position.parent
            && *descend == position.descend
        {
            return Some(keys.clone());
        }
        let parent = if position.parent.trim().is_empty() { None } else { Some(json_path::parse(&position.parent).ok()?) };
        let keys = Arc::new(keys_under(fields, parent.as_ref(), position.descend));
        *kept = Some((position.parent.clone(), position.descend, keys.clone()));
        Some(keys)
    }
}

/// What can follow `parent` (`None`: the values themselves), across `fields`:
/// `*` first, then keys and indexes as they are met.
fn keys_under(fields: &[Arc<Field>], parent: Option<&JsonPath>, descend: bool) -> Vec<String> {
    let mut keys: Vec<String> = Vec::new();
    let mut seen = rustc_hash::FxHashSet::default();
    let mut add = |key: &str, keys: &mut Vec<String>| {
        if keys.len() < MAX_CANDIDATES && seen.insert(key.to_string()) {
            keys.push(key.to_string());
        }
    };
    let mut visits = 0;
    let mut container = false;
    for json in fields.iter().filter_map(|field| field.json.as_ref()) {
        let found = match parent {
            Some(parent) => parent.select(json),
            None => vec![json],
        };
        for value in found {
            if descend {
                keys_at_any_depth(value, &mut visits, &mut |key| add(key, &mut keys));
                container |= matches!(value, Value::Object(_) | Value::Array(_));
                continue;
            }
            match value {
                Value::Object(object) => {
                    container |= !object.is_empty();
                    object.keys().for_each(|key| add(key, &mut keys));
                }
                Value::Array(items) => {
                    container |= !items.is_empty();
                    (0..items.len().min(MAX_INDEXES)).for_each(|i| add(&i.to_string(), &mut keys));
                }
                _ => {}
            }
        }
    }
    if container && !descend {
        keys.retain(|key| key != "*");
        keys.insert(0, "*".to_string());
    }
    keys
}

fn keys_at_any_depth(value: &Value, visits: &mut usize, add: &mut impl FnMut(&str)) {
    *visits += 1;
    if *visits > MAX_VISITS {
        return;
    }
    match value {
        Value::Object(object) => {
            for (key, child) in object {
                add(key);
                keys_at_any_depth(child, visits, add);
            }
        }
        Value::Array(items) => items.iter().for_each(|child| keys_at_any_depth(child, visits, add)),
        _ => {}
    }
}

/// The candidates that hold `typed` (whatever the case), those that start with
/// it first, each with where it matched.
fn narrow(candidates: &[String], typed: &str) -> Vec<Item> {
    let mut starting = Vec::new();
    let mut containing = Vec::new();
    for key in candidates {
        match find_ignoring_case(key, typed) {
            Some(matched) if matched.0 == 0 => starting.push(Item { key: key.clone(), matched }),
            Some(matched) => containing.push(Item { key: key.clone(), matched }),
            None => {}
        }
    }
    starting.extend(containing);
    starting.truncate(MAX_SHOWN);
    starting
}

/// Where `needle` first occurs in `haystack` (the whole of it, or nothing, when
/// `needle` is empty), as a byte range of `haystack`.
fn find_ignoring_case(haystack: &str, needle: &str) -> Option<(usize, usize)> {
    if needle.is_empty() {
        return Some((0, 0));
    }
    let needle_lower = needle.to_lowercase();
    let length = needle.chars().count();
    haystack.char_indices().map(|(i, _)| i).find_map(|start| {
        let end = haystack[start..].char_indices().nth(length).map_or(haystack.len(), |(n, _)| start + n);
        (haystack[start..end].to_lowercase() == needle_lower).then_some((start, end))
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::super::test_support::field;
    use super::*;

    fn fields() -> Vec<Arc<Field>> {
        vec![
            field("one", Some(json!({"records": [{"label": "a", "id": 1}, {"label": "b", "extra": true}], "name": "n", "a.b": 1}))),
            field("two", Some(json!({"records": [{"label": "c"}], "Nested": {"deep": {"label": 1}}}))),
            field("text", None),
        ]
    }

    fn keys(text: &str) -> Vec<String> {
        let at = text.len();
        Suggester::default().complete(&fields(), text, at).map(|c| c.items.into_iter().map(|i| i.key).collect()).unwrap_or_default()
    }

    #[test]
    fn a_dot_lists_what_the_values_have_at_that_path() {
        assert_eq!(keys("records."), ["*", "0", "1"]);
        assert_eq!(keys("records.*."), ["*", "label", "id", "extra"]);
        assert_eq!(keys("records[0]."), ["*", "label", "id"]);
        assert_eq!(keys("$."), ["*", "records", "name", "a.b", "Nested"]);
        assert_eq!(keys("records.0.label."), Vec::<String>::new(), "a string has nothing under it");
        assert_eq!(keys("nothing."), Vec::<String>::new());
        assert_eq!(keys("records[."), Vec::<String>::new(), "no completion inside a bracket");
    }

    #[test]
    fn the_first_segment_completes_once_something_is_typed() {
        assert_eq!(keys(""), Vec::<String>::new());
        assert_eq!(keys("re"), ["records"]);
        assert_eq!(keys("$re"), Vec::<String>::new(), "`$` needs a separator");
    }

    #[test]
    fn what_is_typed_after_the_dot_narrows_the_list_and_ranks_prefixes_first() {
        assert_eq!(keys("records.*.l"), ["label"]);
        // contains, not just starts with; those that start with it first
        assert_eq!(keys("$.a"), ["a.b", "name"]);
        assert_eq!(keys("$.NAM"), ["name"]);
        let completion = Suggester::default().complete(&fields(), "$.ecor", 6).unwrap();
        assert_eq!(completion.items, vec![Item { key: "records".into(), matched: (1, 5) }]);
        assert_eq!(keys("records.*.label"), Vec::<String>::new(), "already what is typed");
        assert_eq!(keys("records.*.zzz"), Vec::<String>::new());
    }

    #[test]
    fn two_dots_list_the_keys_at_any_depth() {
        assert_eq!(keys(".."), ["records", "label", "id", "extra", "name", "a.b", "Nested", "deep"]);
        assert_eq!(keys("Nested.."), ["deep", "label"]);
        assert_eq!(keys("..la"), ["label"]);
    }

    #[test]
    fn accepting_replaces_the_word_and_quotes_what_cannot_be_typed() {
        let suggester = Suggester::default();
        let accept = |text: &str, cursor: usize, key: &str| {
            let completion = suggester.complete(&fields(), text, cursor).unwrap();
            let index = completion.items.iter().position(|i| i.key == key).unwrap();
            completion.accept(text, index).unwrap()
        };
        assert_eq!(accept("re", 2, "records"), Edit { text: "records".into(), cursor: 7 });
        assert_eq!(accept("records.*.l", 11, "label"), Edit { text: "records.*.label".into(), cursor: 15 });
        // the rest of the word being edited goes too, the rest of the path stays
        assert_eq!(accept("rec.x.label", 2, "records"), Edit { text: "records.x.label".into(), cursor: 7 });
        assert_eq!(accept("records.", 8, "*"), Edit { text: "records.*".into(), cursor: 9 });
        // a key holding a dot takes the bracket form, in place of the dot
        assert_eq!(accept("$.", 2, "a.b"), Edit { text: "$[\"a.b\"]".into(), cursor: 8 });
        assert_eq!(accept("..", 2, "a.b").text, "..[\"a.b\"]");
        // and the result is a path that parses
        assert!(json_path::parse("$[\"a.b\"]").is_ok() && json_path::parse("..[\"a.b\"]").is_ok());
    }

    #[test]
    fn what_was_found_is_kept_until_the_fields_change() {
        let suggester = Suggester::default();
        let one = fields();
        assert!(suggester.complete(&one, "records.", 8).is_some());
        assert!(suggester.complete(&[], "records.", 8).is_some(), "kept, so not read again");
        suggester.forget();
        assert!(suggester.complete(&[], "records.", 8).is_none());
    }

    #[test]
    fn a_match_is_found_whatever_the_case_and_reported_in_bytes() {
        assert_eq!(find_ignoring_case("Élan", "ÉL"), Some((0, 3)));
        assert_eq!(find_ignoring_case("xélan", "LA"), Some((3, 5)));
        assert_eq!(find_ignoring_case("abc", ""), Some((0, 0)));
        assert_eq!(find_ignoring_case("abc", "d"), None);
    }
}
