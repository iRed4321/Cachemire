//! Paging through a hash: its next or previous page of values, its next batch of
//! names, and the scan through the rest while a search or the path filter is active.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use rustc_hash::FxHashSet;
use serde_json::Value;
use slint::{ComponentHandle, Model, ModelRc, VecModel};

use crate::{AllValuesRowData, KeyPanel, KeyScroll, MainWindow};
use cachemire_core::explorer::{self, HashValues};
use cachemire_core::key_name::KeyName;

use super::field::{Field, Fields, build_fields};
use super::scroll::{ScrollPos, current_expanded_fields, current_scroll};
use super::search_filter::SearchFilter;
use super::value_list::{build_all_values_table, filter_fields, filter_status};
use super::{Ctx, FetchedKey, HASH_PAGE_SIZE, KeyView, current_search_filter, search_or_filter_active};

/// The most rows a hash keeps loaded, and what their values may weigh: past either (by a page,
/// or a quarter), the rows farthest from the view are dropped back down to them (`trim`).
/// Measured: a row costs about 40 KB with a 2 KB JSON value, its row in the window included.
const KEPT_ROWS: usize = 1000;
const KEPT_BYTES: usize = 16 << 20;

/// The rows always kept on each side of those in sight, so the next scroll finds them.
const KEPT_AROUND: usize = HASH_PAGE_SIZE;

/// The rows a background tab's hash keeps on each side of those it showed (`shrink_idle`).
const IDLE_AROUND: usize = HASH_PAGE_SIZE / 2;

/// The height of a closed row and of what is shown, in each view (0 for one never shown), as the
/// views report them (see `KeyScroll`): the same for every tab, the panel being one.
#[derive(Clone, Copy)]
pub(crate) struct ViewSizes {
    list: (f32, f32),
    table: (f32, f32),
}

/// What the views report of their sizes now.
pub(crate) fn view_sizes(window: &MainWindow) -> ViewSizes {
    let scroll = window.global::<KeyScroll>();
    ViewSizes {
        list: (scroll.get_all_row_stride(), scroll.get_all_view_height()),
        table: (scroll.get_table_row_stride(), scroll.get_table_view_height()),
    }
}

/// The closed rows wholly above a view scrolled to `y` (none past an open one, taller) and the
/// first row below it, for rows `stride` high in a view `height` high.
fn rows_in_sight(y: f32, (stride, height): (f32, f32), first_open: usize) -> (usize, usize) {
    let above = ((-y / stride).floor().max(0.0) as usize).min(first_open);
    (above, ((-y + height) / stride).ceil().max(0.0) as usize)
}

/// Which side of the loaded fields a hash page is added to.
#[derive(Clone, Copy, PartialEq)]
pub(super) enum PageDir {
    Next,
    Previous,
}

/// The next page of a hash's field names after the loaded ones (`Next`) or the
/// page before them (`Previous`), if there is one.
fn hash_page(fetch: &FetchedKey, dir: PageDir) -> Option<Vec<String>> {
    let names = fetch.all_names.as_ref()?;
    let end = fetch.start + fetch.fields.len();
    match dir {
        PageDir::Next => (end < names.len()).then(|| names[end..(end + HASH_PAGE_SIZE).min(names.len())].to_vec()),
        PageDir::Previous => (fetch.start > 0).then(|| names[fetch.start.saturating_sub(HASH_PAGE_SIZE)..fetch.start].to_vec()),
    }
}

/// Whether the loaded fields of `fetch` include `field`, or it isn't a hash field
/// at all (so a different page wouldn't help); a name not read yet may still be one.
pub(super) fn holds_field(fetch: &FetchedKey, field: &str) -> bool {
    match fetch.all_names.as_ref().and_then(|names| names.iter().position(|n| n == field)) {
        Some(i) => (fetch.start..fetch.start + fetch.fields.len()).contains(&i),
        None => fetch.names_cursor.is_none(),
    }
}

/// Whether there's more of the hash after the loaded fields: a page of names already
/// read, or names still to read.
fn has_next(fetch: &FetchedKey) -> bool {
    hash_page(fetch, PageDir::Next).is_some() || fetch.names_cursor.is_some()
}

