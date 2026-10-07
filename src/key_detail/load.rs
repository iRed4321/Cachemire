//! Loading a key into the panel: fetched (or reused from what was fetched),
//! filtered, rendered off the UI thread, then shown and scrolled into place.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use rust_i18n::t;
use slint::{ComponentHandle, ModelRc, VecModel};

use crate::backend::LockExt;
use crate::backend::explorer;
use crate::backend::key_name::KeyName;
use crate::backend::redis_value_type::RedisValueType;
use crate::{AllValuesRowData, KeyPanel, KeyTree};

use super::field::{Fields, build_fields, single_value_field};
use super::paging::holds_field;
use super::pretty_text::paragraphs_for;
use super::scroll::{ScrollPos, arm_all_scroll, current_expanded_fields, disarm_scroll};
use super::table::clear_table;
use super::value_list::{build_all_values_table, filter_fields, filter_status, meta_text, parse_filter};
use super::{AppliedPath, Ctx, FetchedKey, HASH_PAGE_SIZE, KeyDetailShared, KeyView, MAX_ITEMS, current_search_filter, show_key_name};

/// A `load_key` call: what to show and how.
pub(super) struct LoadRequest {
    pub(super) key: String,
    pub(super) scroll: ScrollPos,
    /// Show what was fetched last time if there is any; otherwise (and always
    /// when this is false) the key is fetched from the server.
    pub(super) reuse: bool,
    /// The "Refreshed" toast confirms this load once shown (the refresh button).
    pub(super) announce: bool,
}

