//! Manages which keys have an open tab and which one is active. The tab bar
//! and keyspace tree both drive selection through [`select_key`].

use crate::backend::LockExt;
use rustc_hash::{FxHashMap, FxHashSet};
use std::rc::Rc;

use slint::{ComponentHandle, Model, ModelRc, VecModel};

use crate::app::App;
use crate::backend::key_name::KeyName;
use crate::key_detail::{ScrollPos, current_expanded_fields, current_scroll, restore_expanded_fields};
use crate::redisql;
use crate::search;
use crate::{AllValuesRowData, BadgeData, ConnectionState, ExportPrefs, KeyPanel, KeyTree, KeyView, MainWindow, Preferences, SavedQueries, SearchScope, SearchTab, TabData, TabStrip};
use rust_i18n::t;

/// Per-tab UI state outside the fetched key data: the search bar, scroll
/// position, open value-list rows. Saved/restored on tab switch; a new tab
/// always starts from `default_for_new_tab`, never the previous tab's state.
#[derive(Clone, Default)]
struct TabViewState {
    // the search bar's active badges, currently-picked scope and
    // case-sensitivity toggle (see KeyDetail.filter-badges)
    badges: Vec<BadgeData>,
    scope: SearchScope,
    case_sensitive: bool,
    json_filter: String,
    // which of the key panel's views the tab was left on
    view: KeyView,
    // where the key panel was scrolled to, so the tab comes back to the same place
    scroll: ScrollPos,
    // the value-list rows left open, so the tab comes back with them still open
    expanded_fields: FxHashSet<String>,
    // whether a hash's fields are listed in descending order (see KeyPanel.sort-descending)
    descending: bool,
    // the header's JSON / Escaped switch (see ExportPrefs.header-escaped)
    escaped: bool,
}

impl TabViewState {
    fn default_for_new_tab() -> Self {
        Self::default()
    }
}

/// Which special (non-key) tab kind an id names, if any: a search tab (see
/// `search.rs`) or a query tab (see `redisql.rs`). Neither has a key panel of
/// its own, and each keeps its own state under its own id prefix.
enum Special {
    Search,
    Query,
}

fn special_kind(key: &str) -> Option<Special> {
    if search::is_search_id(key) {
        Some(Special::Search)
    } else if redisql::is_query_id(key) {
        Some(Special::Query)
    } else {
        None
    }
}

/// A special tab is being left: keeps its own state (see `Special`). Returns whether it is one with no
/// key panel (a search), so the caller can skip the key-panel handling that follows; a query tab
/// shows its result in the panel, whose state is kept like a key's.
fn leave_special(app: &App, window: &MainWindow, key: &str) -> bool {
    match special_kind(key) {
        Some(Special::Search) => {
            app.searches.borrow_mut().leave(window, key);
            true
        }
        Some(Special::Query) => {
            app.queries.borrow_mut().leave(window, key);
            false
        }
        None => false,
    }
}

fn forget_special(app: &App, key: &str) {
    match special_kind(key) {
        Some(Special::Search) => app.searches.borrow_mut().forget(key),
        Some(Special::Query) => app.queries.borrow_mut().forget(key),
        None => {}
    }
}

/// The tabs of a connection that isn't the current one: hidden until it is
/// switched back to.
struct ParkedTabs {
    open: Vec<String>,
    active: Option<String>,
    view_state: FxHashMap<String, TabViewState>,
}

/// The open tabs of the connection shown, and those every other one left behind.
pub struct Tabs {
    open: Vec<String>,
    active: Option<String>,
    view_state: FxHashMap<String, TabViewState>,
    // saved-connection id the key panel's content was last loaded under: the
    // same key name can hold something else on another server
    loaded_on: String,
    // the connection `open`/`active`/`view_state` belong to ("" while none is
    // connected), and the tabs every other connection left behind. Kept for
    // the run only, never saved.
    connection: String,
    parked: FxHashMap<String, ParkedTabs>,
    // whether the keyspace tree highlights the active tab's key (a key clicked in
    // the tree, or revealed): that highlight is always the active tab's key, so
    // this is all that needs remembering for a connection that isn't shown
    highlighted: bool,
    parked_highlights: FxHashSet<String>,
    // the next connection change drops the tabs it leaves instead of parking them
    discard_on_leave: bool,
}

