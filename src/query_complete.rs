//! Completion for the query box: keywords, key names and folders (one level of the keyspace at a
//! time, from SCAN), and the fields of an alias's key after `alias.`. It works out the token under
//! the caret, what could replace it, and how a chosen item is written into the text.

use rustc_hash::FxHashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::backend::explorer::scan_namespaces;
use crate::backend::key_name::KeyName;
use crate::backend::redisql::field_names;
use crate::backend::state::AppState;
use crate::backend::LockExt;

const KEYWORDS: [&str; 11] = ["FROM", "JOIN", "KEY", "AS", "ON", "WHERE", "SELECT", "LIMIT", "AND", "OR", "IN"];
/// Most items offered at once.
const MAX_ITEMS: usize = 50;
/// How long a listing of one keyspace level may take, and how many names it keeps.
const SCAN_BUDGET: Duration = Duration::from_millis(1500);
const SCAN_NAMES: usize = 300;
/// How long a listing is reused.
const CACHE_TTL: Duration = Duration::from_secs(30);

/// What is offered for the token at the caret, and the text it would replace.
#[derive(Clone)]
pub struct Completion {
    pub start: usize,
    pub end: usize,
    /// the quote the token sits in, if any
    pub quote: Option<char>,
    /// the items, each with the part of it that matches what was typed
    pub items: Vec<(String, std::ops::Range<usize>)>,
}

/// Names listed by (connection, key or prefix), with when they were listed.
type Listed = Mutex<FxHashMap<(String, String), (Instant, Vec<String>)>>;

/// What was listed lately, by connection and key or prefix.
#[derive(Default)]
pub struct Caches {
    levels: Listed,
    fields: Listed,
}

fn is_token_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | ':' | '.')
}

/// The quote the caret is inside of: quotes before it on its line open and close in turn.
fn open_quote(text: &str, cursor: usize) -> Option<(char, usize)> {
    let line_start = text[..cursor].rfind('\n').map_or(0, |i| i + 1);
    let mut open = None;
    for (i, c) in text[line_start..cursor].char_indices() {
        match open {
            Some((quote, _)) if c == quote => open = None,
            None if c == '\'' || c == '"' => open = Some((c, line_start + i)),
            _ => {}
        }
    }
    open
}

/// The `[start, end)` of the token at `cursor`: the whole quoted key inside quotes (up to its
/// closing quote), otherwise the run of letters, digits, `_`, `:` and `.` around it.
fn token_bounds(text: &str, cursor: usize) -> (usize, usize, Option<char>) {
    if let Some((quote, at)) = open_quote(text, cursor) {
        let end = text[cursor..].find([quote, '\n']).map_or(text.len(), |i| cursor + i);
        return (at + 1, end, Some(quote));
    }
    let start = text[..cursor].char_indices().rev().take_while(|(_, c)| is_token_char(*c)).last().map_or(cursor, |(i, _)| i);
    let end = text[cursor..].find(|c: char| !is_token_char(c)).map_or(text.len(), |i| cursor + i);
    (start, end, None)
}

/// The key each alias of the query names: `KEY 'key' AS alias`, or `KEY 'key' alias`.
fn alias_keys(text: &str) -> FxHashMap<String, String> {
    const NOT_ALIASES: [&str; 11] = ["from", "join", "key", "as", "on", "where", "select", "limit", "and", "or", "in"];
    let lower = text.to_lowercase();
    let mut aliases = FxHashMap::default();
    let mut from = 0;
    while let Some(found) = lower[from..].find("key") {
        let after = from + found + 3;
        from = after;
        let rest = text[after..].trim_start();
        let Some(quote) = rest.chars().next().filter(|c| *c == '\'' || *c == '"') else { continue };
        let Some(close) = rest[1..].find(quote) else { continue };
        let key = &rest[1..1 + close];
        let mut words = rest[close + 2..].split_whitespace();
        let mut word = words.next().unwrap_or_default();
        if word.eq_ignore_ascii_case("as") {
            word = words.next().unwrap_or_default();
        }
        let alias: String = word.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_').collect();
        if !alias.is_empty() && !NOT_ALIASES.contains(&alias.to_lowercase().as_str()) {
            aliases.insert(alias.to_lowercase(), key.to_string());
        }
    }
    aliases
}

fn cached(map: &Listed, key: &(String, String)) -> Option<Vec<String>> {
    map.lock_recover().get(key).filter(|(at, _)| at.elapsed() < CACHE_TTL).map(|(_, names)| names.clone())
}

