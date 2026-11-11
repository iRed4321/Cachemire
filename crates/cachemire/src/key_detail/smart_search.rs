//! The key header's smart box: its words become badges (see `filtering`), a space leaving
//! them waiting until Enter applies them all; a text starting with `$` or `.` is a JSON path
//! filtering the values. Its list offers the path's keys, or for words where to look.

use rust_i18n::t;
use slint::{ComponentHandle, ModelRc, VecModel};

use super::Ctx;
use crate::{CompletionEdit, KeyPanel, SearchScope, SuggestionRow};

/// What one item of the list does with the words typed.
enum Choice {
    Search(SearchScope, String),
    Filter(String),
}

/// Whether `text` is a JSON path rather than words to search for.
fn is_path(text: &str) -> bool {
    matches!(text.trim_start().chars().next(), Some('$' | '.'))
}

/// The scope a `field:` or `value:` before the words names (whatever its case), and the words.
fn scoped(text: &str) -> (Option<SearchScope>, &str) {
    let text = text.trim_start();
    let prefixes = [
        ("fields:", SearchScope::Fields),
        ("field:", SearchScope::Fields),
        ("values:", SearchScope::Value),
        ("value:", SearchScope::Value),
    ];
    for (prefix, scope) in prefixes {
        if text.get(..prefix.len()).is_some_and(|head| head.eq_ignore_ascii_case(prefix)) {
            return (Some(scope), text[prefix.len()..].trim_start());
        }
    }
    (None, text)
}

/// The list for words: where to search for them (only the scope a prefix named), then, for
/// a single word, filtering the JSON at it.
fn choices(text: &str) -> Vec<Choice> {
    let (named, words) = scoped(text);
    let words = words.trim();
    if words.is_empty() {
        return Vec::new();
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
                SearchScope::Fields => (t!("Fields"), "field:"),
                SearchScope::Value => (t!("Value"), "value:"),
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
        let rows: Vec<SuggestionRow> = choices(text).iter().enumerate().map(|(i, choice)| row(choice, i == 0)).collect();
        let any = !rows.is_empty();
        window.global::<KeyPanel>().set_suggestions(ModelRc::new(VecModel::from(rows)));
        let footer = t!("Space adds a search, Enter applies them; all must match. Click one to edit it, Backspace removes the last one.");
        window.global::<KeyPanel>().set_suggestion_footer(footer.as_ref().into());
        any
    }

    /// Item `index` of the list taken: a key of the path goes into the box (its text, and
    /// where the caret goes), or the search or filter offered is applied (cursor -1).
    pub(super) fn complete_suggestion(&self, text: slint::SharedString, cursor: i32, index: i32) -> CompletionEdit {
        if is_path(&text) {
            return self.shared.complete_path(text, cursor, index);
        }
        match choices(&text).into_iter().nth(index.max(0) as usize) {
            Some(Choice::Search(scope, words)) => self.add_badges(scope, &words, true),
            Some(Choice::Filter(path)) => self.apply_path(&path),
            None => {}
        }
        CompletionEdit { text, cursor: -1 }
    }

    /// Enter in the smart box: its path becomes the JSON filter, or its words become
    /// badges, searched everywhere unless a `field:` or `value:` prefix says otherwise;
    /// either way the badges waiting are applied.
    pub(super) fn on_filter_query_submitted(&self) {
        let Some(window) = self.weak.upgrade() else { return };
        let text = window.global::<KeyPanel>().get_filter_query();
        if is_path(&text) {
            self.apply_path(text.trim());
        } else {
            let (named, words) = scoped(&text);
            self.add_badges(named.unwrap_or(SearchScope::Everywhere), words, true);
        }
    }

    /// The box's text edited: a space typed at its end, past words and outside an open
    /// quote, makes them badges, waiting for Enter to apply them; whether it did.
    pub(super) fn on_filter_query_edited(&self, text: &str, cursor: i32) -> bool {
        let Some(window) = self.weak.upgrade() else { return false };
        let at_end = usize::try_from(cursor).is_ok_and(|cursor| cursor == text.len());
        if is_path(text) || !at_end || !text.ends_with(' ') || text.matches('"').count() % 2 == 1 {
            return false;
        }
        let (named, words) = scoped(text);
        if words.trim().is_empty() {
            return false;
        }
        self.add_badges(named.unwrap_or(SearchScope::Everywhere), words, false);
        window.global::<KeyPanel>().get_filter_query().is_empty()
    }

    /// A badge's text clicked: it is dropped and its text goes at the end of the box, quoted
    /// when it holds a space so it stays one term; where the box's text then ends.
    pub(super) fn on_filter_badge_edited(&self, text: &str, scope: SearchScope) -> i32 {
        let Some(window) = self.weak.upgrade() else { return 0 };
        self.on_filter_badge_removed(text, scope);
        let term = if text.contains(char::is_whitespace) { format!("\"{text}\"") } else { text.to_string() };
        let typed = window.global::<KeyPanel>().get_filter_query();
        let query = if typed.trim().is_empty() { term } else { format!("{} {term}", typed.trim_end()) };
        window.global::<KeyPanel>().set_filter_query(query.as_str().into());
        query.len() as i32
    }

    /// Filters the values with `path`, the badges waiting applied with it; the box empties
    /// once it parses (it stays to be fixed).
    fn apply_path(&self, path: &str) {
        let Some(window) = self.weak.upgrade() else { return };
        window.global::<KeyPanel>().set_json_filter(path.into());
        self.on_json_filter_submitted();
        if window.global::<KeyPanel>().get_json_filter_error().is_empty() {
            window.global::<KeyPanel>().set_filter_query("".into());
            self.add_badges(SearchScope::Everywhere, "", true);
        }
    }
}