impl Default for Tabs {
    fn default() -> Self {
        Self::new()
    }
}

impl Tabs {
    fn new() -> Self {
        Self {
            open: Vec::new(),
            active: None,
            view_state: FxHashMap::default(),
            loaded_on: String::new(),
            connection: String::new(),
            parked: FxHashMap::default(),
            highlighted: false,
            parked_highlights: FxHashSet::default(),
            discard_on_leave: false,
        }
    }

    /// The connections whose tabs are parked, to be shown again when switched back to.
    pub fn parked_connections(&self) -> FxHashSet<String> {
        self.parked.keys().cloned().collect()
    }

    /// The next connection change drops the tabs of the one it leaves.
    pub fn discard_on_leave(&mut self) {
        self.discard_on_leave = true;
    }

    /// Hides the current connection's tabs and brings back those of
    /// `connection`, if it left any. `keep_previous` is false when the
    /// connection being left no longer exists (it was deleted).
    fn switch_connection(&mut self, connection: &str, keep_previous: bool) {
        if self.connection == connection {
            return;
        }
        let leaving = ParkedTabs {
            open: std::mem::take(&mut self.open),
            active: self.active.take(),
            view_state: std::mem::take(&mut self.view_state),
        };
        let was_highlighted = std::mem::take(&mut self.highlighted);
        if keep_previous && was_highlighted {
            self.parked_highlights.insert(self.connection.clone());
        }
        if keep_previous && !leaving.open.is_empty() {
            self.parked.insert(std::mem::take(&mut self.connection), leaving);
        }
        self.connection = connection.to_string();
        self.highlighted = self.parked_highlights.remove(connection);
        if let Some(back) = self.parked.remove(connection) {
            self.open = back.open;
            self.active = back.active;
            self.view_state = back.view_state;
        }
    }
}

fn render(window: &MainWindow, tabs: &Tabs) {
    let rows: Vec<TabData> = tabs
        .open
        .iter()
        .map(|key| {
            let kind = special_kind(key);
            let label = match kind {
                Some(Special::Search) => t!("Search").into_owned(),
                Some(Special::Query) => t!("Query").into_owned(),
                None => KeyName::display_id(key).into_owned(),
            };
            TabData {
                key: key.as_str().into(),
                active: tabs.active.as_deref() == Some(key.as_str()),
                label: label.into(),
                search: matches!(kind, Some(Special::Search)),
                query: matches!(kind, Some(Special::Query)),
            }
        })
        .collect();
    window.global::<TabStrip>().set_active_is_search(tabs.active.as_deref().is_some_and(search::is_search_id));
    window.global::<TabStrip>().set_active_is_query(tabs.active.as_deref().is_some_and(redisql::is_query_id));
    window.global::<TabStrip>().set_tabs(ModelRc::new(VecModel::from(rows)));
    let active_index = tabs
        .active
        .as_deref()
        .and_then(|active| tabs.open.iter().position(|k| k == active))
        .map(|i| i as i32)
        .unwrap_or(-1);
    window.global::<TabStrip>().set_active_index(active_index);
    if tabs.active.is_none() {
        window.global::<KeyTree>().set_selected("".into());
    }
}

