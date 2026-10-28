//! Bridges `cachemire_core::explorer::get_key_details` to the key panel. Split by
//! concern across submodules ([`field`], [`pretty_text`]/[`highlight`],
//! [`table`]/[`value_list`], [`export`], [`scroll`]); this wires them together.

use parking_lot::Mutex;
use rustc_hash::{FxHashMap, FxHashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use slint::{ComponentHandle, Model, ModelRc, VecModel, Weak};
use tokio::runtime::Handle;

use crate::{AllValuesRowData, CompletionEdit, KeyPanel, KeyView, MainWindow, SuggestionRow};
use cachemire_core::json_path::JsonPath;
use cachemire_core::key_name::KeyName;
use cachemire_core::redis_value_type::RedisValueType;
use cachemire_core::state::AppState;

mod color;
mod edit;
mod export;
mod field;
mod filtering;
mod full_value;
mod highlight;
mod load;
mod paging;
mod pretty;
mod pretty_text;
mod scroll;
mod search_filter;
mod suggest;
mod table;
#[cfg(test)]
mod test_support;
#[cfg(test)]
pub(crate) mod tests;
mod value_lines;
mod value_list;
mod word;

pub(crate) use highlight::{search_plain_text, search_value_text};
pub use scroll::{ScrollPos, current_expanded_fields, current_scroll, restore_expanded_fields};
pub use table::clear_table;

use field::{Fields, PrettyCache};
use load::LoadRequest;
use paging::PageDir;
use scroll::{arm_table_scroll, disarm_scroll, set_field_expanded, sync_rows_expansion};
use search_filter::{SearchFilter, SearchTerm};
use suggest::Suggester;
use table::{apply_table, build_table, reset_table_selection, select_table_range, table_fields, table_sort};
use value_list::meta_text;

const MAX_ITEMS: usize = 500;
/// Badges the search bar holds at once; adding more is blocked past this.
const MAX_BADGES: usize = 5;
/// A hash's fields are fetched this many at a time: enough for the first
/// screenful, cheap enough that scrolling near the end of what's loaded (or a
/// search/filter looking further in) barely has to wait for the next page.
const HASH_PAGE_SIZE: usize = 200;

/// The search bar's current state, read from the window: its badges (the
/// model Rust itself just wrote, so this never disagrees with what's shown)
/// and the case-sensitivity toggle.
fn current_search_filter(window: &MainWindow) -> SearchFilter {
    let terms =
        window.global::<KeyPanel>().get_filter_badges().iter().map(|b| SearchTerm { text: b.text.to_string(), scope: b.scope }).collect();
    SearchFilter::new(terms, window.global::<KeyPanel>().get_filter_case_sensitive())
}

/// A key as fetched: what a tab shows again when it's switched back to,
/// until the header's refresh button fetches it anew.
#[derive(Clone)]
struct FetchedKey {
    badge: String,
    meta: String,
    fields: Fields,
    // for a hash: the field names read so far, sorted; `fields` holds the values of
    // `names[start..start + fields.len()]`. `None` for every other type.
    all_names: Option<Vec<String>>,
    start: usize,
    // for a hash: its field count and TTL (for the header), and where the scan of
    // its names goes on (`None` once they're all read)
    field_count: usize,
    ttl: i64,
    names_cursor: Option<u64>,
    // for a hash: whether its names are sorted descending rather than ascending
    descending: bool,
    // whether its values can be written back: a hash's fields, or a string read whole
    editable: bool,
}

impl FetchedKey {
    /// The header's size and TTL for a hash, as many names as are read so far.
    fn hash_meta(&self) -> String {
        let loaded = self.names_cursor.and(self.all_names.as_ref()).map(Vec::len);
        meta_text(&RedisValueType::Hash, self.ttl, self.field_count, false, loaded)
    }
}

/// Fetched keys by (saved-connection id, key name). Filled by every
/// successful fetch; `tabs/mod.rs` decides when an entry may be shown instead of
/// fetching (`reuse`) and drops entries when their tab closes.
type FetchedCache = Arc<Mutex<FxHashMap<(String, String), FetchedKey>>>;

/// A search badge is active or the JSON path filter holds something: the signal to
/// keep paging through the rest of a hash without waiting for a scroll.
fn search_or_filter_active(window: &MainWindow) -> bool {
    window.global::<KeyPanel>().get_filter_badges().row_count() > 0 || !window.global::<KeyPanel>().get_json_filter().is_empty()
}

/// State shared across `init`'s callback closures and `load_key`, bundled
/// so `load_key` doesn't need one parameter per piece of it.
#[derive(Clone)]
struct KeyDetailShared {
    // what the panel shows, read and replaced together
    content: Arc<Mutex<PanelContent>>,
    // pretty renderings, built off the UI thread and announced through the
    // window's pretty-ready-* properties; request-pretty-value then serves
    // from here without computing
    pretty_cache: PrettyCache,
    // bumped whenever a table build is started or the view is switched off, so
    // a build that finishes late (a newer key, the switch turned off) is dropped
    table_generation: Arc<AtomicU64>,
    // bumped when a key starts loading and when the filter is applied, so work
    // started for the previous contents (a load, a filter, a pretty rendering)
    // is dropped when it finishes late
    view_epoch: Arc<AtomicU64>,
    fetched: FetchedCache,
    // completions of the filter bar's path, from the loaded fields
    suggester: Arc<Suggester>,
    // guards a single hash-page fetch — a scroll near the end of what's
    // loaded, or the next step of a search/filter's scan through the rest —
    // from overlapping another one already under way for the same key
    paging_in_flight: Arc<AtomicBool>,
    // keys made in the app, by (connection, key): an empty hash Redis doesn't hold
    // yet, shown as such until its first field is written
    new_keys: Arc<Mutex<FxHashSet<(String, String)>>>,
}

/// The key the panel shows and what of it is shown.
#[derive(Default)]
struct PanelContent {
    // the key whose fields these are: a background rendering that finishes after
    // the key changed must not land in the (new key's) cache
    key: String,
    // the key's fields as loaded; `current` is these after the filter bar's path
    // (the same list when there's none), and what everything else shows
    loaded: Fields,
    current: Fields,
    // the search bar's badges in effect, for highlighting matches; updated on load
    // and on every badge change, never from the box's live text
    search: SearchFilter,
    // the filter bar's path as `current` was last built with, and its match count
    path: AppliedPath,
}

impl PanelContent {
    /// The shown field named `name`.
    fn field(&self, name: &str) -> Option<Arc<field::Field>> {
        self.current.iter().find(|f| f.name == name).cloned()
    }
}

/// A JSON path filter in effect: its path (`None`: no filter) and how many matches
/// it found among the fields shown, for the status beside the filter bar.
#[derive(Default)]
struct AppliedPath {
    path: Option<JsonPath>,
    matches: usize,
}

/// What every key panel handler works with: the window, the app state, the
/// runtime and the shared state; cloned into each callback that needs them.
#[derive(Clone)]
struct Ctx {
    weak: Weak<MainWindow>,
    state: Arc<AppState>,
    rt: Handle,
    shared: KeyDetailShared,
}

impl Ctx {
    /// Builds the table view's data from the loaded fields, off the UI thread,
    /// and shows it if that's still wanted when it's ready, scrolled to `scroll`
    /// when there is one (a tab being loaded again).
    fn spawn_table_build(&self, scroll: Option<ScrollPos>) {
        let shared = &self.shared;
        let generation = shared.table_generation.fetch_add(1, Ordering::Relaxed) + 1;
        let latest = shared.table_generation.clone();
        // a snapshot of the pointers, so the lock isn't held while building
        let (fields, filter) = {
            let content = shared.content.lock();
            (content.current.clone(), content.search.clone())
        };
        let weak = self.weak.clone();
        let sort = self.weak.upgrade().and_then(|window| table_sort(&window));
        self.rt.spawn_blocking(move || {
            // the search bar's badges narrow the table the same way they narrow
            // the field/value list, and highlight the same way in its cells
            let needles = filter.highlight_needles();
            let fields = table_fields(&fields, &filter);
            let table = build_table(&fields, &needles, filter.case_sensitive, sort);
            let _ = weak.upgrade_in_event_loop(move |window| {
                if latest.load(Ordering::Relaxed) == generation && window.global::<KeyPanel>().get_view() == KeyView::Table {
                    if let Some(scroll) = scroll {
                        arm_table_scroll(&window, scroll);
                    }
                    apply_table(&window, table);
                }
            });
        });
    }

    /// Fills the table view from `current` if it is the view shown, otherwise empties
    /// it and drops any build under way; it is built when switched to, from whatever
    /// the filter leaves by then.
    fn show_view(&self, window: &MainWindow, scroll: Option<ScrollPos>) {
        match window.global::<KeyPanel>().get_view() {
            KeyView::Table => self.spawn_table_build(scroll),
            KeyView::Fields => {
                self.shared.table_generation.fetch_add(1, Ordering::Relaxed);
                clear_table(window);
            }
        }
    }
}

impl Ctx {
    /// The filter's completion list, from every loaded field's value (not what the
    /// filter has left of them); whether it has any.
    fn suggest(&self, text: &str, cursor: i32) -> bool {
        let Some(window) = self.weak.upgrade() else { return false };
        let fields: Fields = self.shared.content.lock().loaded.clone();
        let completion = self.shared.suggester.complete(&fields, text, cursor.max(0) as usize);
        let rows: Vec<SuggestionRow> = completion
            .iter()
            .flat_map(|completion| &completion.items)
            .map(|item| {
                let (start, end) = item.matched;
                SuggestionRow { before: item.key[..start].into(), matched: item.key[start..end].into(), after: item.key[end..].into() }
            })
            .collect();
        let any = !rows.is_empty();
        window.global::<KeyPanel>().set_suggestions(ModelRc::new(VecModel::from(rows)));
        any
    }

    /// The switcher picked another view (already stored in `key-view`).
    fn on_view_changed(&self) {
        if let Some(window) = self.weak.upgrade() {
            if window.global::<KeyPanel>().get_view() == KeyView::Fields {
                sync_rows_expansion(&window);
            }
            self.show_view(&window, None);
        }
    }

    /// The header's refresh button: fetches the shown key again. The filters, the open
    /// rows and the scroll position are all still in the window, read as a tab switch does.
    fn refresh_key(&self) {
        let Some(window) = self.weak.upgrade() else { return };
        let key = window.global::<KeyPanel>().get_key_id().to_string();
        if key.is_empty() {
            return;
        }
        self.load_key(LoadRequest { key, scroll: current_scroll(&window), reuse: false, announce: true });
    }

    /// A table header's triangle: orders the rows by that column, in the direction it asks for;
    /// a hash's Field column flips the hash's own order instead, which covers all its fields.
    fn sort_table(&self, column: i32, descending: bool) {
        let Some(window) = self.weak.upgrade() else { return };
        let panel = window.global::<KeyPanel>();
        if column == 0 && panel.get_badge() == "HASH" {
            panel.set_table_sort_column(-1);
            if descending != panel.get_sort_descending() {
                self.toggle_sort();
            } else {
                self.show_view(&window, None);
            }
            return;
        }
        panel.set_table_sort_column(column);
        panel.set_table_sort_descending(descending);
        self.show_view(&window, None);
    }

    /// The Field header's arrow: flips the order of the hash's fields and loads it
    /// again from the top, with every row folded.
    fn toggle_sort(&self) {
        let Some(window) = self.weak.upgrade() else { return };
        let panel = window.global::<KeyPanel>();
        panel.set_sort_descending(!panel.get_sort_descending());
        let key = panel.get_key_id().to_string();
        if key.is_empty() {
            return;
        }
        restore_expanded_fields(&window, &FxHashSet::default());
        self.load_key(LoadRequest { key, scroll: ScrollPos::default(), reuse: false, announce: false });
    }
}

impl KeyDetailShared {
    /// The filter's text once completion `index` of the list is taken, and where its
    /// cursor goes; the text unchanged (cursor -1) when there is no such completion.
    fn complete_suggestion(&self, text: slint::SharedString, cursor: i32, index: i32) -> CompletionEdit {
        let fields: Fields = self.content.lock().loaded.clone();
        let edit = self
            .suggester
            .complete(&fields, &text, cursor.max(0) as usize)
            .and_then(|completion| completion.accept(&text, index.max(0) as usize));
        match edit {
            Some(edit) => CompletionEdit { text: edit.text.into(), cursor: edit.cursor as i32 },
            None => CompletionEdit { text, cursor: -1 },
        }
    }
}

/// Puts key `id` in the panel's header, as its name reads; another key's table starts unsorted.
pub(crate) fn show_key_name(window: &MainWindow, id: &str) {
    let panel = window.global::<KeyPanel>();
    if panel.get_key_id() != id {
        panel.set_table_sort_column(-1);
    }
    panel.set_key_id(id.into());
    panel.set_name(KeyName::display_id(id).as_ref().into());
}

/// Empties the panel: no key and no rows are shown, and what a load still on its way brings is dropped.
fn show_nothing(window: &MainWindow, shared: &KeyDetailShared) {
    shared.view_epoch.fetch_add(1, Ordering::Relaxed);
    disarm_scroll(window);
    window.global::<KeyPanel>().set_badge("".into());
    window.global::<KeyPanel>().set_loaded(false);
    show_key_name(window, "");
    window.global::<KeyPanel>().set_meta("".into());
    window.global::<KeyPanel>().set_editable(false);
    window.global::<KeyPanel>().set_all_values_field_chars(8);
    window.global::<KeyPanel>().set_all_values_rows(ModelRc::new(VecModel::from(Vec::<AllValuesRowData>::new())));
    window.global::<KeyPanel>().set_json_filter_status("".into());
    {
        let mut content = shared.content.lock();
        content.loaded = Vec::new();
        content.current = Vec::new();
    }
    shared.suggester.forget();
    clear_table(window);
}

/// Wires the key panel's callbacks and returns the handles `tabs::init` drives
/// from tab and tree clicks.
pub fn init(window: &MainWindow, state: Arc<AppState>, rt: Handle) -> KeyDetail {
    let shared = KeyDetailShared {
        content: Arc::default(),
        pretty_cache: PrettyCache::default(),
        table_generation: Arc::new(AtomicU64::new(0)),
        view_epoch: Arc::new(AtomicU64::new(0)),
        fetched: Arc::new(Mutex::new(FxHashMap::default())),
        suggester: Arc::new(Suggester::default()),
        paging_in_flight: Arc::new(AtomicBool::new(false)),
        new_keys: Arc::default(),
    };
    let ctx = Ctx { weak: window.as_weak(), state, rt, shared };
    let panel = window.global::<KeyPanel>();
    // each handler gets its own handle on the shared state
    let with_ctx = |handler: fn(&Ctx)| {
        let ctx = ctx.clone();
        move || handler(&ctx)
    };

    panel.on_json_filter_submitted(with_ctx(Ctx::on_json_filter_submitted));
    // the search bar: Enter adds badges, a badge's × drops it, and flipping
    // case-sensitivity re-renders under the same badges
    panel.on_filter_query_submitted(with_ctx(Ctx::on_filter_query_submitted));
    panel.on_filter_badge_removed({
        let ctx = ctx.clone();
        move |text, scope| ctx.on_filter_badge_removed(&text, scope)
    });
    panel.on_filter_case_sensitivity_changed(with_ctx(Ctx::on_search_changed));
    // the field list scrolled near the first loaded field: the page before it is added
    panel.on_near_start(with_ctx(|ctx| ctx.fetch_page(PageDir::Previous)));
    // the key panel (either view) scrolled near the end of what's loaded: a hash not
    // fully loaded gets its next page (see `fetch_page` for chaining)
    panel.on_near_end(with_ctx(|ctx| ctx.fetch_page(PageDir::Next)));
    panel.on_suggest({
        let ctx = ctx.clone();
        move |text, cursor| ctx.suggest(&text, cursor)
    });
    panel.on_complete_suggestion({
        let shared = ctx.shared.clone();
        move |text, cursor, index| shared.complete_suggestion(text, cursor, index)
    });
    panel.on_view_changed(with_ctx(Ctx::on_view_changed));

    // a value-list row opened or folded: kept in KeyScroll.all-values-expanded,
    // for tabs/mod.rs to save when the tab is left and restore when it's shown again
    panel.on_field_expanded_changed({
        let weak = window.as_weak();
        move |field, expanded| {
            if let Some(window) = weak.upgrade() {
                set_field_expanded(&window, &field, expanded);
            }
        }
    });

    pretty::init(window, &ctx);
    export::init(window, &ctx);
    edit::init(window, &ctx);
    panel.on_load_value({
        let ctx = ctx.clone();
        move |field| ctx.load_full_value(&field)
    });

    panel.on_table_clear_selection({
        let weak = window.as_weak();
        move || {
            if let Some(window) = weak.upgrade() {
                reset_table_selection(&window, window.global::<KeyPanel>().get_table_rows().row_count());
            }
        }
    });
    panel.on_table_select_range({
        let weak = window.as_weak();
        move |from, to| {
            if let Some(window) = weak.upgrade() {
                select_table_range(&window, from, to);
            }
        }
    });
    panel.on_refresh_key(with_ctx(Ctx::refresh_key));
    panel.on_sort_toggled(with_ctx(Ctx::toggle_sort));
    panel.on_table_sort_requested({
        let ctx = ctx.clone();
        move |column, descending| ctx.sort_table(column, descending)
    });

    KeyDetail(ctx)
}

/// What `init` hands `tabs/mod.rs` and the query tabs: show a key or a query's rows,
/// and drop what was fetched for a connection's keys.
pub struct KeyDetail(Ctx);

impl KeyDetail {
    /// Shows a key, reusing what was fetched for it if `reuse` and there is any,
    /// then scrolls it to `scroll`.
    pub fn load(&self, key: String, scroll: ScrollPos, reuse: bool) {
        self.0.load_key(LoadRequest { key, scroll, reuse, announce: false });
    }

    /// Shows a query tab's result rows in the panel (named by the tab's id), or nothing for `None`.
    pub fn show_rows(&self, id: String, rows: Option<Vec<serde_json::Map<String, serde_json::Value>>>, scroll: ScrollPos) {
        let ctx = &self.0;
        let Some(window) = ctx.weak.upgrade() else { return };
        let Some(rows) = rows else {
            show_nothing(&window, &ctx.shared);
            return;
        };
        // the rows stand for the fields of a key named after the tab: the panel then shows,
        // filters and exports them like any key's, without a trip to the server
        let fields: Fields = rows
            .into_iter()
            .enumerate()
            .map(|(i, row)| Arc::new(field::Field::from_value((i + 1).to_string(), serde_json::Value::Object(row))))
            .collect();
        let slot = (window.connection_id().to_string(), id.clone());
        ctx.shared.fetched.lock().insert(
            slot,
            FetchedKey {
                badge: String::new(),
                meta: String::new(),
                fields,
                all_names: None,
                start: 0,
                field_count: 0,
                ttl: -1,
                names_cursor: None,
                descending: false,
                editable: false,
            },
        );
        ctx.load_key(LoadRequest { key: id, scroll, reuse: true, announce: false });
    }

    /// Drops what was fetched for a connection's key (`Some`), or all its keys (`None`).
    pub fn forget(&self, connection: &str, key: Option<&str>) {
        self.0.shared.fetched.lock().retain(|(c, k), _| c != connection || key.is_some_and(|key| key != k));
    }

    /// Shows `key` of `connection` as an empty hash while Redis doesn't hold it (a key
    /// just made in the app: Redis stores a hash once it has a field).
    pub fn mark_new(&self, connection: &str, key: &str) {
        self.0.shared.new_keys.lock().insert((connection.to_string(), key.to_string()));
    }

    /// Whether `key` of `connection` is a new hash Redis doesn't hold yet (see [`Self::mark_new`]).
    pub fn is_new(&self, connection: &str, key: &str) -> bool {
        self.0.shared.new_keys.lock().contains(&(connection.to_string(), key.to_string()))
    }

    /// The new hashes of `connection` picked by `gone` (deleted or renamed) are no longer shown as such.
    pub fn forget_new(&self, connection: &str, gone: impl Fn(&str) -> bool) {
        self.0.shared.new_keys.lock().retain(|(c, k)| c != connection || !gone(k));
    }
}
