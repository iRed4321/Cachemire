//! Paging through a hash: its next or previous page of values, its next batch of
//! names, and the scan through the rest while a search or the path filter is active.

use std::sync::atomic::Ordering;

use rustc_hash::FxHashSet;
use slint::{ComponentHandle, Model, ModelRc, VecModel};

use crate::backend::LockExt;
use crate::backend::explorer;
use crate::backend::key_name::KeyName;
use crate::{AllValuesRowData, KeyPanel};

use super::field::{Fields, build_fields};
use super::scroll::current_expanded_fields;
use super::search_filter::SearchFilter;
use super::value_list::{build_all_values_table, filter_fields, filter_status};
use super::{Ctx, FetchedKey, HASH_PAGE_SIZE, KeyView, current_search_filter, search_or_filter_active};

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

impl Ctx {
    /// A hash page just landed: merges it into the reuse cache and, if `key` is still
    /// shown and unreloaded since (`epoch`), into `loaded`, then adds its rows (under the
    /// applied path and the search bar) before or after those shown, off the UI thread.
    fn on_hash_page_loaded(&self, connection: &str, key: &str, epoch: u64, dir: PageDir, new_fields: Fields) {
        let shared = &self.shared;
        let Some(window) = self.weak.upgrade().filter(|_| shared.view_epoch.load(Ordering::Relaxed) == epoch) else {
            self.page_done();
            return;
        };
        if let Some(fetch) = shared.fetched.lock_recover().get_mut(&(connection.to_string(), key.to_string())) {
            match dir {
                PageDir::Next => fetch.fields.extend(new_fields.iter().cloned()),
                PageDir::Previous => {
                    fetch.start = fetch.start.saturating_sub(new_fields.len());
                    fetch.fields.splice(0..0, new_fields.iter().cloned());
                }
            }
        }
        match dir {
            PageDir::Next => shared.loaded.lock_recover().extend(new_fields.iter().cloned()),
            PageDir::Previous => {
                shared.loaded.lock_recover().splice(0..0, new_fields.iter().cloned());
            }
        }
        let path = shared.applied_path.lock_recover().path.clone();
        let search = current_search_filter(&window);
        let expanded = current_expanded_fields(&window);
        let ctx = self.clone();
        self.rt.spawn_blocking(move || {
            let (shown, matches) = filter_fields(&new_fields, path.as_ref());
            let (field_chars, rows) = build_all_values_table(&shown, &search, &expanded);
            let _ = slint::invoke_from_event_loop(move || {
                ctx.show_page(dir, epoch, shown, matches, &search, field_chars, rows);
                ctx.page_done();
            });
        });
    }

    /// Puts a page's filtered fields and rows before or after those shown, unless a
    /// newer load or filter has replaced them (it was built from `loaded`, page included).
    #[allow(clippy::too_many_arguments)]
    fn show_page(&self, dir: PageDir, epoch: u64, shown: Fields, matches: usize, search: &SearchFilter, field_chars: i32, rows: Vec<AllValuesRowData>) {
        let shared = &self.shared;
        let Some(window) = self.weak.upgrade() else { return };
        if shared.view_epoch.load(Ordering::Relaxed) != epoch {
            return;
        }
        let total = {
            let mut current = shared.current.lock_recover();
            match dir {
                PageDir::Next => current.extend(shown),
                PageDir::Previous => {
                    current.splice(0..0, shown);
                }
            }
            current.len()
        };
        let status = {
            let mut applied = shared.applied_path.lock_recover();
            applied.matches += matches;
            filter_status(applied.path.as_ref(), applied.matches, total)
        };
        let panel = window.global::<KeyPanel>();
        panel.set_json_filter_status(status.into());
        // badges changed while the rows were built: every row is built again under them
        if current_search_filter(&window) != *search {
            self.on_search_changed();
            return;
        }
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
        self.show_view(&window, None);
    }

    /// A page fetch is over (shown, dropped or failed): the next one may start, and does
    /// at once while a search or the path filter is reading through the rest of the hash.
    fn page_done(&self) {
        self.shared.paging_in_flight.store(false, Ordering::Release);
        self.kick_off_scan_if_active();
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
        let key = window.global::<KeyPanel>().get_key_id().to_string();
        let epoch = shared.view_epoch.load(Ordering::Relaxed);
        let (names, cursor) = match shared.fetched.lock_recover().get(&(connection.clone(), key.clone())) {
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
        let ctx = self.clone();
        self.rt.spawn(async move {
            let result = explorer::hash_field_page(&ctx.state, None, &KeyName::from_id(&key), &names).await;
            let _ = slint::invoke_from_event_loop(move || match result {
                Ok(value) => ctx.on_hash_page_loaded(&connection, &key, epoch, dir, build_fields(value)),
                // not retried right away: the next scroll near the end asks again
                Err(_) => ctx.shared.paging_in_flight.store(false, Ordering::Release),
            });
        });
    }

    /// Reads the shown hash's next batch of field names from `cursor`, adds them after
    /// those read so far, then fetches the next page of values as `fetch_page` would.
    fn fetch_more_names(&self, connection: String, key: String, cursor: u64, epoch: u64) {
        if self.shared.paging_in_flight.swap(true, Ordering::AcqRel) {
            return;
        }
        let descending = self.weak.upgrade().is_some_and(|w| w.global::<KeyPanel>().get_sort_descending());
        let ctx = self.clone();
        self.rt.spawn(async move {
            let result = explorer::hash_more_names(&ctx.state, None, &KeyName::from_id(&key), cursor, descending).await;
            let _ = slint::invoke_from_event_loop(move || {
                let shared = &ctx.shared;
                shared.paging_in_flight.store(false, Ordering::Release);
                let Ok((batch, next)) = result else { return };
                if shared.view_epoch.load(Ordering::Relaxed) != epoch {
                    return;
                }
                let meta = {
                    let mut fetched = shared.fetched.lock_recover();
                    let Some(fetch) = fetched.get_mut(&(connection, key)) else { return };
                    let names = fetch.all_names.get_or_insert_with(Vec::new);
                    // a scan may repeat a name it already gave
                    let known: FxHashSet<&String> = names.iter().collect();
                    let fresh: Vec<String> = batch.into_iter().filter(|name| !known.contains(name)).collect();
                    names.extend(fresh);
                    fetch.names_cursor = next;
                    fetch.meta = fetch.hash_meta();
                    fetch.meta.clone()
                };
                if let Some(window) = ctx.weak.upgrade() {
                    window.global::<KeyPanel>().set_meta(meta.into());
                }
                ctx.fetch_page(PageDir::Next);
            });
        });
    }

    /// The next page of the shown hash, else (once its end is reached) the page before
    /// the loaded fields.
    fn fetch_more(&self) {
        let key = self.weak.upgrade().map(|w| (w.connection_id().to_string(), w.global::<KeyPanel>().get_key_id().to_string()));
        let Some(key) = key else { return };
        let dir = match self.shared.fetched.lock_recover().get(&key) {
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