fn clear_detail_view(window: &MainWindow) {
    window.global::<KeyPanel>().set_badge("".into());
    window.global::<KeyPanel>().set_loaded(false);
    crate::key_detail::show_key_name(window, "");
    window.global::<KeyPanel>().set_meta("".into());
    window.global::<KeyPanel>().set_all_values_field_chars(8);
    window.global::<KeyPanel>().set_all_values_rows(ModelRc::new(VecModel::from(Vec::<AllValuesRowData>::new())));
    window.global::<KeyPanel>().set_filter_badges(ModelRc::new(VecModel::from(Vec::<BadgeData>::new())));
    window.global::<KeyPanel>().set_filter_scope(SearchScope::default());
    window.global::<KeyPanel>().set_filter_case_sensitive(false);
    window.global::<KeyPanel>().set_filter_query("".into());
    window.global::<KeyPanel>().set_filter_limit_message("".into());
    window.global::<KeyPanel>().set_json_filter("".into());
    window.global::<KeyPanel>().set_json_filter_status("".into());
    window.global::<KeyPanel>().set_json_filter_error("".into());
    window.global::<KeyPanel>().set_sort_descending(false);
    window.global::<KeyPanel>().set_view(KeyView::default());
    crate::key_detail::clear_table(window);
}

/// Reads the window's live filter state into `tabs.view_state` under `key`
/// — call this for the tab that's about to stop being active, before its
/// data on screen gets replaced by the next tab's.
fn save_view_state(app: &App, window: &MainWindow, tabs: &mut Tabs, key: &str) {
    // a search or a query tab keeps its own state (see Special)
    if leave_special(app, window, key) {
        return;
    }
    tabs.view_state.insert(
        key.to_string(),
        TabViewState {
            badges: window.global::<KeyPanel>().get_filter_badges().iter().collect(),
            scope: window.global::<KeyPanel>().get_filter_scope(),
            case_sensitive: window.global::<KeyPanel>().get_filter_case_sensitive(),
            json_filter: window.global::<KeyPanel>().get_json_filter().to_string(),
            view: window.global::<KeyPanel>().get_view(),
            scroll: current_scroll(window),
            expanded_fields: current_expanded_fields(window),
            descending: window.global::<KeyPanel>().get_sort_descending(),
            escaped: window.global::<ExportPrefs>().get_header_escaped(),
        },
    );
}

/// Writes `key`'s saved state (or the default for an unvisited tab) into
/// the window, for `load_detail` to read (including, for open rows,
/// `current_expanded_fields`). Returns the scroll position it applies later.
fn restore_view_state(window: &MainWindow, tabs: &Tabs, key: &str) -> ScrollPos {
    if matches!(special_kind(key), Some(Special::Search)) {
        return ScrollPos::default();
    }
    // a query tab's result starts as a table (a key's fields start as the list of fields),
    // and a new tab's export switch as the settings say
    let state = tabs.view_state.get(key).cloned().unwrap_or_else(|| TabViewState {
        view: if redisql::is_query_id(key) { KeyView::Table } else { KeyView::default() },
        escaped: window.global::<Preferences>().get_export_escaped(),
        ..TabViewState::default_for_new_tab()
    });
    window.global::<KeyPanel>().set_filter_badges(ModelRc::new(VecModel::from(state.badges)));
    window.global::<KeyPanel>().set_filter_scope(state.scope);
    window.global::<KeyPanel>().set_filter_case_sensitive(state.case_sensitive);
    window.global::<KeyPanel>().set_filter_query("".into());
    window.global::<KeyPanel>().set_filter_limit_message("".into());
    window.global::<KeyPanel>().set_json_filter(state.json_filter.into());
    window.global::<KeyPanel>().set_view(state.view);
    restore_expanded_fields(window, &state.expanded_fields);
    window.global::<KeyPanel>().set_sort_descending(state.descending);
    window.global::<ExportPrefs>().set_header_escaped(state.escaped);
    window.global::<ExportPrefs>().set_escaped(state.escaped);
    state.scroll
}

