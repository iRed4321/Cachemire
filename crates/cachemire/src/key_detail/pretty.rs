//! An open row's pretty text: rendered off the UI thread into the shared cache,
//! then served from it with find matches highlighted, and searched for selections.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use slint::ComponentHandle;
use slint::private_unstable_api::re_exports::StyledText;

use crate::backend::LockExt;
use crate::{FindResult, KeyPanel, MainWindow, SelectionRange};

use super::highlight::{find_matches, pretty_styled_text_with_highlights};
use super::pretty_text::{ColoredParagraph, paragraphs_for};
use super::{Ctx, word};

/// Pokes every row to check whether its own field's pretty text has landed
/// in the cache (via `pretty-is-ready`) — not "tells them which field", so
/// two renderings completing close together can't race a shared property.
fn signal_pretty_ready(window: &MainWindow) {
    window.global::<KeyPanel>().set_pretty_ready_tick(window.global::<KeyPanel>().get_pretty_ready_tick().wrapping_add(1));
}

/// Wires the KeyPanel callbacks an open row asks its pretty text through.
pub(super) fn init(window: &MainWindow, ctx: &Ctx) {
    let shared = &ctx.shared;

    // Cached paragraphs for a field. Rows only ask after the ready signal, so
    // this is normally a hit; on a miss it computes on the UI thread once,
    // which beats showing nothing.
    let cached_or_compute = {
        let content = shared.content.clone();
        let pretty_cache = shared.pretty_cache.clone();
        move |name: &str, escaped: bool| -> Option<Arc<Vec<ColoredParagraph>>> {
            if let Some(cached) = pretty_cache.lock_recover().get(name, escaped) {
                return Some(cached.clone());
            }
            let field = content.lock_recover().field(name)?;
            let paragraphs = Arc::new(paragraphs_for(&field.value(), escaped));
            pretty_cache.lock_recover().insert(name.to_string(), escaped, paragraphs.clone());
            Some(paragraphs)
        }
    };

    // a cheap, non-computing peek at the cache: whether `field`'s pretty text is
    // ready yet, for a row to ask about itself on every `pretty-ready-tick` pulse
    // (see that property's doc comment) instead of being told which field it was
    window.global::<KeyPanel>().on_pretty_is_ready({
        let pretty_cache = shared.pretty_cache.clone();
        move |field, escaped| pretty_cache.lock_recover().contains(&field, escaped)
    });

    window.global::<KeyPanel>().on_request_pretty_value({
        let cached_or_compute = cached_or_compute.clone();
        let content = shared.content.clone();
        move |field, query, current_match, escaped| {
            let Some(paragraphs) = cached_or_compute(field.as_str(), escaped) else { return StyledText::default() };
            let filter = content.lock_recover().search.clone();
            pretty_styled_text_with_highlights(&paragraphs, &filter.highlight_needles(), filter.case_sensitive, &query.trim().to_lowercase(), current_match)
        }
    });

    window.global::<KeyPanel>().on_find_in_value({
        let cached_or_compute = cached_or_compute.clone();
        move |field, query, current_match, escaped| {
            let query = query.trim().to_lowercase();
            let Some(paragraphs) = cached_or_compute(field.as_str(), escaped) else { return FindResult::default() };
            let matches = find_matches(&paragraphs, &query, false);
            if matches.is_empty() {
                return FindResult { count: 0, index: 0, line: 0, label: if query.is_empty() { "" } else { "0" }.into() };
            }
            let count = matches.len() as i32;
            let index = current_match.rem_euclid(count);
            FindResult {
                count,
                index,
                line: matches[index as usize].0 as i32,
                label: format!("{}/{count}", index + 1).into(),
            }
        }
    });

    // double click: the word under the click is selected; triple click: the whole value
    window.global::<KeyPanel>().on_value_selection({
        let cached_or_compute = cached_or_compute.clone();
        move |field, offset, whole, escaped| {
            let none = SelectionRange { start: -1, end: -1 };
            let Some(paragraphs) = cached_or_compute(field.as_str(), escaped) else { return none };
            let text = word::plain_text(&paragraphs);
            if whole {
                return SelectionRange { start: 0, end: text.len() as i32 };
            }
            match word::word_range(&text, offset.max(0) as usize) {
                Some((start, end)) => SelectionRange { start: start as i32, end: end as i32 },
                None => none,
            }
        }
    });

    // Renders a field's pretty text off the UI thread, announced via the
    // pretty-ready-* properties once cached. Called on expand, or ahead of
    // time by a row's hover dwell timer (only after the pointer lingers).
    window.global::<KeyPanel>().on_request_pretty_value_async({
        let ctx = ctx.clone();
        move |name, escaped| {
            let name = name.to_string();
            let shared = &ctx.shared;
            if shared.pretty_cache.lock_recover().contains(&name, escaped) {
                if let Some(window) = ctx.weak.upgrade() {
                    signal_pretty_ready(&window);
                }
                return;
            }
            let (field, key_at_request) = {
                let content = shared.content.lock_recover();
                let Some(field) = content.field(&name) else { return };
                (field, content.key.clone())
            };
            let epoch_at_request = shared.view_epoch.load(Ordering::Relaxed);
            let content = shared.content.clone();
            let view_epoch = shared.view_epoch.clone();
            let pretty_cache = shared.pretty_cache.clone();
            let weak = ctx.weak.clone();
            ctx.rt.spawn_blocking(move || {
                let paragraphs = Arc::new(paragraphs_for(&field.value(), escaped));
                // the key changed, or a filter was applied, while rendering: this is for stale contents
                if content.lock_recover().key != key_at_request || view_epoch.load(Ordering::Relaxed) != epoch_at_request {
                    return;
                }
                pretty_cache.lock_recover().insert(name, escaped, paragraphs);
                let _ = weak.upgrade_in_event_loop(move |window| {
                    signal_pretty_ready(&window);
                });
            });
        }
    });
}
