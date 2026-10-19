//! Search tabs: a tab that searches the whole instance for a word. Each tab keeps
//! its box text, hits and status in [`SearchTabs`], shown through the window's
//! `search-*` properties while active; the search itself runs on Tokio.

use crate::error::Localize;
use rustc_hash::FxHashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use rayon::prelude::*;
use rust_i18n::t;
use slint::{ModelRc, VecModel};
use tokio::sync::watch;

use crate::app::App;
use crate::key_detail::{search_plain_text, search_value_text};
use crate::{MainWindow, SearchRowData, SearchTab};
use cachemire_core::explorer::{self, EntriesBatch, KeyEntries};
use cachemire_core::key_name::KeyName;
use cachemire_core::redis_value_type::RedisValueType;
use slint::ComponentHandle;

/// A search tab's id in `tabs/mod.rs`: it can't be a key's name, whatever the key is
/// called, because of the control character.
const ID_PREFIX: &str = "\u{1}search:";

/// Hits kept per search; the scan stops there.
const MAX_HITS: usize = 1000;

/// The least time between two reports that carry no hit, only the count of keys
/// walked.
const PROGRESS_INTERVAL: Duration = Duration::from_millis(60);

// The key and field columns fit their longest cell, within these bounds (in chars),
// each char being this many zoom-independent px, plus the cell's padding.
const CHAR_PX: f32 = 7.4;
const CELL_PAD_PX: f32 = 20.0;
const KEY_CHARS: std::ops::RangeInclusive<usize> = 5..=48;
const FIELD_CHARS: std::ops::RangeInclusive<usize> = 5..=28;

/// The longest key and field of the hits so far, in chars.
#[derive(Clone, Copy, Default)]
struct Longest {
    key: usize,
    field: usize,
}

impl Longest {
    /// (key column, field column) widths in zoom-independent px.
    fn widths(self) -> (f32, f32) {
        let width = |chars: usize, bounds: std::ops::RangeInclusive<usize>| {
            chars.clamp(*bounds.start(), *bounds.end()) as f32 * CHAR_PX + CELL_PAD_PX
        };
        (width(self.key, KEY_CHARS), width(self.field, FIELD_CHARS))
    }
}

pub fn is_search_id(id: &str) -> bool {
    id.starts_with(ID_PREFIX)
}

struct TabSearch {
    /// the box's text
    query: String,
    searched: bool,
    loading: bool,
    status: String,
    /// how far the search is, 0 to 1 — only ever grows during a search; -1 when
    /// the size of the instance isn't known (the bar then just shows activity)
    progress: f32,
    rows: Rc<VecModel<SearchRowData>>,
    longest: Longest,
    /// bumped to drop what a running search still sends (a newer search, a closed tab)
    run: Arc<AtomicU64>,
    /// the running search is paused: it asks Redis for nothing more until this clears
    paused: watch::Sender<bool>,
}

impl TabSearch {
    fn new() -> Self {
        Self {
            query: String::new(),
            searched: false,
            loading: false,
            status: String::new(),
            progress: 0.0,
            rows: Rc::new(VecModel::default()),
            longest: Longest::default(),
            run: Arc::new(AtomicU64::new(0)),
            paused: watch::Sender::new(false),
        }
    }
}

/// Every search tab's state, and which one is shown.
#[derive(Default)]
pub struct SearchTabs {
    tabs: FxHashMap<String, TabSearch>,
    last_id: u64,
    active: Option<String>,
}

impl SearchTabs {
    /// A new, empty search tab; returns its id.
    pub fn create(&mut self) -> String {
        self.last_id += 1;
        let id = format!("{ID_PREFIX}{}", self.last_id);
        self.tabs.insert(id.clone(), TabSearch::new());
        id
    }