/// Shows a tab's content in the main area: a key is loaded (or shown as it was
/// fetched), a search tab shows its box and hits, a query tab its box and result.
fn show_tab(app: &App, window: &MainWindow, key: String, scroll: ScrollPos, reuse: bool) {
    match special_kind(&key) {
        Some(Special::Search) => app.searches.borrow_mut().enter(window, &key),
        Some(Special::Query) => crate::redisql::enter(app, window, &key, scroll),
        None => app.key_detail.load(key, scroll, reuse),
    }
}

/// Makes `id`, a tab just created, the open and active one, leaving the previous
/// tab as it was; returns where it scrolls to.
fn open_new_tab(app: &App, window: &MainWindow, id: &str) -> ScrollPos {
    let mut tabs = app.tabs.borrow_mut();
    if let Some(prev) = tabs.active.clone() {
        save_view_state(app, window, &mut tabs, &prev);
    }
    tabs.open.push(id.to_string());
    tabs.active = Some(id.to_string());
    // the tree's highlight was the previous tab's key
    tabs.highlighted = false;
    window.global::<KeyTree>().set_selected("".into());
    let scroll = restore_view_state(window, &tabs, id);
    render(window, &tabs);
    scroll
}

/// Opens (or activates) the tab for a key and shows it; an already-open tab is
/// shown as fetched, not fetched again.
pub fn select_key(app: &App, key: String) {
    let Some(window) = app.window.upgrade() else { return };
    let (scroll, reuse) = {
        let mut tabs = app.tabs.borrow_mut();
        // clicking the active tab (or its key in the tree) again while
        // the key panel already shows it, loaded, has nothing to
        // fetch; a failed load has no badge, so that one is retried
        let connection_id = window.connection_id().to_string();
        let already_shown = tabs.active.as_deref() == Some(key.as_str())
            && (special_kind(&key).is_some() || (window.global::<KeyPanel>().get_loaded() && tabs.loaded_on == connection_id));
        if already_shown {
            return;
        }
        tabs.loaded_on = connection_id;
        // save the tab we're switching away from (if any) before its
        // on-screen state gets overwritten by the one we're opening
        if let Some(prev_key) = tabs.active.clone() {
            save_view_state(app, &window, &mut tabs, &prev_key);
        }
        // a tab already open comes back as it was fetched; a new one is fetched
        let reuse = tabs.open.contains(&key);
        if !reuse {
            tabs.open.push(key.clone());
        }
        tabs.active = Some(key.clone());
        let scroll = restore_view_state(&window, &tabs, &key);
        render(&window, &tabs);
        // a click in the tree has just highlighted this key; a click on a
        // tab leaves the old highlight, which is about to be removed
        tabs.highlighted = window.global::<KeyTree>().get_selected().as_str() == key;
        (scroll, reuse)
    };
    show_tab(app, &window, key, scroll, reuse);
}

/// "New search": a search tab, opened and shown.
pub fn open_search(app: &App) {
    let Some(window) = app.window.upgrade() else { return };
    let id = app.searches.borrow_mut().create();
    open_new_tab(app, &window, &id);
    app.searches.borrow_mut().enter(&window, &id);
}

/// "New query" (empty `text`), or a saved query opened from the sidebar: a new
/// Redisql tab, pre-filled but not run — the user reviews it before firing it.
pub fn open_query(app: &App, text: String) {
    let Some(window) = app.window.upgrade() else { return };
    let id = app.queries.borrow_mut().create(text);
    let scroll = open_new_tab(app, &window, &id);
    crate::redisql::enter(app, &window, &id, scroll);
}

