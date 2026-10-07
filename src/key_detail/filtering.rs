//! The search bar (badges, case-sensitivity) and the filter bar's JSON path:
//! each re-renders what's loaded under them, off the UI thread.

use std::sync::atomic::Ordering;

use rust_i18n::t;
use rustc_hash::FxHashSet;
use slint::{ComponentHandle, Model, ModelRc, VecModel};

use crate::backend::LockExt;
use crate::{BadgeData, KeyPanel, SearchScope};

use super::field::Fields;
use super::scroll::{current_expanded_fields, disarm_scroll};
use super::search_filter::tokenize;
use super::value_list::{build_all_values_table, filter_fields, filter_status, parse_filter};
use super::{AppliedPath, Ctx, MAX_BADGES, current_search_filter};

impl Ctx {
    /// Re-renders the value list from the loaded `current` fields under the search
    /// bar's badges and case-sensitivity, without refetching; runs off the UI thread
    /// like `load_key`. Called when a badge is added or removed or the case toggle flips.
    pub(super) fn on_search_changed(&self) {
        let Some(window) = self.weak.upgrade() else { return };
        let filter = current_search_filter(&window);
        *self.shared.applied_search.lock_recover() = filter.clone();
        // a snapshot of the pointers, so the lock isn't held while building; the
        // expanded set is read the same way, off the window, since this doesn't
        // touch which rows are open
        let fields: Fields = self.shared.current.lock_recover().clone();
        let expanded = current_expanded_fields(&window);
        let rows_weak = self.weak.clone();
        self.rt.spawn_blocking(move || {
            let (av_field_chars, av_rows) = build_all_values_table(&fields, &filter, &expanded);

            let _ = slint::invoke_from_event_loop(move || {
                let Some(window) = rows_weak.upgrade() else { return };
                window.global::<KeyPanel>().set_all_values_field_chars(av_field_chars);
                window.global::<KeyPanel>().set_all_values_rows(ModelRc::new(VecModel::from(av_rows)));
            });
        });
        // the table view, if that's what's shown, needs the same badges applied
        self.show_view(&window, None);
        // a hash not fully loaded yet: keep paging through the rest of it now
        // that there's something (still) to search for
        self.kick_off_scan_if_active();
    }

    /// Enter in the search bar: turns the typed text into badges (a quoted run is one,
    /// otherwise each word) tagged with the picked scope, and clears the box. Duplicates
    /// are dropped; if the rest exceeds [`MAX_BADGES`], none are added and the box says so.
    pub(super) fn on_filter_query_submitted(&self) {
        let Some(window) = self.weak.upgrade() else { return };
        let scope = window.global::<KeyPanel>().get_filter_scope();
        let tokens = tokenize(&window.global::<KeyPanel>().get_filter_query());
        if tokens.is_empty() {
            return;
        }
        let mut badges: Vec<BadgeData> = window.global::<KeyPanel>().get_filter_badges().iter().collect();
        let already_active = |text: &str| badges.iter().any(|b| b.scope == scope && b.text.as_str() == text);
        let mut seen = FxHashSet::default();
        let new_tokens: Vec<String> = tokens.into_iter().filter(|t| !already_active(t) && seen.insert(t.clone())).collect();
        if new_tokens.is_empty() {
            window.global::<KeyPanel>().set_filter_query("".into());
            return;
        }
        if badges.len() + new_tokens.len() > MAX_BADGES {
            window.global::<KeyPanel>().set_filter_limit_message(t!("Remove a badge first (5 max)").as_ref().into());
            return;
        }
        badges.extend(new_tokens.into_iter().map(|text| BadgeData { text: text.into(), scope }));
        window.global::<KeyPanel>().set_filter_badges(ModelRc::new(VecModel::from(badges)));
        window.global::<KeyPanel>().set_filter_query("".into());
        window.global::<KeyPanel>().set_filter_limit_message("".into());
        self.on_search_changed();
    }

    /// A badge's × was clicked: drops it and re-renders.
    pub(super) fn on_filter_badge_removed(&self, text: &str, scope: SearchScope) {
        let Some(window) = self.weak.upgrade() else { return };
        let badges: Vec<BadgeData> = window.global::<KeyPanel>().get_filter_badges().iter().filter(|b| !(b.scope == scope && b.text.as_str() == text)).collect();
        window.global::<KeyPanel>().set_filter_badges(ModelRc::new(VecModel::from(badges)));
        window.global::<KeyPanel>().set_filter_limit_message("".into());
        self.on_search_changed();
    }

    /// Enter in the filter bar: applies its path to the loaded fields (off the UI
    /// thread) and swaps what both views show. A path that doesn't parse is
    /// reported next to the box and leaves the display as it was.
    pub(super) fn on_json_filter_submitted(&self) {
        let Some(window) = self.weak.upgrade() else { return };
        let path = match parse_filter(&window.global::<KeyPanel>().get_json_filter()) {
            Ok(path) => path,
            Err(error) => {
                window.global::<KeyPanel>().set_json_filter_error(error.into());
                return;
            }
        };
        window.global::<KeyPanel>().set_json_filter_error("".into());
        let filter = current_search_filter(&window);

        let epoch = self.shared.view_epoch.fetch_add(1, Ordering::Relaxed) + 1;
        let loaded: Fields = self.shared.loaded.lock_recover().clone();
        let expanded = current_expanded_fields(&window);
        let ctx = self.clone();
        self.rt.spawn_blocking(move || {
            let (fields, matches) = filter_fields(&loaded, path.as_ref());
            let status = filter_status(path.as_ref(), matches, fields.len());
            let (av_field_chars, av_rows) = build_all_values_table(&fields, &filter, &expanded);
            let _ = slint::invoke_from_event_loop(move || {
                let Some(window) = ctx.weak.upgrade() else { return };
                let shared = &ctx.shared;
                if shared.view_epoch.load(Ordering::Relaxed) != epoch {
                    return;
                }
                // a jump armed for the previous contents must not fire on these
                disarm_scroll(&window);
                *shared.current.lock_recover() = fields;
                *shared.applied_path.lock_recover() = AppliedPath { path, matches };
                // the cached renderings are of the previous contents
                shared.pretty_cache.lock_recover().clear();
                window.global::<KeyPanel>().set_json_filter_status(status.into());
                window.global::<KeyPanel>().set_all_values_field_chars(av_field_chars);
                window.global::<KeyPanel>().set_all_values_rows(ModelRc::new(VecModel::from(av_rows)));
                ctx.show_view(&window, None);
                // a hash not fully loaded yet: keep paging through the rest of it
                // now that the path filter might match further into it
                ctx.kick_off_scan_if_active();
            });
        });
    }
}
