//! Query tabs: a tab that runs a Redisql query (see `backend::redisql`) and shows
//! the result as a table. Each tab keeps its own query text and last result in
//! [`QueryTabs`]; the box and result live in the window's `query-*` properties while it is active.

use rustc_hash::FxHashMap;
use std::ops::Range;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use rust_i18n::t;
use serde_json::{Map, Value};
use slint::{ModelRc, VecModel};
use crate::app::App;
use crate::backend::error::AppError;
use crate::backend::history_store::HistoryEntry;
use crate::backend::{LockExt, now_ms};
use crate::backend::redisql::{QueryOutput, is_declaration, run_script, split_statements};
use crate::backend::state::AppState;
use crate::key_detail::ScrollPos;
use crate::query_complete::{Caches, Completion, complete};
use slint::ComponentHandle;
use crate::{CompletionEdit, HistoryRow, MainWindow, QueryTab, SuggestionRow};

/// A query tab's id in `tabs.rs`: never a key, for the same reason a search
/// tab's isn't (see `search::ID_PREFIX`) — a different control character, so
/// the two kinds of tab can't collide.
const ID_PREFIX: &str = "\u{2}query:";

pub fn is_query_id(id: &str) -> bool {
    id.starts_with(ID_PREFIX)
}

/// One result of a run: a script gives one per query it ran.
#[derive(Clone, Default)]
struct ResultTable {
    /// what the tab above the results says (empty while there is only one result)
    label: String,
    status: String,
    status_is_error: bool,
    /// the rows, as the key panel shows them
    rows: Vec<Map<String, Value>>,
}

struct TabQuery {
    /// the box's text
    text: String,
    /// a run has been attempted (before that, the table area shows a hint instead)
    ran: bool,
    loading: bool,
    /// what the run says while it is going, or about the result shown
    status: String,
    status_is_error: bool,
    /// the last run's results, and which is shown
    results: Vec<ResultTable>,
    active: usize,
    /// where in the box the last run's error is, and the text that was run
    error: Option<(Range<usize>, String)>,
    /// bumped to drop what a running query still sends (a newer run, a closed tab)
    run: Arc<AtomicU64>,
}

impl Default for TabQuery {
    fn default() -> Self {
        Self {
            text: String::new(),
            ran: false,
            loading: false,
            status: String::new(),
            status_is_error: false,
            results: Vec::new(),
            active: 0,
            error: None,
            run: Arc::new(AtomicU64::new(0)),
        }
    }
}

impl TabQuery {
    fn shown(&self) -> Option<&ResultTable> {
        self.results.get(self.active)
    }
}

/// Every query tab's state and which one is shown, with what the query box's
/// history and completion lists offer at the moment.
#[derive(Default)]
pub struct QueryTabs {
    tabs: FxHashMap<String, TabQuery>,
    last_id: u64,
    active: Option<String>,
    navigation: Navigation,
    /// the text of each row of the history popup as last shown
    history_shown: Vec<String>,
    /// what the completion list offers now, for accepting one of its rows
    offered: Option<Completion>,
    /// numbers the completion requests: an answer that isn't the latest is dropped
    suggest_generation: Arc<AtomicU64>,
}

impl QueryTabs {
    /// A new query tab holding `text` (empty, or a saved query opened from the
    /// sidebar, left un-run for the user to review first); returns its id.
    pub fn create(&mut self, text: String) -> String {
        self.last_id += 1;
        let id = format!("{ID_PREFIX}{}", self.last_id);
        self.tabs.insert(id.clone(), TabQuery { text, ..TabQuery::default() });
        id
    }

    /// The tab is being left: keeps what was typed in its box (its last result is
    /// already kept, from when the run that produced it landed).
    pub fn leave(&mut self, window: &MainWindow, id: &str) {
        if let Some(tab) = self.tabs.get_mut(id) {
            tab.text = window.global::<QueryTab>().get_text().to_string();
        }
        self.drop_active(id);
    }

    /// The tab is closed: its query stops (if still running) and its result goes.
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
}