/// The folders (ending in `:`) and keys one level under `prefix`, listed for a moment at most.
async fn level_names(state: &Arc<AppState>, caches: &Caches, connection: &str, prefix: &str) -> Vec<String> {
    let cache_key = (connection.to_string(), prefix.to_string());
    if let Some(names) = cached(&caches.levels, &cache_key) {
        return names;
    }
    let names = Arc::new(Mutex::new(Vec::<String>::new()));
    let enough = Arc::new(AtomicBool::new(false));
    let deadline = Instant::now() + SCAN_BUDGET;
    let (collected, stop) = (names.clone(), enough.clone());
    // the prefix is what's typed in the query, and so are the names offered
    let _ = scan_namespaces(
        state,
        None,
        &KeyName::from(prefix),
        ":",
        {
            let enough = enough.clone();
            move || !enough.load(Ordering::Relaxed) && Instant::now() < deadline
        },
        move |batch| {
            let mut names = collected.lock_recover();
            names.extend(batch.folders.iter().map(KeyName::to_string_lossy));
            names.extend(batch.keys.iter().map(|key| key.name.to_string_lossy()));
            if names.len() >= SCAN_NAMES {
                stop.store(true, Ordering::Relaxed);
            }
        },
    )
    .await;
    let mut names = std::mem::take(&mut *names.lock_recover());
    names.sort();
    names.dedup();
    caches.levels.lock_recover().insert(cache_key, (Instant::now(), names.clone()));
    names
}

async fn field_candidates(state: &Arc<AppState>, caches: &Caches, connection: &str, key: &str) -> Vec<String> {
    let cache_key = (connection.to_string(), key.to_string());
    if let Some(names) = cached(&caches.fields, &cache_key) {
        return names;
    }
    let names = field_names(state, None, key).await.unwrap_or_default();
    caches.fields.lock_recover().insert(cache_key, (Instant::now(), names.clone()));
    names
}

/// What could replace the token at `cursor` in `text`, or `None` for nothing to offer. Fewer
/// than two characters are only completed when `force` (Ctrl+Space).
pub async fn complete(state: &Arc<AppState>, caches: &Caches, text: &str, cursor: usize, force: bool) -> Option<Completion> {
    if cursor > text.len() || !text.is_char_boundary(cursor) {
        return None;
    }
    let (start, end, quote) = token_bounds(text, cursor);
    let token = &text[start..cursor];
    let lower = token.to_lowercase();
    let connection = state.connections.lock_recover().active_connection_id().to_string();
    if connection.is_empty() {
        return None;
    }

    let mut items: Vec<(String, std::ops::Range<usize>)> = Vec::new();
    let alias_field = quote.is_none().then(|| token.split_once('.')).flatten().filter(|(alias, field)| {
        alias.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_') && alias.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') && !field.contains(['.', ':'])
    });
    if let Some((alias, field_prefix)) = alias_field {
        let key = alias_keys(text).get(&alias.to_lowercase())?.clone();
        let wanted = field_prefix.to_lowercase();
        for field in field_candidates(state, caches, &connection, &key).await {
            if let Some(at) = field.to_lowercase().find(&wanted) {
                let shown = format!("{alias}.{field}");
                let from = alias.len() + 1 + at;
                items.push((shown, from..from + wanted.len()));
            }
        }
        items.sort_by_key(|(shown, hit)| (hit.start, shown.len()));
    } else {
        if !force && lower.chars().count() < 2 {
            return None;
        }
        if quote.is_none() {
            items.extend(KEYWORDS.iter().filter(|k| k.to_lowercase().starts_with(&lower)).map(|k| (k.to_string(), 0..lower.len())));
        }
        let parent = token.rfind(':').map_or("", |i| &token[..=i]);
        let mut names = level_names(state, caches, &connection, parent).await;
        names.retain(|name| name.to_lowercase().starts_with(&lower) && !(token.ends_with(':') && name.to_lowercase() == lower));
        // folders before keys, each in order
        names.sort_by_key(|name| (!name.ends_with(':'), name.clone()));
        items.extend(names.into_iter().map(|name| (name, 0..lower.len())));
    }
    items.truncate(MAX_ITEMS);
    (!items.is_empty()).then_some(Completion { start, end, quote, items })
}

impl Completion {
    /// `text` with item `index` written in place of the token, and where the caret goes.
    pub fn accept(&self, text: &str, index: usize) -> Option<(String, usize)> {
        let (value, _) = self.items.get(index)?;
        let is_key = value.contains(':');
        let replacement = match self.quote {
            // a key inside quotes: closed, unless it is a folder (to be continued) or already closed
            Some(quote) if is_key && !value.ends_with(':') && !text[self.end..].starts_with(quote) => format!("{value}{quote}"),
            Some(_) => value.clone(),
            // a key written out of quotes gets them; a folder leaves its quote open for what follows
            None if is_key && value.ends_with(':') => format!("'{value}"),
            None if is_key => format!("'{value}'"),
            None => value.clone(),
        };
        let mut next = String::with_capacity(text.len() + replacement.len());
        next.push_str(&text[..self.start]);
        next.push_str(&replacement);
        next.push_str(&text[self.end..]);
        Some((next, self.start + replacement.len()))
    }
}