/// Each connection has its own tabs: switching hides the ones on screen and
/// shows the new connection's (a failed connect or a delete leaves no
/// connection, so nothing shows). The key panel follows the active tab.
fn on_connection_changed(app: &App, window: &MainWindow) {
    let state = &app.state;
    let connection = window.connection_id().to_string();
    crate::queries::refresh(window, state);
    let active = {
        let mut tabs = app.tabs.borrow_mut();
        if tabs.connection == connection {
            return;
        }
        let mut scroll = ScrollPos::default();
        // what's typed in the active tab's filters goes with it
        if let Some(active) = tabs.active.clone() {
            save_view_state(app, window, &mut tabs, &active);
        }
        let discard = std::mem::take(&mut tabs.discard_on_leave);
        let still_exists = tabs.connection.is_empty() || state.connections.lock_recover().get(&tabs.connection).is_some();
        let keep = still_exists && !discard;
        if !keep {
            app.key_detail.forget(&tabs.connection, None);
            tabs.open.iter().for_each(|id| forget_special(app, id));
        }
        tabs.switch_connection(&connection, keep);
        render(window, &tabs);
        // the tree was reloaded for this connection (which cleared its
        // highlight): bring back the one it had when it was left
        let highlighted_key = tabs.active.clone().filter(|_| tabs.highlighted);
        window.global::<KeyTree>().set_selected(highlighted_key.unwrap_or_default().into());
        if let Some(active) = tabs.active.clone() {
            tabs.loaded_on = connection;
            scroll = restore_view_state(window, &tabs, &active);
        }
        tabs.active.clone().map(|active| (active, scroll))
    };
    // (after the tabs are released: the list shows which connections have tabs parked)
    crate::connections::refresh(app, window);
    match active {
        Some((key, scroll)) => show_tab(app, window, key, scroll, true),
        None => clear_detail_view(window),
    }
}

fn on_tab_closed(app: &App, window: &MainWindow, key: String) {
    let (next_active, was_active) = {
        let mut tabs = app.tabs.borrow_mut();
        let Some(pos) = tabs.open.iter().position(|k| k == &key) else {
            return;
        };
        tabs.open.remove(pos);
        tabs.view_state.remove(&key);
        if tabs.active.as_deref() == Some(key.as_str()) {
            let neighbor = if pos > 0 { pos - 1 } else { 0 };
            tabs.active = tabs.open.get(neighbor).cloned();
            // the active tab changed: the tree highlight goes with the closed one
            tabs.highlighted = false;
            (tabs.active.clone(), true)
        } else {
            (None, false)
        }
    };

    app.key_detail.forget(window.connection_id().as_str(), Some(&key));
    forget_special(app, &key);
    if was_active {
        window.global::<KeyTree>().set_selected("".into());
    }
    let is_empty = {
        let tabs = app.tabs.borrow();
        render(window, &tabs);
        tabs.open.is_empty()
    };
    if is_empty {
        clear_detail_view(window);
    } else if let Some(next_key) = next_active {
        let scroll = {
            let mut tabs = app.tabs.borrow_mut();
            tabs.loaded_on = window.connection_id().to_string();
            restore_view_state(window, &tabs, &next_key)
        };
        show_tab(app, window, next_key, scroll, true);
    }
}

fn on_close_other_tabs(app: &App, window: &MainWindow, key: String) {
    let needs_load = {
        let mut tabs = app.tabs.borrow_mut();
        if !tabs.open.contains(&key) {
            return;
        }
        let mut scroll = ScrollPos::default();
        let was_active = tabs.active.as_deref() == Some(key.as_str());
        let connection = window.connection_id();
        for closed in tabs.open.iter().filter(|k| **k != key) {
            app.key_detail.forget(connection.as_str(), Some(closed));
            forget_special(app, closed);
        }
        tabs.open.retain(|k| k == &key);
        tabs.view_state.retain(|k, _| k == &key);
        tabs.active = Some(key.clone());
        render(window, &tabs);
        // the kept tab already shows its own content if it was the
        // active one; otherwise switch the key panel over to it
        if !was_active {
            tabs.loaded_on = window.connection_id().to_string();
            scroll = restore_view_state(window, &tabs, &key);
            // another tab became the active one: the tree highlight goes
            tabs.highlighted = false;
            window.global::<KeyTree>().set_selected("".into());
        }
        (!was_active).then_some(scroll)
    };
    if let Some(scroll) = needs_load {
        show_tab(app, window, key, scroll, true);
    }
}