/// Shows query tab `id` in the main area: its box and its last result.
pub fn enter(app: &App, window: &MainWindow, id: &str, scroll: ScrollPos) {
    let mut queries = app.queries.borrow_mut();
    queries.active = Some(id.to_string());
    queries.navigation = Navigation::default();
    let Some(tab) = queries.tabs.get(id) else { return };
    window.global::<QueryTab>().set_text(tab.text.as_str().into());
    window.global::<QueryTab>().set_ran(tab.ran);
    window.global::<QueryTab>().set_loading(tab.loading);
    window.global::<QueryTab>().set_status(tab.status.as_str().into());
    window.global::<QueryTab>().set_status_is_error(tab.status_is_error);
    show_error(window, tab.error.as_ref());
    show_results(app, window, id, tab, scroll);
}

/// Puts the tab's shown result into the key panel (and the tabs to pick another into the window).
fn show_results(app: &App, window: &MainWindow, id: &str, tab: &TabQuery, scroll: ScrollPos) {
    let labels: Vec<slint::SharedString> = if tab.results.len() > 1 { tab.results.iter().map(|r| r.label.as_str().into()).collect() } else { Vec::new() };
    window.global::<QueryTab>().set_results(ModelRc::new(VecModel::from(labels)));
    window.global::<QueryTab>().set_result_active(tab.active as i32);
    let rows = tab.shown().filter(|shown| !shown.status_is_error).map(|shown| shown.rows.clone());
    app.key_detail.show_rows(id.to_string(), rows, scroll);
}

/// How long a run gets before the Run button flips to its spinner: a query
/// that answers faster than this never shows it at all, so a quick one
/// doesn't flash the button for a single frame before snapping back.
const LOADING_DELAY: std::time::Duration = std::time::Duration::from_millis(150);

/// The script that runs for part of the box's text, and how its positions map back.
struct ScriptToRun {
    script: String,
    /// where the part taken from the box starts, in the script and in the box
    /// (the `var` declarations put before it are in neither)
    in_script: usize,
    in_box: usize,
}

impl ScriptToRun {
    /// `span` of the script as a range of the box, unless it lies in the declarations put before.
    fn to_box(&self, span: Range<usize>) -> Option<Range<usize>> {
        (span.start >= self.in_script).then(|| span.start - self.in_script + self.in_box..span.end - self.in_script + self.in_box)
    }
}

/// The script to run for the box's `text`: all of it, or the selection (between `cursor` and
/// `anchor`) or the statement at the caret, with the `var` declarations that come before it.
fn script_to_run(text: &str, cursor: usize, anchor: usize, whole: bool) -> Option<ScriptToRun> {
    if whole {
        return Some(ScriptToRun { script: text.to_string(), in_script: 0, in_box: 0 });
    }
    let statements = split_statements(text);
    let (low, high) = (cursor.min(anchor).min(text.len()), cursor.max(anchor).min(text.len()));
    let (target, before) = if low != high {
        (text.get(low..high)?.to_string(), low)
    } else {
        let at = statements.iter().find(|s| s.range.start <= low && low <= s.range.end).or_else(|| statements.iter().rev().find(|s| s.range.end <= low))?;
        (text[at.range.clone()].to_string(), at.range.start)
    };
    let mut script: String = statements.iter().filter(|s| s.range.end <= before && is_declaration(&s.text)).map(|s| format!("{};\n", s.text)).collect();
    let in_script = script.len();
    script.push_str(&target);
    Some(ScriptToRun { script, in_script, in_box: before })
}

