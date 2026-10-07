//! Bridges `backend::explorer::get_key_details` to the key panel. Split by
//! concern across submodules ([`field`], [`pretty_text`]/[`highlight`],
//! [`table`]/[`value_list`], [`export`], [`scroll`]); this wires them together.

use rustc_hash::{FxHashMap, FxHashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use rayon::prelude::*;
use slint::{ComponentHandle, Model, ModelRc, VecModel, Weak};
use tokio::runtime::Handle;

use crate::backend::LockExt;
use crate::backend::key_name::KeyName;
use crate::backend::redis_value_type::RedisValueType;
use crate::backend::state::AppState;
use crate::json_path::JsonPath;
use crate::{AllValuesRowData, CompletionEdit, KeyPanel, KeyView, MainWindow, SuggestionRow};

mod color;
mod export;
mod field;
mod filtering;
#[cfg(test)]
mod headless_ui_tests;
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
mod value_list;
mod word;

pub use scroll::{ScrollPos, current_expanded_fields, current_scroll, restore_expanded_fields};
pub use table::clear_table;
pub(crate) use highlight::{search_plain_text, search_value_text};

use field::{Fields, PrettyCache};
use load::LoadRequest;
use paging::PageDir;
use search_filter::{SearchFilter, SearchTerm};
use suggest::Suggester;
use scroll::{arm_table_scroll, disarm_scroll, set_field_expanded, sync_rows_expansion};
use table::{apply_table, build_table, reset_table_selection, select_table_range};
use value_list::meta_text;

const MAX_ITEMS: i64 = 500;
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
    let terms = window.global::<KeyPanel>().get_filter_badges().iter().map(|b| SearchTerm { text: b.text.to_string(), scope: b.scope }).collect();
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
}

impl FetchedKey {
    /// The header's size and TTL for a hash, as many names as are read so far.
    fn hash_meta(&self) -> String {
        let loaded = self.names_cursor.and(self.all_names.as_ref()).map(Vec::len);
        meta_text(&RedisValueType::Hash, self.ttl, self.field_count, false, loaded)
    }
}