/// A page's fields that pass the applied path, and their rows as built under `search`.
struct BuiltPage {
    shown: Fields,
    matches: usize,
    search: SearchFilter,
    field_chars: i32,
    rows: Vec<AllValuesRowData>,
}

/// How many fields of `far_end` (the side to drop from, at most `most` of them) leave the `rows`
/// loaded, weighing `bytes`, within the budget.
fn over_budget<'a>(mut far_end: impl Iterator<Item = &'a Arc<Field>>, most: usize, mut rows: usize, mut bytes: usize) -> usize {
    let mut dropped = 0;
    while dropped < most && (rows > KEPT_ROWS || bytes > KEPT_BYTES) {
        let Some(field) = far_end.next() else { break };
        rows -= 1;
        bytes -= field.raw.len();
        dropped += 1;
    }
    dropped
}

impl Ctx {
    /// A hash page just landed: merges it into the reuse cache and, if the contents it
    /// was read for (`epoch`) are still shown, into `loaded`, then adds its rows (under
    /// the applied path and the search bar) before or after those shown, off the UI thread.
    fn on_hash_page_loaded(&self, connection: &str, key: &str, epoch: u64, dir: PageDir, new_fields: Fields) {
        let shared = &self.shared;
        let Some(window) = self.weak.upgrade().filter(|_| shared.shows(epoch)) else {
            self.page_done();
            return;
        };
        if let Some(fetch) = shared.fetched.lock().get_mut(&(connection.to_string(), key.to_string())) {
            match dir {
                PageDir::Next => fetch.fields.extend(new_fields.iter().cloned()),
                PageDir::Previous => {
                    fetch.start = fetch.start.saturating_sub(new_fields.len());
                    fetch.fields.splice(0..0, new_fields.iter().cloned());
                }
            }
        }
        let path = {
            let mut content = shared.content.lock();
            match dir {
                PageDir::Next => content.loaded.extend(new_fields.iter().cloned()),
                PageDir::Previous => {
                    content.loaded.splice(0..0, new_fields.iter().cloned());
                }
            }
            content.path.path.clone()
        };
        let search = current_search_filter(&window);
        let expanded = current_expanded_fields(&window);
        let ctx = self.clone();
        self.rt.spawn_blocking(move || {
            let (shown, matches) = filter_fields(&new_fields, path.as_ref());
            let (field_chars, rows) = build_all_values_table(&shown, &search, &expanded);
            let _ = slint::invoke_from_event_loop(move || {
                ctx.show_page(dir, epoch, BuiltPage { shown, matches, search, field_chars, rows });
                ctx.page_done();
            });
        });
    }

    /// Puts a page's filtered fields and rows before or after those shown, unless a
    /// newer load or filter has replaced them (it was built from `loaded`, page included).
    fn show_page(&self, dir: PageDir, epoch: u64, page: BuiltPage) {
        let BuiltPage { shown, matches, search, field_chars, rows } = page;
        let shared = &self.shared;
        let Some(window) = self.weak.upgrade() else { return };
        let status = {
            let mut content = shared.content.lock();
            if content.epoch != epoch {
                return;
            }
            match dir {
                PageDir::Next => content.current.extend(shown),
                PageDir::Previous => {
                    content.current.splice(0..0, shown);
                }
            }
            content.path.matches += matches;
            filter_status(content.path.path.as_ref(), content.path.matches, content.current.len())
        };
        let panel = window.global::<KeyPanel>();
        panel.set_json_filter_status(status.into());
        // badges changed while the rows were built: every row is built again under them
        if current_search_filter(&window) != search {
            self.on_search_changed();
            return;
        }
        let added = rows.len();
        panel.set_all_values_field_chars(panel.get_all_values_field_chars().max(field_chars));
        let model = panel.get_all_values_rows();
        match (dir, model.as_any().downcast_ref::<VecModel<AllValuesRowData>>()) {
            // added to the model in place: the rows already made stay as they are
            (PageDir::Next, Some(vec)) => vec.extend(rows),
            _ => {
                // rows added above the view: the list keeps what was in sight where it was
                if dir == PageDir::Previous && panel.get_view() == KeyView::Fields {
                    window.global::<crate::KeyScroll>().set_prepended(rows.len() as i32);
                }
                let open = current_expanded_fields(&window);
                let before = model.iter().map(|mut row| {
                    row.initial_expanded = open.contains(row.field.as_str());
                    row
                });
                let all: Vec<AllValuesRowData> = match dir {
                    PageDir::Next => before.chain(rows).collect(),
                    PageDir::Previous => rows.into_iter().chain(before).collect(),
                };
                panel.set_all_values_rows(ModelRc::new(VecModel::from(all)));
            }
        }
        let filtered = search_or_filter_active(&window);
        let dropped_above = if filtered { 0 } else { self.trim(&window, dir) };
        // the table keeps what was in sight: rows added above push it down, dropped ones pull it up
        let shifted = if dir == PageDir::Previous && !filtered { added } else { 0 } as f32 - dropped_above as f32;
        let scroll = (panel.get_view() == KeyView::Table && shifted != 0.0).then(|| {
            let mut pos = current_scroll(&window);
            pos.table_y = (pos.table_y - shifted * window.global::<KeyScroll>().get_table_row_stride()).min(0.0);
            pos
        });
        self.show_view(&window, scroll);
    }