/// Runs the box's query: all of it as a script (`whole`), or the selection or the statement at the caret.
fn run(app: &App, window: &MainWindow, query: String, cursor: usize, anchor: usize, whole: bool) {
    let started = {
        let mut queries = app.queries.borrow_mut();
        let Some(id) = queries.active.clone() else { return };
        let Some(tab) = queries.tabs.get_mut(&id) else { return };
        let run = tab.run.fetch_add(1, Ordering::Relaxed) + 1;
        tab.text.clone_from(&query);
        tab.ran = true;
        // cleared right away: the old row count must never sit next to a
        // table that's about to be replaced by this run's own result
        tab.status = t!("running...").into_owned();
        tab.status_is_error = false;
        tab.error = None;
        (id, tab.run.clone(), run)
    };
    let (id, run_counter, run_number) = started;
    let (state, rt) = (&app.state, &app.rt);
    window.global::<QueryTab>().set_ran(true);
    window.global::<QueryTab>().set_status(t!("running...").as_ref().into());
    window.global::<QueryTab>().set_status_is_error(false);
    show_error(window, None);
    let Some(to_run) = script_to_run(&query, cursor, anchor, whole).filter(|to_run| !to_run.script.trim().is_empty()) else {
        let empty = ResultTable { status: t!("The query is empty.").into_owned(), status_is_error: true, ..Default::default() };
        apply_outcome(app, window, &id, run_number, vec![empty], None);
        return;
    };

    // set once `run_script` returns: the delayed task checks it before turning the
    // spinner on, so an already-answered query never gets a spinner left on
    let done = Arc::new(std::sync::atomic::AtomicBool::new(false));

    {
        let (id, run_counter, done) = (id.clone(), run_counter.clone(), done.clone());
        rt.spawn(async move {
            tokio::time::sleep(LOADING_DELAY).await;
            if done.load(Ordering::Relaxed) {
                return;
            }
            let _ = slint::invoke_from_event_loop(move || {
                if done.load(Ordering::Relaxed) || run_counter.load(Ordering::Relaxed) != run_number {
                    return;
                }
                crate::app::with(|app, window| {
                    let mut queries = app.queries.borrow_mut();
                    if let Some(tab) = queries.tabs.get_mut(&id) {
                        tab.loading = true;
                    }
                    if queries.active.as_deref() == Some(id.as_str()) {
                        window.global::<QueryTab>().set_loading(true);
                    }
                });
            });
        });
    }

    app.queries.borrow_mut().navigation = Navigation::default();
    let connection_id = state.connections.lock_recover().active_connection_id().to_string();
    let state = state.clone();
    rt.spawn(async move {
        let started = std::time::Instant::now();
        let results = run_script(&state, None, &to_run.script).await;
        let script = to_run.script.clone();
        let error = results.last().and_then(|result| result.error_span.clone()).and_then(|span| to_run.to_box(span));
        done.store(true, Ordering::Relaxed);
        if !results.is_empty() && !connection_id.is_empty() {
            let ok = results.iter().all(|result| result.output.is_ok());
            let entry = HistoryEntry { connection_id, text: script, executed_at: now_ms(), duration_ms: started.elapsed().as_millis() as u64, ok };
            state.history.lock_recover().record(entry);
        }
        let count = results.len();
        let built: Vec<ResultTable> = results.into_iter().enumerate().map(|(i, result)| result_of(i, count, result.output)).collect();
        let _ = slint::invoke_from_event_loop(move || {
            crate::app::with(|app, window| apply_outcome(app, window, &id, run_number, built, error));
        });
    });
}

/// Where Ctrl+Up / Ctrl+Down are in the connection's history: the entry shown and what was in the
/// box before the first step.
#[derive(Default)]
struct Navigation {
    position: Option<usize>,
    draft: String,
}

/// How long ago `then` was, as the history shows it.
fn ago(now: i64, then: i64) -> String {
    let minutes = ((now - then).max(0) / 60_000) as u64;
    match minutes {
        0 => t!("just now").into_owned(),
        1..=59 => t!("%{n} min ago", n = minutes).into_owned(),
        60..=1439 => t!("%{n} h ago", n = minutes / 60).into_owned(),
        _ => t!("%{n} d ago", n = minutes / 1440).into_owned(),
    }
}

/// The earlier runs on the connection in use, newest first.
fn history_of(window: &MainWindow, state: &AppState) -> Vec<HistoryEntry> {
    let connection_id = window.connection_id().to_string();
    state.history.lock_recover().for_connection(&connection_id)
}