    /// Shows search tab `id` in the main area: its box, hits and status.
    pub fn enter(&mut self, window: &MainWindow, id: &str) {
        self.active = Some(id.to_string());
        let Some(tab) = self.tabs.get(id) else { return };
        window.global::<SearchTab>().set_query(tab.query.as_str().into());
        window.global::<SearchTab>().set_searched(tab.searched);
        window.global::<SearchTab>().set_loading(tab.loading);
        window.global::<SearchTab>().set_status(tab.status.as_str().into());
        window.global::<SearchTab>().set_progress(tab.progress);
        window.global::<SearchTab>().set_paused(*tab.paused.borrow());
        window.global::<SearchTab>().set_rows(ModelRc::from(tab.rows.clone()));
        show_widths(window, tab.longest);
    }

    /// The tab is being left: keeps what was typed in its box, and pauses its search
    /// if it is still running — whatever is opened next (a key, from a hit, say) gets
    /// Redis to itself, and the search is resumed from the tab when wanted.
    pub fn leave(&mut self, window: &MainWindow, id: &str) {
        if let Some(tab) = self.tabs.get_mut(id) {
            tab.query = window.global::<SearchTab>().get_query().to_string();
            if tab.loading {
                tab.paused.send_replace(true);
            }
        }
        self.drop_active(id);
    }

    /// The tab is closed: its search stops and its hits go.
    pub fn forget(&mut self, id: &str) {
        if let Some(tab) = self.tabs.remove(id) {
            tab.run.fetch_add(1, Ordering::Relaxed);
        }
        self.drop_active(id);
    }

    fn drop_active(&mut self, id: &str) {
        if self.active.as_deref() == Some(id) {
            self.active = None;
        }
    }

    /// The pause button of the shown search tab: pauses its running search, or resumes it.
    fn toggle_pause(&self, window: &MainWindow) {
        let Some(tab) = self.active.as_ref().and_then(|id| self.tabs.get(id)).filter(|tab| tab.loading) else { return };
        let paused = !*tab.paused.borrow();
        tab.paused.send_replace(paused);
        window.global::<SearchTab>().set_paused(paused);
    }
}

fn show_widths(window: &MainWindow, longest: Longest) {
    let (key, field) = longest.widths();
    window.global::<SearchTab>().set_key_auto_w(key);
    window.global::<SearchTab>().set_field_auto_w(field);
}

/// The row of the key panel that shows a hit: a hash's field is a row of its own; any
/// other type is shown as one `(value)` row holding all of it.
fn target_field(key_type: &RedisValueType, field: &str) -> String {
    match key_type {
        RedisValueType::Hash => field.to_string(),
        _ => t!("(value)").into_owned(),
    }
}

/// What one key holds that matches `needle` (lowercase): every matching entry,
/// or the key alone when only its name matches. Independent per key, so a
/// batch's keys can be scanned across cores.
fn hits_in_key(KeyEntries { key, key_type, entries }: KeyEntries, needle: &str) -> (Vec<SearchRowData>, Longest) {
    // a hit opens its key by id; it is matched and shown as the name reads
    let id = key.id();
    let key = KeyName::display_id(&id).into_owned();
    let mut rows = Vec::new();
    let mut longest = Longest::default();
    for (field, value) in &entries {
        let field_hit = !field.is_empty() && field.to_lowercase().contains(needle);
        let value_hit = value.to_lowercase().contains(needle);
        if field_hit || value_hit {
            longest.key = longest.key.max(key.chars().count());
            longest.field = longest.field.max(field.chars().count());
            rows.push(SearchRowData {
                key: search_plain_text(&key, needle),
                field: search_plain_text(field, needle),
                value: search_value_text(value, needle),
                key_name: id.as_str().into(),
                target_field: target_field(&key_type, field).into(),
            });
        }
    }
    if rows.is_empty() && key.to_lowercase().contains(needle) {
        longest.key = longest.key.max(key.chars().count());
        rows.push(SearchRowData {
            key: search_plain_text(&key, needle),
            field: search_plain_text("", needle),
            value: search_value_text("", needle),
            key_name: id.as_str().into(),
            target_field: "".into(),
        });
    }
    (rows, longest)
}