    /// Past the budget (`KEPT_ROWS`, `KEPT_BYTES`), drops the loaded rows farthest from the view
    /// on the side away from the page that just `landed`, keeping `KEPT_AROUND` rows past those in
    /// sight and every open row; scrolling back fetches them again. Returns the rows dropped above.
    fn trim(&self, window: &MainWindow, landed: PageDir) -> usize {
        let (panel, scroll) = (window.global::<KeyPanel>(), window.global::<KeyScroll>());
        let table = panel.get_view() == KeyView::Table;
        let jumping = scroll.get_reveal_pending() || scroll.get_restore_all_pending() || scroll.get_restore_table_pending();
        // a table sorted or ticked here holds its rows in another order, or by their place
        if jumping || table && (panel.get_table_selected_count() > 0 || panel.get_table_sort_column() >= 0) {
            return 0;
        }
        let (stride, height, y) = match table {
            true => (scroll.get_table_row_stride(), scroll.get_table_view_height(), scroll.get_table_y()),
            false => (scroll.get_all_row_stride(), scroll.get_all_view_height(), scroll.get_all_y()),
        };
        let model = panel.get_all_values_rows();
        let open = current_expanded_fields(window);
        let mut content = self.shared.content.lock();
        let n = content.loaded.len();
        if stride <= 0.0 || !content.is_hash() || content.current.len() != n || model.row_count() != n {
            return 0;
        }
        let bytes: usize = content.loaded.iter().map(|field| field.raw.len()).sum();
        if n <= KEPT_ROWS + HASH_PAGE_SIZE && bytes <= KEPT_BYTES + KEPT_BYTES / 4 {
            return 0;
        }
        // the closed rows wholly above the view (an open one is taller: none is dropped past it),
        // and the first row below it; open rows only matter in the list view
        let is_open = |field: &Arc<Field>| !table && open.contains(&field.name);
        let first_open = content.loaded.iter().position(is_open).unwrap_or(n);
        let last_open = content.loaded.iter().rposition(is_open);
        let (above, below) = rows_in_sight(y, (stride, height), first_open);
        let (front, back) = match landed {
            PageDir::Next => (over_budget(content.loaded.iter(), above.saturating_sub(KEPT_AROUND), n, bytes), 0),
            PageDir::Previous => {
                let kept_to = (below + KEPT_AROUND).max(last_open.map_or(0, |i| i + 1));
                (0, over_budget(content.loaded.iter().rev(), n.saturating_sub(kept_to), n, bytes))
            }
        };
        if front + back == 0 {
            return 0;
        }
        let gone: FxHashSet<String> =
            content.loaded[..front].iter().chain(&content.loaded[n - back..]).map(|field| field.name.clone()).collect();
        let shown = &mut *content;
        for fields in [&mut shown.loaded, &mut shown.current] {
            fields.truncate(n - back);
            fields.drain(..front);
        }
        let slot = (window.connection_id().to_string(), shown.key.clone());
        drop(content);
        if let Some(fetch) = self.shared.fetched.lock().get_mut(&slot) {
            fetch.fields.truncate(fetch.fields.len().saturating_sub(back));
            fetch.fields.drain(..front.min(fetch.fields.len()));
            fetch.start += front;
        }
        self.shared.pretty_cache.lock().forget(&gone);
        match (front, model.as_any().downcast_ref::<VecModel<AllValuesRowData>>()) {
            // only rows below the view: taken off the end in place
            (0, Some(vec)) => (0..back).for_each(|_| _ = vec.remove(vec.row_count() - 1)),
            _ => {
                if !table {
                    scroll.set_prepended(-(front as i32));
                }
                let kept: Vec<AllValuesRowData> = model.iter().skip(front).take(n - front - back).collect();
                panel.set_all_values_rows(ModelRc::new(VecModel::from(kept)));
            }
        }
        tracing::debug!(front, back, kept = n - front - back, "dropped loaded rows far from the view");
        front
    }