/// The `index`th of `count` results, from what its statement gave.
fn result_of(index: usize, count: usize, output: Result<QueryOutput, AppError>) -> ResultTable {
    let number = index + 1;
    let mut table = match output {
        Ok(output) => {
            let status = status_text(output.rows.len(), output.truncated, output.sources_truncated);
            ResultTable { label: format!("{number} · {}", output.rows.len()), status, status_is_error: false, rows: output.rows }
        }
        Err(error) => ResultTable { label: format!("{number} · !"), status: error.to_string(), status_is_error: true, rows: Vec::new() },
    };
    if count == 1 {
        table.label.clear();
    }
    table
}

fn status_text(shown: usize, truncated: bool, sources_truncated: bool) -> String {
    let base = if truncated {
        t!("%{shown} rows shown (more matched; raise LIMIT to see them)", shown = shown).into_owned()
    } else {
        t!("%{count} rows", count = shown).into_owned()
    };
    if sources_truncated {
        format!("{base} — {}", t!("a source key was too large to read fully; some matches may be missing"))
    } else {
        base
    }
}

/// Puts the results of a run into the tab, showing the last.
fn apply_outcome(app: &App, window: &MainWindow, id: &str, run_number: u64, results: Vec<ResultTable>, error: Option<Range<usize>>) {
    let mut queries = app.queries.borrow_mut();
    let is_active = queries.active.as_deref() == Some(id);
    let Some(tab) = queries.tabs.get_mut(id) else { return };
    // a newer run (or a closed tab) has taken over
    if tab.run.load(Ordering::Relaxed) != run_number {
        return;
    }
    tab.loading = false;
    tab.results = results;
    tab.error = error.map(|span| (span, tab.text.clone()));
    tab.active = tab.results.len().saturating_sub(1);
    (tab.status, tab.status_is_error) = match tab.shown() {
        Some(shown) => (shown.status.clone(), shown.status_is_error),
        None => (t!("Nothing to show: only variables were declared.").into_owned(), false),
    };
    if is_active {
        window.global::<QueryTab>().set_loading(false);
        window.global::<QueryTab>().set_status(tab.status.as_str().into());
        window.global::<QueryTab>().set_status_is_error(tab.status_is_error);
        show_results(app, window, id, tab, ScrollPos::default());
        show_error(window, tab.error.as_ref());
    }
}

/// Marks where the last run's error is in the box (see QueryTab.error-start).
fn show_error(window: &MainWindow, error: Option<&(Range<usize>, String)>) {
    let query = window.global::<QueryTab>();
    let (start, end, text) = error.map_or((-1, -1, ""), |(span, text)| (span.start as i32, span.end as i32, text.as_str()));
    query.set_error_start(start);
    query.set_error_end(end);
    query.set_error_text(text.into());
}

/// Shows result `index` of the active tab's last run.
fn select_result(app: &App, window: &MainWindow, index: usize) {
    let mut queries = app.queries.borrow_mut();
    let Some(id) = queries.active.clone() else { return };
    let Some(tab) = queries.tabs.get_mut(&id) else { return };
    if index >= tab.results.len() {
        return;
    }
    tab.active = index;
    tab.status = tab.results[index].status.clone();
    tab.status_is_error = tab.results[index].status_is_error;
    window.global::<QueryTab>().set_status(tab.status.as_str().into());
    window.global::<QueryTab>().set_status_is_error(tab.status_is_error);
    show_results(app, window, &id, tab, ScrollPos::default());
}

/// How long typing pauses before the completion list is worked out.
const SUGGEST_DELAY: std::time::Duration = std::time::Duration::from_millis(150);

