//! The key header's smart box: Enter makes its text one badge, spaces included (see
//! `filtering`); a text starting with `$` or `.` is a JSON path filtering the values.
//! Its list offers the path's keys, or for words where to look.

use rust_i18n::t;
use slint::{ComponentHandle, Model, ModelRc, VecModel};

use super::Ctx;
use crate::{BadgeData, CompletionEdit, KeyPanel, SearchScope, SuggestionRow};

/// What one item of the list does with the words typed.
enum Choice {
    Search(SearchScope, String),
    Filter(String),
}

/// Whether `text` is a JSON path rather than words to search for.
fn is_path(text: &str) -> bool {
    matches!(text.trim_start().chars().next(), Some('$' | '.'))
}

/// Whether `text` is words to search for: not empty and not a path.
pub(super) fn is_words(text: &str) -> bool {
    !text.trim().is_empty() && !is_path(text)
}

/// The scope a `field:` or `value:` (or `f:`, `v:`) before the words names, whatever its
/// case, and the words.
fn scoped(text: &str) -> (Option<SearchScope>, &str) {
    let text = text.trim_start();
    let prefixes = [
        ("fields:", SearchScope::Fields),
        ("field:", SearchScope::Fields),
        ("values:", SearchScope::Value),
        ("value:", SearchScope::Value),
        ("f:", SearchScope::Fields),
        ("v:", SearchScope::Value),
    ];
    for (prefix, scope) in prefixes {
        if text.get(..prefix.len()).is_some_and(|head| head.eq_ignore_ascii_case(prefix)) {
            return (Some(scope), text[prefix.len()..].trim_start());
        }
    }
    (None, text)
}

/// What is typed before words to search for them in `scope` alone ("" for everywhere).
fn short_prefix(scope: SearchScope) -> &'static str {
    match scope {
        SearchScope::Fields => "f:",
        SearchScope::Value => "v:",
        _ => "",
    }
}

/// The list for words: where to search for them (only the scope a prefix named), then, for
/// a single word, filtering the JSON at it. With no word yet, what can be typed first.
fn choices(text: &str) -> Vec<Choice> {
    let (named, words) = scoped(text);
    let words = words.trim();
    if words.is_empty() {
        return match named {
            Some(_) => Vec::new(),
            None => vec![
                Choice::Search(SearchScope::Fields, String::new()),
                Choice::Search(SearchScope::Value, String::new()),
                Choice::Filter("$.".to_string()),
            ],
        };
    }
    let scopes = match named {
        Some(scope) => vec![scope],
        None => vec![SearchScope::Everywhere, SearchScope::Fields, SearchScope::Value],
    };
    let mut choices: Vec<Choice> = scopes.into_iter().map(|scope| Choice::Search(scope, words.to_string())).collect();
    if named.is_none() && !words.contains(|c: char| c.is_whitespace() || c == '"') {
        choices.push(Choice::Filter(format!("$.{words}")));
    }
    choices
}

/// How `choice` reads in the list: "Search", the scope as a pill in its color, the words;
/// or "Apply a", a Filter pill, the path. Its hint is what picks it from the box: Enter for the
/// `first`, which Enter alone takes, else the prefix to type.
fn row(choice: &Choice, first: bool) -> SuggestionRow {
    let hint = |typed: &str| if first { t!("Enter").into_owned() } else { typed.to_string() };
    match choice {
        Choice::Search(scope, words) => {
            let (tag, prefix) = match scope {
                SearchScope::Fields => (t!("Fields"), "f: / field:"),
                SearchScope::Value => (t!("Value"), "v: / value:"),
                _ => (t!("All"), ""),
            };
            SuggestionRow {
                icon: "search".into(),
                before: format!("{} ", t!("Search (smart box)")).into(),
                tag: tag.as_ref().into(),
                scope: *scope,
                matched: words.as_str().into(),
                hint: hint(prefix).into(),
                ..Default::default()
            }
        }
        Choice::Filter(path) => SuggestionRow {
            icon: "filter".into(),
            before: format!("{} ", t!("Apply a")).into(),
            tag: t!("Filter").as_ref().into(),
            matched: path.as_str().into(),
            hint: hint("$.").into(),
            ..Default::default()
        },
    }
}