    /// A background tab's hash, shown last in `view` scrolled to `scroll`: keeps what was fetched
    /// of it to the rows in sight then and `IDLE_AROUND` on each side (and every `open` row), and
    /// moves `scroll` by the rows dropped above. Coming back fetches the others again as needed.
    pub(super) fn shrink_idle(
        &self,
        slot: (String, String),
        view: KeyView,
        scroll: &mut ScrollPos,
        open: &FxHashSet<String>,
        sizes: ViewSizes,
    ) {
        let mut fetched = self.shared.fetched.lock();
        let Some(fetch) = fetched.get_mut(&slot).filter(|fetch| fetch.all_names.is_some()) else { return };
        let n = fetch.fields.len();
        let (sizes_shown, y) = match view {
            KeyView::Table => (sizes.table, scroll.table_y),
            KeyView::Fields => (sizes.list, scroll.all_y),
        };
        if n <= 2 * HASH_PAGE_SIZE || sizes_shown.0 <= 0.0 {
            return;
        }
        let first_open = fetch.fields.iter().position(|field| open.contains(&field.name)).unwrap_or(n);
        let last_open = fetch.fields.iter().rposition(|field| open.contains(&field.name));
        let (above, below) = rows_in_sight(y, sizes_shown, if view == KeyView::Table { n } else { first_open });
        let front = above.saturating_sub(IDLE_AROUND).min(first_open);
        let kept_to = (below + IDLE_AROUND).max(last_open.map_or(0, |i| i + 1)).min(n);
        if front == 0 && kept_to == n {
            return;
        }
        fetch.fields.truncate(kept_to);
        fetch.fields.drain(..front);
        fetch.start += front;
        // each view's place among the rows left: the table's rows are all one height; the list
        // moves only if every row dropped was closed and above it, else starts from the top
        let shift = front as f32;
        scroll.table_y = (scroll.table_y + shift * sizes.table.0).min(0.0);
        let (list_above, _) = rows_in_sight(scroll.all_y, sizes.list, first_open);
        scroll.all_y = if sizes.list.0 > 0.0 && front <= list_above { (scroll.all_y + shift * sizes.list.0).min(0.0) } else { 0.0 };
        tracing::info!(key = %slot.1, dropped = n - fetch.fields.len(), kept = fetch.fields.len(), "shrank an idle tab's hash");
    }

    /// A page fetch is over (shown, dropped or failed): the next one may start, and does
    /// at once while a search or the path filter is reading through the rest of the hash.
    fn page_done(&self) {
        self.shared.paging_in_flight.store(false, Ordering::Release);
        self.kick_off_scan_if_active();
    }

    /// A page fetch failed: the next one waits for a scroll, unless what it was for
    /// was replaced since, whose own pages may start at once.
    fn page_failed(&self, epoch: u64) {
        if self.shared.shows(epoch) {
            self.shared.paging_in_flight.store(false, Ordering::Release);
        } else {
            self.page_done();
        }
    }