/// A click on a hit of a search tab: the hit's key opens (or comes forward) with
/// the row of the hit open and scrolled to, in the list view and with no filter hiding it.
fn on_search_hit_opened(app: &App, window: &MainWindow, key: String, field: String) {
    {
        let mut tabs = app.tabs.borrow_mut();
        let view = tabs.view_state.entry(key.clone()).or_insert_with(TabViewState::default_for_new_tab);
        view.view = KeyView::Fields;
        view.badges.clear();
        view.json_filter.clear();
        if !field.is_empty() {
            view.expanded_fields.insert(field.clone());
        }
    }
    let scroll = window.global::<crate::KeyScroll>();
    scroll.set_reveal_pending(!field.is_empty());
    scroll.set_reveal_field(field.into());
    scroll.set_reveal_index(-1);
    select_key(app, key);
}

/// Wires the tab bar, and the buttons and links that open tabs.
pub fn init(app: &Rc<App>, window: &MainWindow) {
    // runs `f` with the app and the window, from a callback
    fn on<A>(app: &Rc<App>, f: impl Fn(&App, &MainWindow, A) + 'static) -> impl Fn(A) + 'static {
        let app = app.clone();
        move |arg| {
            if let Some(window) = app.window.upgrade() {
                f(&app, &window, arg);
            }
        }
    }

    let changed = on(app, |app, window, ()| on_connection_changed(app, window));
    window.global::<ConnectionState>().on_switched(move || changed(()));
    let new_search = on(app, |app, _, ()| open_search(app));
    window.global::<TabStrip>().on_new_search_clicked(move || new_search(()));
    let new_query = on(app, |app, _, ()| open_query(app, String::new()));
    window.global::<TabStrip>().on_new_query_clicked(move || new_query(()));
    // a saved query in the sidebar: a new tab (not reused — like "New query", always fresh)
    let saved_query = on(app, |app, _, id: String| {
        let saved = app.state.settings.lock_recover().saved_query(&id);
        if let Some(saved) = saved {
            open_query(app, saved.text);
        }
    });
    window.global::<SavedQueries>().on_clicked(move |id| saved_query(id.to_string()));

    let selected = on(app, |app, window, key: String| {
        let switching = app.tabs.borrow().active.as_deref() != Some(key.as_str());
        select_key(app, key);
        // the tree's highlight marks the key that was clicked in the tree:
        // switching tabs takes it away (the reveal button brings it back)
        if switching {
            app.tabs.borrow_mut().highlighted = false;
            window.global::<KeyTree>().set_selected("".into());
        }
    });
    window.global::<TabStrip>().on_selected(move |key| selected(key.to_string()));

    let reveal = on(app, |app, window, ()| {
        let active = app.tabs.borrow().active.clone().filter(|key| special_kind(key).is_none());
        if let Some(key) = active {
            window.global::<KeyTree>().invoke_focus_key(key.into());
            app.tabs.borrow_mut().highlighted = true;
        }
    });
    window.global::<KeyTree>().on_reveal_active_tab(move || reveal(()));

    let reordered = on(app, |app, window, (key, new_index): (String, i32)| {
        let mut tabs = app.tabs.borrow_mut();
        if let Some(from) = tabs.open.iter().position(|k| k == &key) {
            let item = tabs.open.remove(from);
            let to = (new_index.max(0) as usize).min(tabs.open.len());
            tabs.open.insert(to, item);
        }
        render(window, &tabs);
    });
    window.global::<TabStrip>().on_reordered(move |key, new_index| reordered((key.to_string(), new_index)));

    let close_all = on(app, |app, window, ()| {
        app.key_detail.forget(window.connection_id().as_str(), None);
        let mut tabs = app.tabs.borrow_mut();
        tabs.open.iter().for_each(|id| forget_special(app, id));
        tabs.open.clear();
        tabs.active = None;
        tabs.highlighted = false;
        tabs.view_state.clear();
        render(window, &tabs);
        clear_detail_view(window);
    });
    window.global::<TabStrip>().on_close_all(move || close_all(()));

    let close_others = on(app, |app, window, key: String| on_close_other_tabs(app, window, key));
    window.global::<TabStrip>().on_close_others(move |key| close_others(key.to_string()));

    // Ctrl+W: closes the active tab like its × does
    let close_active = on(app, |app, window, ()| {
        let active = app.tabs.borrow().active.clone();
        if let Some(active) = active {
            window.global::<TabStrip>().invoke_closed(active.into());
        }
    });
    window.global::<TabStrip>().on_close_active(move || close_active(()));

    let closed = on(app, |app, window, key: String| on_tab_closed(app, window, key));
    window.global::<TabStrip>().on_closed(move |key| closed(key.to_string()));

    let hit_opened = on(app, |app, window, (key, field): (String, String)| on_search_hit_opened(app, window, key, field));
    window.global::<SearchTab>().on_hit_opened(move |key, field| hit_opened((key.to_string(), field.to_string())));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open(tabs: &mut Tabs, keys: &[&str]) {
        tabs.open = keys.iter().map(|k| k.to_string()).collect();
        tabs.active = keys.last().map(|k| k.to_string());
    }

    #[test]
    fn each_connection_gets_its_tabs_back() {
        let mut tabs = Tabs::new();
        tabs.switch_connection("a", true);
        open(&mut tabs, &["users", "orders"]);
        tabs.view_state.insert("users".into(), TabViewState { json_filter: "x".into(), ..Default::default() });

        tabs.switch_connection("b", true);
        assert!(tabs.open.is_empty() && tabs.active.is_none(), "b starts with no tabs");
        open(&mut tabs, &["cache"]);

        tabs.switch_connection("a", true);
        assert_eq!((tabs.open.clone(), tabs.active.clone()), (vec!["users".to_string(), "orders".to_string()], Some("orders".to_string())));
        assert_eq!(tabs.view_state["users"].json_filter, "x");

        tabs.switch_connection("b", true);
        assert_eq!(tabs.open, vec!["cache".to_string()]);
        // switching to the connection already shown changes nothing
        tabs.switch_connection("b", true);
        assert_eq!(tabs.open, vec!["cache".to_string()]);
    }

    #[test]
    fn no_connection_hides_the_tabs_and_a_deleted_one_forgets_them() {
        let mut tabs = Tabs::new();
        tabs.switch_connection("a", true);
        open(&mut tabs, &["k"]);
        tabs.switch_connection("", true);
        assert!(tabs.open.is_empty());
        tabs.switch_connection("a", true);
        assert_eq!(tabs.open, vec!["k".to_string()]);

        tabs.switch_connection("", false);
        tabs.switch_connection("a", true);
        assert!(tabs.open.is_empty(), "the tabs of a connection that no longer exists aren't kept");
    }

    #[test]
    fn the_tree_highlight_is_remembered_per_connection() {
        let mut tabs = Tabs::new();
        tabs.switch_connection("a", true);
        open(&mut tabs, &["users"]);
        tabs.highlighted = true;

        tabs.switch_connection("b", true);
        assert!(!tabs.highlighted, "b starts without one");
        open(&mut tabs, &["cache"]);

        tabs.switch_connection("a", true);
        assert!(tabs.highlighted, "a's key is highlighted again");
        tabs.highlighted = false;
        tabs.switch_connection("b", true);
        assert!(!tabs.highlighted, "b's key never was");
        tabs.switch_connection("a", true);
        assert!(!tabs.highlighted, "and a's no longer is");

        // a deleted connection leaves nothing behind
        tabs.highlighted = true;
        tabs.switch_connection("", false);
        tabs.switch_connection("a", true);
        assert!(!tabs.highlighted);
    }
}