/// The completion list of the query box: worked out off the UI thread (it may list the keyspace)
/// and shown if the text is still what it was asked about.
fn suggest(app: &App, caches: &Arc<Caches>, text: String, cursor: usize, force: bool) {
    let latest = app.queries.borrow().suggest_generation.clone();
    let generation = latest.fetch_add(1, Ordering::SeqCst) + 1;
    let (state, caches) = (app.state.clone(), caches.clone());
    app.rt.spawn(async move {
        if !force {
            tokio::time::sleep(SUGGEST_DELAY).await;
            if latest.load(Ordering::SeqCst) != generation {
                return;
            }
        }
        let found = complete(&state, &caches, &text, cursor, force).await;
        let _ = slint::invoke_from_event_loop(move || crate::app::with(|app, window| {
            if latest.load(Ordering::SeqCst) != generation || window.global::<QueryTab>().get_text().as_str() != text {
                return;
            }
            let rows: Vec<SuggestionRow> = found
                .as_ref()
                .map(|found| {
                    found
                        .items
                        .iter()
                        .map(|(item, hit)| SuggestionRow { before: item[..hit.start].into(), matched: item[hit.clone()].into(), after: item[hit.end..].into() })
                        .collect()
                })
                .unwrap_or_default();
            window.global::<QueryTab>().set_suggest_open(!rows.is_empty());
            window.global::<QueryTab>().set_suggestions(ModelRc::new(VecModel::from(rows)));
            app.queries.borrow_mut().offered = found;
        }));
    });
}

