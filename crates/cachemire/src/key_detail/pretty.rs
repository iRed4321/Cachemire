//! An open row's pretty text: rendered off the UI thread into the shared cache, then
//! served from it a few lines at a time (see [`super::value_lines`]), with find matches
//! highlighted, and searched for matches and selections.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use slint::{ComponentHandle, ModelRc, VecModel};

use crate::{FindResult, KeyPanel, LineSelection, MainWindow, ValueLine};

use super::Ctx;
use super::highlight::window_highlights;
use super::pretty_text::{ColoredParagraph, paragraphs_for};
use super::value_lines::{LineMemos, Lines, VisualLine};

/// Pokes every row to check whether its own field's pretty text has landed
/// in the cache (via `pretty-is-ready`) — not "tells them which field", so
/// two renderings completing close together can't race a shared property.
fn signal_pretty_ready(window: &MainWindow) {
    window.global::<KeyPanel>().set_pretty_ready_tick(window.global::<KeyPanel>().get_pretty_ready_tick().wrapping_add(1));
}

/// A field's paragraphs, and the lines they're drawn in.
type TextLines = (Arc<Vec<ColoredParagraph>>, Rc<Vec<VisualLine>>);

/// A position as Slint holds it (line, byte offset), negatives as 0.
fn position(line: i32, offset: i32) -> (usize, usize) {
    (line.max(0) as usize, offset.max(0) as usize)
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
            if let Some(cached) = pretty_cache.lock().get(name, escaped) {
                return Some(cached.clone());
            }
            let field = content.lock().field(name)?;
            let paragraphs = Arc::new(paragraphs_for(&field.value(), escaped));
            pretty_cache.lock().insert(name.to_string(), escaped, paragraphs.clone());
            Some(paragraphs)
        }
    };
    // a field's paragraphs and their lines wrapped to `columns` characters
    let memos = Rc::new(RefCell::new(LineMemos::default()));
    let with_lines = {
        let memos = memos.clone();
        let cached_or_compute = cached_or_compute.clone();
        move |name: &str, escaped: bool, columns: i32| -> Option<TextLines> {
            let paragraphs = cached_or_compute(name, escaped)?;
            let layout = memos.borrow_mut().layout(name, escaped, &paragraphs, columns.max(1) as usize);
            Some((paragraphs, layout))
        }
    };

    // a cheap, non-computing peek at the cache: whether `field`'s pretty text is
    // ready yet, for a row to ask about itself on every `pretty-ready-tick` pulse
    // (see that property's doc comment) instead of being told which field it was
    window.global::<KeyPanel>().on_pretty_is_ready({
        let pretty_cache = shared.pretty_cache.clone();
        move |field, escaped| pretty_cache.lock().contains(&field, escaped)
    });

    window.global::<KeyPanel>().on_value_line_count({
        let with_lines = with_lines.clone();
        move |field, escaped, columns| with_lines(&field, escaped, columns).map_or(0, |(_, layout)| layout.len() as i32)
    });

    window.global::<KeyPanel>().on_request_value_lines({
        let with_lines = with_lines.clone();
        let memos = memos.clone();
        let content = shared.content.clone();
        move |field, query, current, escaped, columns, first, count| {
            let Some((paragraphs, layout)) = with_lines(&field, escaped, columns) else { return ModelRc::default() };
            let query = query.trim().to_lowercase();
            let finds = memos.borrow_mut().finds(&field, escaped, &paragraphs, &query);
            let filter = content.lock().search.clone();
            let needles = filter.highlight_needles();
            let lines = Lines { paragraphs: &paragraphs, layout: &layout };
            let first = first.max(0) as usize;
            let built = lines.styled(first..first + count.max(0) as usize, |range| {
                window_highlights(&paragraphs, range, &needles, filter.case_sensitive, &finds, current)
            });
            let rows: Vec<ValueLine> = built.into_iter().map(|(text, len)| ValueLine { text, len: len as i32 }).collect();
            ModelRc::new(VecModel::from(rows))
        }
    });

    window.global::<KeyPanel>().on_find_in_value({
        let with_lines = with_lines.clone();
        let memos = memos.clone();
        move |field, query, current_match, escaped, columns| {
            let query = query.trim().to_lowercase();
            let Some((paragraphs, layout)) = with_lines(&field, escaped, columns) else { return FindResult::default() };
            let matches = memos.borrow_mut().finds(&field, escaped, &paragraphs, &query);
            if matches.is_empty() {
                return FindResult { count: 0, index: 0, line: 0, label: if query.is_empty() { "" } else { "0" }.into() };
            }
            let count = matches.len() as i32;
            let index = current_match.rem_euclid(count);
            let (para, range) = &matches[index as usize];
            let (line, _) = Lines { paragraphs: &paragraphs, layout: &layout }.position_of(*para, range.start);
            FindResult { count, index, line: line as i32, label: format!("{}/{count}", index + 1).into() }
        }
    });

    window.global::<KeyPanel>().on_value_offset_at({
        let with_lines = with_lines.clone();
        move |field, escaped, columns, line, column| {
            let Some((paragraphs, layout)) = with_lines(&field, escaped, columns) else { return 0 };
            let (line, column) = position(line, column);
            Lines { paragraphs: &paragraphs, layout: &layout }.offset_at(line, column) as i32
        }
    });

    // double click: the word under the click is selected; triple click: the whole value
    window.global::<KeyPanel>().on_value_selection({
        let with_lines = with_lines.clone();
        move |field, escaped, columns, line, offset, whole| {
            let none = LineSelection { start_line: -1, start_offset: 0, end_line: -1, end_offset: 0 };
            let Some((paragraphs, layout)) = with_lines(&field, escaped, columns) else { return none };
            let lines = Lines { paragraphs: &paragraphs, layout: &layout };
            let range = if whole { Some(((0, 0), lines.end())) } else { lines.word_at(line.max(0) as usize, offset.max(0) as usize) };
            match range {
                Some(((sl, so), (el, eo))) => {
                    LineSelection { start_line: sl as i32, start_offset: so as i32, end_line: el as i32, end_offset: eo as i32 }
                }
                None => none,
            }
        }
    });

    window.global::<KeyPanel>().on_value_selected_text({
        let with_lines = with_lines.clone();
        move |field, escaped, columns, selection| {
            let Some((paragraphs, layout)) = with_lines(&field, escaped, columns) else { return Default::default() };
            let from = position(selection.start_line, selection.start_offset);
            let to = position(selection.end_line, selection.end_offset);
            Lines { paragraphs: &paragraphs, layout: &layout }.selected_text(from, to).into()
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
            if shared.pretty_cache.lock().contains(&name, escaped) {
                if let Some(window) = ctx.weak.upgrade() {
                    signal_pretty_ready(&window);
                }
                return;
            }
            let (field, key_at_request) = {
                let content = shared.content.lock();
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
                if content.lock().key != key_at_request || view_epoch.load(Ordering::Relaxed) != epoch_at_request {
                    return;
                }
                pretty_cache.lock().insert(name, escaped, paragraphs);
                let _ = weak.upgrade_in_event_loop(move |window| {
                    signal_pretty_ready(&window);
                });
            });
        }
    });
}