impl Ctx {
    /// Shows the requested key in the panel; once it is shown it is scrolled to
    /// its requested position. See [`LoadRequest`].
    pub(super) fn load_key(&self, request: LoadRequest) {
        let LoadRequest { key, scroll, reuse, announce } = request;
        let ctx = self.clone();
        let weak = &self.weak;
        let KeyDetailShared { current, pretty_cache, current_key, applied_search, applied_path, loaded, view_epoch, fetched, suggester, .. } = self.shared.clone();
        let epoch = view_epoch.fetch_add(1, Ordering::Relaxed) + 1;
        let cache_slot = (weak.upgrade().map(|w| w.connection_id().to_string()).unwrap_or_default(), key.clone());
        // a field asked for (a hit of a search tab): the page holding it is the one loaded
        let focus = weak.upgrade().and_then(|w| {
            let scroll = w.global::<crate::KeyScroll>();
            scroll.get_reveal_pending().then(|| scroll.get_reveal_field().to_string())
        });
        let descending = weak.upgrade().is_some_and(|w| w.global::<KeyPanel>().get_sort_descending());
        let cached = if reuse { fetched.lock_recover().get(&cache_slot).cloned() } else { None };
        let cached = cached.filter(|fetch| (fetch.all_names.is_none() || fetch.descending == descending) && focus.as_ref().is_none_or(|f| holds_field(fetch, f)));

        // field names can collide across keys, so the previous key's pretty text
        // must not be served for this one; renderings still in flight are dropped
        // on completion by comparing against current_key
        pretty_cache.lock_recover().clear();
        current_key.lock_recover().clone_from(&key);

        // read on the UI thread, before spawning: on a tab switch `tabs.rs`
        // writes the target tab's saved filter/expanded fields into these
        // properties just before calling us, so this is that tab's own state.
        let (filter, path, expanded) = weak
            .upgrade()
            .map(|window| {
                // flags this key as in flight so the keyspace tree can spin its
                // row; cleared in the completion closure below
                window.global::<KeyTree>().set_loading_key(key.as_str().into());
                // nothing shown yet: the header carries the key's name while it loads
                if !window.global::<KeyPanel>().get_loaded() {
                    show_key_name(&window, &key);
                }
                // the filter bar's path comes along (the tab's own, restored just
                // like the two filters)
                let path = match parse_filter(&window.global::<KeyPanel>().get_json_filter()) {
                    Ok(path) => {
                        window.global::<KeyPanel>().set_json_filter_error("".into());
                        path
                    }
                    Err(error) => {
                        window.global::<KeyPanel>().set_json_filter_error(error.into());
                        None
                    }
                };
                (current_search_filter(&window), path, current_expanded_fields(&window))
            })
            .unwrap_or_default();
        *applied_search.lock_recover() = filter.clone();
        let pretty_cache_for_restore = pretty_cache.clone();
        let applied = path.clone();

        self.rt.spawn(async move {
            let fetch = match cached {
                Some(fetch) => Ok(fetch),
                None => explorer::get_key_details(&ctx.state, None, &KeyName::from_id(&key), MAX_ITEMS, HASH_PAGE_SIZE, focus.as_deref(), descending).await.map(|details| {
                    let fields: Fields = if details.key_type == RedisValueType::Hash {
                        build_fields(details.value)
                    } else {
                        vec![Arc::new(single_value_field(details.value))]
                    };
                    // a hash's true field count is known from the start (`HLEN`), so
                    // it's never marked truncated: the rest just hasn't been paged in yet
                    let badge = details.key_type.as_str().to_uppercase();
                    let mut fetch = FetchedKey {
                        badge,
                        meta: String::new(),
                        fields,
                        all_names: details.all_field_names,
                        start: details.page_start,
                        field_count: details.field_count,
                        ttl: details.ttl,
                        names_cursor: details.names_cursor,
                        descending,
                    };
                    fetch.meta = if details.key_type == RedisValueType::Hash {
                        fetch.hash_meta()
                    } else {
                        meta_text(&details.key_type, details.ttl, fetch.fields.len(), details.truncated, None)
                    };
                    fetched.lock_recover().insert(cache_slot, fetch.clone());
                    fetch
                }),
            };

            // parsing, coloring and pretty-printing every field scales with the
            // field count, so it all happens here rather than in the event-loop
            // closure below, which only wraps models and sets properties
            let prepared = fetch.map(|FetchedKey { badge, meta, fields, .. }| {
                let (shown, matches) = filter_fields(&fields, path.as_ref());
                let status = filter_status(path.as_ref(), matches, shown.len());
                // rows about to reopen (`expanded`) get pretty text rendered here,
                // on this same task, instead of on first ask — otherwise a restored
                // row flashes "Formatting..." then grows
                {
                    let mut cache = pretty_cache_for_restore.lock_recover();
                    for f in shown.iter().filter(|f| expanded.contains(&f.name)) {
                        cache.entry((f.name.clone(), false)).or_insert_with(|| Arc::new(paragraphs_for(&f.value(), false)));
                    }
                }
                let (av_field_chars, av_rows) = build_all_values_table(&shown, &filter, &expanded);

                (badge, meta, fields, shown, matches, status, av_field_chars, av_rows)
            });

            let _ = slint::invoke_from_event_loop(move || {
                let Some(window) = ctx.weak.upgrade() else { return };
                // only clear our own flag: a newer load for another key may have
                // replaced it in the meantime, and that one is still in flight
                if window.global::<KeyTree>().get_loading_key().as_str() == key {
                    window.global::<KeyTree>().set_loading_key("".into());
                }
                // a newer load (or a filter applied since) has taken over
                if view_epoch.load(Ordering::Relaxed) != epoch {
                    return;
                }
                match prepared {
                    Ok((badge, meta, fields, shown, matches, status, av_field_chars, av_rows)) => {
                        window.global::<KeyPanel>().set_badge(badge.into());
                        window.global::<KeyPanel>().set_loaded(true);
                        show_key_name(&window, &key);
                        window.global::<KeyPanel>().set_meta(meta.into());
                        window.global::<KeyPanel>().set_json_filter_status(status.into());

                        // the view that shows this content takes the jump when the content
                        // reaches it: the value list right here, the table once it is built
                        disarm_scroll(&window);
                        if window.global::<KeyPanel>().get_view() == KeyView::Fields {
                            arm_all_scroll(&window, scroll);
                        }
                        // a row asked for (a click on a hit of a search tab): where it is
                        // among these rows, so the list builds up to it and scrolls there
                        {
                            let scroll = window.global::<crate::KeyScroll>();
                            if scroll.get_reveal_pending() {
                                let field = scroll.get_reveal_field();
                                let index = av_rows.iter().position(|row| row.field == field);
                                scroll.set_reveal_index(index.map_or(-1, |i| i as i32));
                                scroll.set_reveal_pending(index.is_some());
                            }
                        }
                        window.global::<KeyPanel>().set_all_values_field_chars(av_field_chars);
                        window.global::<KeyPanel>().set_all_values_rows(ModelRc::new(VecModel::from(av_rows)));

                        *loaded.lock_recover() = fields;
                        suggester.forget();
                        *current.lock_recover() = shown;
                        *applied_path.lock_recover() = AppliedPath { path: applied, matches };
                        ctx.show_view(&window, Some(scroll));
                        // a tab restored with an active search and a hash not
                        // fully loaded yet: keep paging through the rest of it
                        ctx.kick_off_scan_if_active();
                        if announce {
                            crate::toast::show_refreshed(&window);
                        }
                    }
                    Err(e) => {
                        disarm_scroll(&window);
                        window.global::<KeyPanel>().set_badge("".into());
                        window.global::<KeyPanel>().set_loaded(false);
                        show_key_name(&window, &key);
                        window.global::<KeyPanel>().set_meta(t!("failed to load: %{error}", error = e).as_ref().into());
                        window.global::<KeyPanel>().set_all_values_field_chars(8);
                        window.global::<KeyPanel>().set_all_values_rows(ModelRc::new(VecModel::from(Vec::<AllValuesRowData>::new())));
                        window.global::<KeyPanel>().set_json_filter_status("".into());
                        *loaded.lock_recover() = Vec::new();
                        suggester.forget();
                        *current.lock_recover() = Vec::new();
                        clear_table(&window);
                    }
                }
            });
        });
    }
}