/// Wires the query box of the query tabs.
pub fn init(app: &Rc<App>, window: &MainWindow) {
    let caches = Arc::new(Caches::default());
    // the query's lines, to number them
    window.global::<QueryTab>().on_lines(|text| ModelRc::new(VecModel::from(text.split('\n').map(slint::SharedString::from).collect::<Vec<_>>())));
    window.global::<QueryTab>().on_suggest({
        let (app, caches) = (app.clone(), caches.clone());
        move |text, cursor, force| suggest(&app, &caches, text.to_string(), cursor.max(0) as usize, force)
    });
    // a row chosen: the box's text with it written in, and a folder's own children offered at once
    window.global::<QueryTab>().on_complete({
        let (app, caches) = (app.clone(), caches);
        move |text, cursor, index| {
            let index = index.max(0) as usize;
            let accepted = app.queries.borrow().offered.as_ref().and_then(|found| found.accept(&text, index).map(|edit| (edit, found.items[index].0.ends_with(':'))));
            let Some(((next, caret), folder)) = accepted else { return CompletionEdit { text, cursor: -1 } };
            let _ = cursor;
            if folder {
                let (caches, next) = (caches.clone(), next.clone());
                let _ = slint::invoke_from_event_loop(move || crate::app::with(|app, _| suggest(app, &caches, next, caret, true)));
            }
            CompletionEdit { text: next.into(), cursor: caret as i32 }
        }
    });

    // recomputed on every edit — see redisql_highlight's own doc comment for
    // why that's cheap enough to need no caching
    window.global::<QueryTab>().on_highlight(|text, start, end, error_text| {
        let error = (start >= 0 && end > start && text == error_text).then_some(start as usize..end as usize);
        crate::redisql_highlight::highlight_query(&text, error)
    });

    // strips `\r` a pasted Windows query brings in, right after it lands in
    // the box (see query-panel.slint's `changed query` and
    // redisql_highlight::normalize_line_endings)
    window.global::<QueryTab>().on_sanitize(|text| crate::redisql_highlight::normalize_line_endings(&text).into());

    window.global::<QueryTab>().on_run_requested({
        let app = app.clone();
        move |query, cursor, anchor, script| {
            if let Some(window) = app.window.upgrade() {
                run(&app, &window, query.to_string(), cursor.max(0) as usize, anchor.max(0) as usize, script);
            }
        }
    });

    // the history button: the runs on this connection, newest first
    window.global::<QueryTab>().on_history_requested({
        let app = app.clone();
        move || {
            let Some(window) = app.window.upgrade() else { return };
            let entries = history_of(&window, &app.state);
            let now = now_ms();
            let rows: Vec<HistoryRow> = entries
                .iter()
                .map(|e| HistoryRow {
                    label: e.text.split_whitespace().collect::<Vec<_>>().join(" ").into(),
                    detail: t!("%{ago} · %{ms} ms", ago = ago(now, e.executed_at), ms = e.duration_ms).into_owned().into(),
                    failed: !e.ok,
                })
                .collect();
            app.queries.borrow_mut().history_shown = entries.into_iter().map(|e| e.text).collect();
            window.global::<QueryTab>().set_history(ModelRc::new(VecModel::from(rows)));
        }
    });
    // a row picked: its text goes in the box, and runs when asked
    window.global::<QueryTab>().on_history_picked({
        let app = app.clone();
        move |index, run_it| {
            let Some(window) = app.window.upgrade() else { return };
            let Some(text) = app.queries.borrow().history_shown.get(index.max(0) as usize).cloned() else { return };
            window.global::<QueryTab>().set_text(text.as_str().into());
            if run_it {
                run(&app, &window, text.clone(), text.len(), text.len(), true);
            }
        }
    });
    window.global::<QueryTab>().on_history_cleared({
        let app = app.clone();
        move || {
            let Some(window) = app.window.upgrade() else { return };
            let connection_id = window.connection_id().to_string();
            app.state.history.lock_recover().clear(&connection_id);
            app.queries.borrow_mut().history_shown.clear();
            window.global::<QueryTab>().set_history(ModelRc::new(VecModel::from(Vec::<HistoryRow>::new())));
        }
    });
    // Ctrl+Up / Ctrl+Down: older and newer runs, back to what was typed after the newest
    window.global::<QueryTab>().on_history_step({
        let app = app.clone();
        move |direction, current| {
            let none = CompletionEdit { text: current.clone(), cursor: -1 };
            let Some(window) = app.window.upgrade() else { return none };
            let entries = history_of(&window, &app.state);
            let mut queries = app.queries.borrow_mut();
            let navigation = &mut queries.navigation;
            // a text edited since the last step starts again from what is in the box
            if navigation.position.is_none_or(|p| entries.get(p).is_none_or(|e| e.text != current.as_str())) {
                navigation.position = None;
                navigation.draft = current.to_string();
            }
            let next = match (navigation.position, direction < 0) {
                (None, true) => Some(0),
                (Some(p), true) => Some(p + 1),
                (Some(0), false) => None,
                (Some(p), false) => Some(p - 1),
                (None, false) => return none,
            };
            let text = match next {
                Some(p) => match entries.get(p) {
                    Some(entry) => entry.text.clone(),
                    None => return none,
                },
                None => navigation.draft.clone(),
            };
            navigation.position = next;
            CompletionEdit { cursor: text.len() as i32, text: text.into() }
        }
    });

    window.global::<QueryTab>().on_result_selected({
        let app = app.clone();
        move |index| {
            if let Some(window) = app.window.upgrade() {
                select_result(&app, &window, index.max(0) as usize);
            }
        }
    });

    // "Save" in the query box's header: saves the box under `name` in the sidebar's
    // Query list; a failure is only logged, as there's no place to show it.
    window.global::<QueryTab>().on_save_requested({
        let app = app.clone();
        move |name, global| {
            let Some(window) = app.window.upgrade() else { return };
            let state = &app.state;
            let text = window.global::<QueryTab>().get_text().to_string();
            let connection_id = if global { String::new() } else { window.connection_id().to_string() };
            let saved = state.settings.lock_recover().create_saved_query(&name, &text, &connection_id);
            match saved {
                Ok(_) => crate::queries::refresh(&window, state),
                Err(e) => eprintln!("save query failed: {e}"),
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_query_tab_id_is_never_a_key_or_a_search_tab() {
        let mut tabs = QueryTabs::default();
        let id = tabs.create(String::new());
        assert!(is_query_id(&id));
        assert!(!is_query_id("search:1"));
        assert!(!is_query_id("\u{1}search:1"));
        assert_ne!(id, tabs.create(String::new()));
        tabs.forget(&id);
    }
}