    /// Fetches the next or previous page of the shown hash's field values unless one is
    /// already being fetched. While a search badge or the JSON path filter is active,
    /// each landed page chains on (`fetch_more`) until the whole hash is read.
    pub(super) fn fetch_page(&self, dir: PageDir) {
        let shared = &self.shared;
        if shared.paging_in_flight.swap(true, Ordering::AcqRel) {
            return;
        }
        let Some(window) = self.weak.upgrade() else {
            shared.paging_in_flight.store(false, Ordering::Release);
            return;
        };
        let connection = window.connection_id().to_string();
        let (key, epoch) = {
            let content = shared.content.lock();
            (content.key.clone(), content.epoch)
        };
        let (names, cursor) = match shared.fetched.lock().get(&(connection.clone(), key.clone())) {
            Some(fetch) => (hash_page(fetch, dir), fetch.names_cursor.filter(|_| dir == PageDir::Next)),
            None => (None, None),
        };
        let Some(names) = names else {
            shared.paging_in_flight.store(false, Ordering::Release);
            // past the names read so far: the next batch of them is read first
            if let Some(cursor) = cursor {
                self.fetch_more_names(connection, key, cursor, epoch);
            }
            return;
        };
        let (state, asked, name) = (self.state.clone(), connection.clone(), KeyName::from_id(&key));
        let page = async move {
            let HashValues { values, oversized, binary } = explorer::hash_field_page(&state, Some(&asked), &name, &names).await?;
            Ok(tokio::task::spawn_blocking(move || build_fields(Value::Object(values), oversized, &binary, &names)).await?)
        };
        let ctx = self.clone();
        crate::app::spawn(&self.rt, page, move |_, _, page| match page.flatten() {
            Ok(fields) => ctx.on_hash_page_loaded(&connection, &key, epoch, dir, fields),
            // not retried right away: the next scroll near the end asks again
            Err(_) => ctx.page_failed(epoch),
        });
    }

    /// Reads the shown hash's next batch of field names from `cursor`, adds them after
    /// those read so far, then fetches the next page of values as `fetch_page` would.
    fn fetch_more_names(&self, connection: String, key: String, cursor: u64, epoch: u64) {
        if self.shared.paging_in_flight.swap(true, Ordering::AcqRel) {
            return;
        }
        let descending = self.weak.upgrade().is_some_and(|w| w.global::<KeyPanel>().get_sort_descending());
        let (state, asked, name) = (self.state.clone(), connection.clone(), KeyName::from_id(&key));
        let more = async move { explorer::hash_more_names(&state, Some(&asked), &name, cursor, descending).await };
        let ctx = self.clone();
        crate::app::spawn(&self.rt, more, move |_, window, result| {
            let shared = &ctx.shared;
            let Ok((batch, next)) = result.flatten() else { return ctx.page_failed(epoch) };
            if !shared.shows(epoch) {
                return ctx.page_done();
            }
            shared.paging_in_flight.store(false, Ordering::Release);
            let meta = {
                let mut fetched = shared.fetched.lock();
                let Some(fetch) = fetched.get_mut(&(connection, key)) else { return };
                let names = Arc::make_mut(fetch.all_names.get_or_insert_with(Arc::default));
                // a scan may repeat a name it already gave
                let known: FxHashSet<&String> = names.iter().collect();
                let fresh: Vec<String> = batch.into_iter().filter(|name| !known.contains(name)).collect();
                names.extend(fresh);
                fetch.names_cursor = next;
                fetch.meta = fetch.hash_meta();
                fetch.meta.clone()
            };
            window.global::<KeyPanel>().set_meta(meta.into());
            ctx.fetch_page(PageDir::Next);
        });
    }

    /// The next page of the shown hash, else (once its end is reached) the page before
    /// the loaded fields.
    fn fetch_more(&self) {
        let Some(window) = self.weak.upgrade() else { return };
        let key = (window.connection_id().to_string(), self.shared.content.lock().key.clone());
        let dir = match self.shared.fetched.lock().get(&key) {
            Some(fetch) if !has_next(fetch) => PageDir::Previous,
            _ => PageDir::Next,
        };
        self.fetch_page(dir);
    }

    /// Starts or continues fetching the rest of the shown hash when a search badge or
    /// the JSON path filter is active; called after either changes. A no-op with
    /// nothing active or nothing left to fetch.
    pub(super) fn kick_off_scan_if_active(&self) {
        if let Some(window) = self.weak.upgrade()
            && search_or_filter_active(&window)
        {
            self.fetch_more();
        }
    }
}

#[cfg(test)]
mod tests;