/// What a page of keys holds that matches `needle` (lowercase): a row per matching
/// entry, or one for the key alone when only its name matches. Each key's scan
/// (`hits_in_key`) is independent, so a batch is spread across cores.
fn hits_in(
    batch_keys: Vec<KeyEntries>,
    needle: &str,
    room: usize,
    keys_with_hits: &mut usize,
    longest: &mut Longest,
) -> Vec<SearchRowData> {
    if room == 0 {
        return Vec::new();
    }
    let mut rows = Vec::new();
    for (key_rows, key_longest) in batch_keys.into_par_iter().map(|k| hits_in_key(k, needle)).collect::<Vec<_>>() {
        if !key_rows.is_empty() {
            *keys_with_hits += 1;
            longest.key = longest.key.max(key_longest.key);
            longest.field = longest.field.max(key_longest.field);
            rows.extend(key_rows);
        }
        if rows.len() >= room {
            break;
        }
    }
    rows.truncate(room);
    rows
}

fn status_text(query: &str, scanned: usize, total: usize, hits: usize, keys: usize, done: bool, capped: bool) -> String {
    match (done, hits) {
        (false, _) if total > 0 => t!(
            "searching... %{scanned} of %{total} keys scanned, %{hits} matches",
            scanned = scanned.min(total),
            total = total,
            hits = hits
        )
        .into_owned(),
        (false, _) => t!("searching... %{scanned} keys scanned, %{hits} matches", scanned = scanned, hits = hits).into_owned(),
        (true, 0) => t!("no match for “%{query}” in %{scanned} keys", query = query, scanned = scanned).into_owned(),
        (true, _) if capped => t!("first %{hits} matches, in %{keys} keys (search stopped)", hits = hits, keys = keys).into_owned(),
        (true, _) => t!("%{hits} matches in %{keys} keys, of %{scanned} scanned", hits = hits, keys = keys, scanned = scanned).into_owned(),
    }
}

/// How far a search is once it reports `reported`, given where its bar already
/// is: never back, whatever order or size the reports come in. A negative
/// report (the size of the instance is unknown) stays negative.
fn next_progress(shown: f32, reported: f32) -> f32 {
    if reported < 0.0 { -1.0 } else { shown.max(reported).min(1.0) }
}

/// How far along a walk of `total` keys is once `scanned` are seen: it stops just
/// short of the end until the walk is over, as the total is only what the instance
/// held at the start. -1 when `total` isn't known.
fn progress_of(scanned: usize, total: usize) -> f32 {
    if total == 0 {
        return -1.0;
    }
    (scanned as f32 / total as f32).clamp(0.0, 0.99)
}

/// What a search reports to the tab that started it, on the UI thread.
enum Report {
    Progress { rows: Vec<SearchRowData>, status: String, longest: Longest, progress: f32 },
    Finished { rows: Vec<SearchRowData>, status: String, longest: Longest },
}

fn report(id: &str, run: u64, report: Report) {
    let id = id.to_string();
    crate::app::later(move |app, window| {
        let (rows, status, longest, loading, progress) = match report {
            Report::Progress { rows, status, longest, progress } => (rows, status, longest, true, progress),
            Report::Finished { rows, status, longest } => (rows, status, longest, false, 1.0),
        };
        let mut searches = app.searches.borrow_mut();
        let is_active = searches.active.as_deref() == Some(id.as_str());
        let Some(tab) = searches.tabs.get_mut(&id) else { return };
        // a newer search (or a closed tab) has taken over
        if tab.run.load(Ordering::Relaxed) != run {
            return;
        }
        tab.rows.extend(rows);
        tab.status.clone_from(&status);
        tab.loading = loading;
        tab.longest = longest;
        tab.progress = next_progress(tab.progress, progress);
        if !loading {
            tab.paused.send_replace(false);
        }
        if is_active {
            window.global::<SearchTab>().set_paused(*tab.paused.borrow());
            show_widths(window, longest);
            window.global::<SearchTab>().set_status(status.into());
            window.global::<SearchTab>().set_loading(loading);
            window.global::<SearchTab>().set_progress(tab.progress);
        }
    });
}