impl Ctx {
    /// The smart box's list for `text`: the keys that can follow the path, or where to
    /// search for the words; whether it has any.
    pub(super) fn suggest(&self, text: &str, cursor: i32) -> bool {
        if is_path(text) {
            return self.suggest_path(text, cursor);
        }
        let Some(window) = self.weak.upgrade() else { return false };
        // with no word yet there is nothing for Enter to take: every row says what to type
        let typed = !scoped(text).1.trim().is_empty();
        let rows: Vec<SuggestionRow> = choices(text).iter().enumerate().map(|(i, choice)| row(choice, typed && i == 0)).collect();
        let any = !rows.is_empty();
        window.global::<KeyPanel>().set_suggestions(ModelRc::new(VecModel::from(rows)));
        let footer = t!("Enter adds the search; all must match. Click one or press ← to edit it, Backspace removes the last one.");
        window.global::<KeyPanel>().set_suggestion_footer(footer.as_ref().into());
        any
    }

    /// Item `index` of the list taken: a key of the path, or with no word yet a prefix, goes into
    /// the box (its text, and where the caret goes); else the search or filter offered is
    /// applied (cursor -1).
    pub(super) fn complete_suggestion(&self, text: slint::SharedString, cursor: i32, index: i32) -> CompletionEdit {
        if is_path(&text) {
            return self.shared.complete_path(text, cursor, index);
        }
        match choices(&text).into_iter().nth(index.max(0) as usize) {
            Some(Choice::Search(scope, words)) if words.is_empty() => {
                let prefix = short_prefix(scope);
                return CompletionEdit { text: prefix.into(), cursor: prefix.len() as i32 };
            }
            Some(Choice::Filter(path)) if scoped(&text).1.trim().is_empty() => {
                return CompletionEdit { cursor: path.len() as i32, text: path.into() };
            }
            Some(Choice::Search(scope, words)) => self.add_badge(scope, &words),
            Some(Choice::Filter(path)) => self.apply_path(&path),
            None => {}
        }
        CompletionEdit { text, cursor: -1 }
    }

    /// Enter in the smart box: its path becomes the JSON filter, or its words one badge,
    /// searched everywhere unless a `field:` or `value:` prefix (or `f:`, `v:`) says otherwise.
    pub(super) fn on_filter_query_submitted(&self) {
        let Some(window) = self.weak.upgrade() else { return };
        let text = window.global::<KeyPanel>().get_filter_query();
        if is_path(&text) {
            self.apply_path(text.trim());
        } else {
            let (named, words) = scoped(&text);
            self.add_badge(named.unwrap_or(SearchScope::Everywhere), words);
        }
    }

    /// A badge's text clicked (or ← at the box's start): what is typed becomes a search first,
    /// then the badge goes back in the box with its scope's prefix; where the box's text ends,
    /// or -1 when a path typed doesn't parse (it stays in the box, the badge stays too).
    pub(super) fn on_filter_badge_edited(&self, text: &str, scope: SearchScope) -> i32 {
        let Some(window) = self.weak.upgrade() else { return -1 };
        let panel = window.global::<KeyPanel>();
        let typed = panel.get_filter_query();
        if is_path(&typed) {
            self.apply_path(typed.trim());
            if !panel.get_filter_query().is_empty() {
                return -1;
            }
        }
        let mut badges: Vec<BadgeData> =
            panel.get_filter_badges().iter().filter(|b| !(b.scope == scope && b.text.as_str() == text)).collect();
        let (named, words) = scoped(&typed);
        let (words, named) = (words.trim(), named.unwrap_or(SearchScope::Everywhere));
        if !is_path(&typed) && !words.is_empty() && !badges.iter().any(|b| b.scope == named && b.text.as_str() == words) {
            badges.push(BadgeData { text: words.into(), scope: named });
        }
        panel.set_filter_badges(ModelRc::new(VecModel::from(badges)));
        panel.set_filter_limit_message("".into());
        let query = format!("{}{text}", short_prefix(scope));
        panel.set_filter_query(query.as_str().into());
        self.on_search_changed();
        query.len() as i32
    }

    /// Filters the values with `path`; the box empties once it parses (it stays to be fixed).
    fn apply_path(&self, path: &str) {
        let Some(window) = self.weak.upgrade() else { return };
        window.global::<KeyPanel>().set_json_filter(path.into());
        self.on_json_filter_submitted();
        if window.global::<KeyPanel>().get_json_filter_error().is_empty() {
            window.global::<KeyPanel>().set_filter_query("".into());
        }
    }
}