/// Fetched keys by (saved-connection id, key name). Filled by every
/// successful fetch; `tabs.rs` decides when an entry may be shown instead of
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
    current: Arc<Mutex<Fields>>,
    // pretty renderings, built off the UI thread and announced through the
    // window's pretty-ready-* properties; request-pretty-value then serves
    // from here without computing
    pretty_cache: PrettyCache,
    // the key whose fields `current` holds — a background rendering that
    // finishes after the key changed must not land in the (new key's) cache
    current_key: Arc<Mutex<String>>,
    // the search bar's badges in effect, for highlighting matches in previews
    // and expanded values; updated on load and whenever a badge is added,
    // removed, or case-sensitivity is flipped — never from the box's live text
    applied_search: Arc<Mutex<SearchFilter>>,
    // bumped whenever a table build is started or the view is switched off, so
    // a build that finishes late (a newer key, the switch turned off) is dropped
    table_generation: Arc<AtomicU64>,
    // The key's fields as loaded; `current` is these after the filter bar's
    // path (the same list when there's none), and what everything else shows.
    loaded: Arc<Mutex<Fields>>,
    // bumped when a key starts loading and when the filter is applied, so work
    // started for the previous contents (a load, a filter, a pretty rendering)
    // is dropped when it finishes late
    view_epoch: Arc<AtomicU64>,
    fetched: FetchedCache,
    // completions of the filter bar's path, from `loaded`
    suggester: Arc<Suggester>,
    // the filter bar's path as `current` was last built with, and its match count
    applied_path: Arc<Mutex<AppliedPath>>,
    // guards a single hash-page fetch — a scroll near the end of what's
    // loaded, or the next step of a search/filter's scan through the rest —
    // from overlapping another one already under way for the same key
    paging_in_flight: Arc<AtomicBool>,
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
        let fields: Fields = shared.current.lock_recover().clone();
        let filter = shared.applied_search.lock_recover().clone();
        let weak = self.weak.clone();
        self.rt.spawn_blocking(move || {
            // the search bar's badges narrow the table the same way they narrow
            // the field/value list, and highlight the same way in its cells
            let needles = filter.highlight_needles();
            let fields: Fields = fields.into_par_iter().filter(|f| filter.matches(f)).collect();
            let table = build_table(&fields, &needles, filter.case_sensitive);
            let _ = slint::invoke_from_event_loop(move || {
                let Some(window) = weak.upgrade() else { return };
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

/// Puts key `id` in the panel's header, as its name reads.
pub(crate) fn show_key_name(window: &MainWindow, id: &str) {
    let panel = window.global::<KeyPanel>();
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
    window.global::<KeyPanel>().set_all_values_field_chars(8);
    window.global::<KeyPanel>().set_all_values_rows(ModelRc::new(VecModel::from(Vec::<AllValuesRowData>::new())));
    window.global::<KeyPanel>().set_json_filter_status("".into());
    *shared.loaded.lock_recover() = Vec::new();
    shared.suggester.forget();
    *shared.current.lock_recover() = Vec::new();
    clear_table(window);
}

/// Wires the key panel's callbacks and returns the handles `tabs::init` drives
/// from tab and tree clicks.
pub fn init(window: &MainWindow, state: Arc<AppState>, rt: Handle) -> KeyDetail {
    let shared = KeyDetailShared {
        current: Arc::new(Mutex::new(Vec::new())),
        pretty_cache: Arc::new(Mutex::new(FxHashMap::default())),
        current_key: Arc::new(Mutex::new(String::new())),
        applied_search: Arc::new(Mutex::new(SearchFilter::default())),
        table_generation: Arc::new(AtomicU64::new(0)),
        loaded: Arc::new(Mutex::new(Vec::new())),
        view_epoch: Arc::new(AtomicU64::new(0)),
        fetched: Arc::new(Mutex::new(FxHashMap::default())),
        suggester: Arc::new(Suggester::default()),
        applied_path: Arc::new(Mutex::new(AppliedPath::default())),
        paging_in_flight: Arc::new(AtomicBool::new(false)),
    };
    let ctx = Ctx { weak: window.as_weak(), state, rt, shared };
    let panel = window.global::<KeyPanel>();

    panel.on_json_filter_submitted({
        let ctx = ctx.clone();
        move || ctx.on_json_filter_submitted()
    });
    // the search bar: Enter adds badges, a badge's × drops it, and flipping
    // case-sensitivity re-renders under the same badges
    panel.on_filter_query_submitted({
        let ctx = ctx.clone();
        move || ctx.on_filter_query_submitted()
    });
    panel.on_filter_badge_removed({
        let ctx = ctx.clone();
        move |text, scope| ctx.on_filter_badge_removed(&text, scope)
    });
    panel.on_filter_case_sensitivity_changed({
        let ctx = ctx.clone();
        move || ctx.on_search_changed()
    });
    // the field list scrolled near the first loaded field: the page before it is added
    panel.on_near_start({
        let ctx = ctx.clone();
        move || ctx.fetch_page(PageDir::Previous)
    });
    // the key panel (either view) scrolled near the end of what's loaded: a hash not
    // fully loaded gets its next page (see `fetch_page` for chaining)
    panel.on_near_end({
        let ctx = ctx.clone();
        move || ctx.fetch_page(PageDir::Next)
    });

    // the filter's completion list, from every loaded field's value (not what
    // the filter has left of them)
    panel.on_suggest({
        let ctx = ctx.clone();
        move |text, cursor| {
            let Some(window) = ctx.weak.upgrade() else { return false };
            let fields: Fields = ctx.shared.loaded.lock_recover().clone();
            let completion = ctx.shared.suggester.complete(&fields, &text, cursor.max(0) as usize);
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
    });

    panel.on_complete_suggestion({
        let shared = ctx.shared.clone();
        move |text, cursor, index| {
            let fields: Fields = shared.loaded.lock_recover().clone();
            let edit = shared
                .suggester
                .complete(&fields, &text, cursor.max(0) as usize)
                .and_then(|completion| completion.accept(&text, index.max(0) as usize));
            match edit {
                Some(edit) => CompletionEdit { text: edit.text.into(), cursor: edit.cursor as i32 },
                None => CompletionEdit { text, cursor: -1 },
            }
        }
    });

    // the switcher picked another view (already stored in `key-view`)
    panel.on_view_changed({
        let ctx = ctx.clone();
        move || {
            if let Some(window) = ctx.weak.upgrade() {
                if window.global::<KeyPanel>().get_view() == KeyView::Fields {
                    sync_rows_expansion(&window);
                }
                ctx.show_view(&window, None);
            }
        }
    });

    // a value-list row opened or folded: kept in KeyScroll.all-values-expanded,
    // for tabs.rs to save when the tab is left and restore when it's shown again
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

    // header refresh button: fetches the shown key again. The filters, the open
    // rows and the scroll position are all still in the window, so this reads
    // them the way a tab switch restores them.
    panel.on_refresh_key({
        let ctx = ctx.clone();
        move || {
            let Some(window) = ctx.weak.upgrade() else { return };
            let key = window.global::<KeyPanel>().get_key_id().to_string();
            if key.is_empty() {
                return;
            }
            ctx.load_key(LoadRequest { key, scroll: current_scroll(&window), reuse: false, announce: true });
        }
    });

    // the Field header's arrow: flips the order of the hash's fields and loads it again
    // from the top, with every row folded
    panel.on_sort_toggled({
        let ctx = ctx.clone();
        move || {
            let Some(window) = ctx.weak.upgrade() else { return };
            let panel = window.global::<KeyPanel>();
            panel.set_sort_descending(!panel.get_sort_descending());
            let key = panel.get_key_id().to_string();
            if key.is_empty() {
                return;
            }
            restore_expanded_fields(&window, &FxHashSet::default());
            ctx.load_key(LoadRequest { key, scroll: ScrollPos::default(), reuse: false, announce: false });
        }
    });

    KeyDetail(ctx)
}

/// What `init` hands `tabs.rs` and the query tabs: show a key or a query's rows,
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
        let fields: Fields = rows.into_iter().enumerate().map(|(i, row)| Arc::new(field::Field::from_value((i + 1).to_string(), serde_json::Value::Object(row)))).collect();
        let slot = (window.connection_id().to_string(), id.clone());
        ctx.shared.fetched.lock_recover().insert(slot, FetchedKey { badge: String::new(), meta: String::new(), fields, all_names: None, start: 0, field_count: 0, ttl: -1, names_cursor: None, descending: false });
        ctx.load_key(LoadRequest { key: id, scroll, reuse: true, announce: false });
    }

    /// Drops what was fetched for a connection's key (`Some`), or all its keys (`None`).
    pub fn forget(&self, connection: &str, key: Option<&str>) {
        self.0.shared.fetched.lock_recover().retain(|(c, k), _| c != connection || key.is_some_and(|key| key != k));
    }
}