/// Starts the search of `query` in the active search tab, replacing its hits.
fn submit(app: &App, window: &MainWindow, query: &str) {
    let query = query.trim().to_string();
    let connection = window.connection_id().to_string();

    let started = {
        let mut searches = app.searches.borrow_mut();
        let Some(id) = searches.active.clone() else { return };
        let Some(tab) = searches.tabs.get_mut(&id) else { return };
        let run = tab.run.fetch_add(1, Ordering::Relaxed) + 1;
        tab.query.clone_from(&query);
        tab.rows.set_vec(Vec::new());
        tab.longest = Longest::default();
        tab.progress = 0.0;
        window.global::<SearchTab>().set_progress(0.0);
        show_widths(window, tab.longest);
        tab.searched = !query.is_empty();
        tab.loading = tab.searched;
        tab.status = if tab.searched { t!("searching...").into_owned() } else { String::new() };
        window.global::<SearchTab>().set_searched(tab.searched);
        window.global::<SearchTab>().set_loading(tab.loading);
        window.global::<SearchTab>().set_status(tab.status.as_str().into());
        window.global::<SearchTab>().set_run(window.global::<SearchTab>().get_run().wrapping_add(1));
        // a new search starts running
        tab.paused.send_replace(false);
        window.global::<SearchTab>().set_paused(false);
        tab.searched.then(|| (id, tab.run.clone(), run, tab.paused.subscribe()))
    };
    let Some((id, run_flag, run, paused)) = started else { return };
    let (state, rt) = (&app.state, &app.rt);

    // experimental: Redis itself leaves out what certainly doesn't match
    let (server_search, depth) = {
        let settings = state.settings.lock();
        (settings.server_search(), settings.search_depth() as usize)
    };
    let (state, needle) = (state.clone(), query.to_lowercase());
    rt.spawn(async move {
        let mut found = 0usize;
        let mut keys_with_hits = 0usize;
        let mut longest = Longest::default();
        let enough = AtomicBool::new(false);
        let mut last_report = Instant::now();
        let result = explorer::scan_entries(
            &state,
            Some(&connection),
            depth,
            server_search.then_some(needle.as_str()),
            || run_flag.load(Ordering::Relaxed) == run && !enough.load(Ordering::Relaxed),
            paused,
            |EntriesBatch { keys, scanned, total, done }| {
                let rows = hits_in(keys, &needle, MAX_HITS - found, &mut keys_with_hits, &mut longest);
                found += rows.len();
                let capped = found >= MAX_HITS;
                if capped {
                    enough.store(true, Ordering::Relaxed);
                }
                let finished = done || capped;
                // hits go to the screen at once; a batch with none only moves the
                // counter, which needn't be redrawn more than a few times a second
                if !finished && rows.is_empty() && last_report.elapsed() < PROGRESS_INTERVAL {
                    return;
                }
                last_report = Instant::now();
                let status = status_text(&query, scanned, total, found, keys_with_hits, finished, capped);
                report(
                    &id,
                    run,
                    if finished {
                        Report::Finished { rows, status, longest }
                    } else {
                        Report::Progress { rows, status, longest, progress: progress_of(scanned, total) }
                    },
                );
            },
        )
        .await;
        if let Err(error) = result {
            report(
                &id,
                run,
                Report::Finished {
                    rows: Vec::new(),
                    status: t!("search failed: %{error}", error = error.localized()).into_owned(),
                    longest: Longest::default(),
                },
            );
        }
    });
}

/// Wires the search box of the search tabs.
pub fn init(app: &Rc<App>, window: &MainWindow) {
    window.global::<SearchTab>().on_pause_toggled({
        let app = app.clone();
        move || {
            if let Some(window) = app.window.upgrade() {
                app.searches.borrow().toggle_pause(&window);
            }
        }
    });
    window.global::<SearchTab>().on_submitted({
        let app = app.clone();
        move |query| {
            if let Some(window) = app.window.upgrade() {
                submit(&app, &window, &query);
            }
        }
    });
}

#[cfg(test)]
mod tests;
