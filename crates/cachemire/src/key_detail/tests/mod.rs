//! The key panel's headless tests, on the window [`Harness`] drives: [`header`], [`table`]
//! and [`values`]; with the helpers that put a key's rows on screen, which the app's own
//! UI tests (`crate::ui_tests`) use too.

use super::scroll::arm_all_scroll;
use super::test_support::field;
use super::value_list::build_all_values_table;
use super::*;
use crate::SearchScope;
use crate::ui_tests::*;
use serde_json::json;
use slint::LogicalPosition;
use slint::platform::{PointerEventButton, WindowEvent};
use slint::private_unstable_api::re_exports::StyledText;
use std::cell::RefCell;
use std::rc::Rc;

mod header;
mod table;
mod values;

impl Harness {
    /// Opens the key panel's "More" section (the search bar and the JSON path
    /// filter) with a click on the chevron at the extreme right of the key's header.
    pub(crate) fn open_more(&mut self) {
        self.click(HEADER_MORE.0, HEADER_MORE.1);
        self.frame();
    }
}

// Where things sit in the 1300x800 window (the tab bar and the key's header above
// them; measured from a render, so a layout change moves these).
/// The "More" chevron, at the right end of the key's header.
const HEADER_MORE: (f32, f32) = (1263.0, 107.0);
/// The view switcher's segments span these x, on the header's row.
const HEADER_SWITCHER_X: std::ops::Range<i32> = 1120..1240;
const HEADER_ROW_Y: std::ops::Range<i32> = 98..118;
/// The row of the search bar and the JSON filter, once "More" is open.
pub(crate) const MORE_ROW_Y: std::ops::Range<i32> = 138..164;
/// The search box, and the JSON filter box, on that row.
pub(crate) const SEARCH_BOX_X: f32 = 700.0;
const JSON_FILTER_X: f32 = 1000.0;
/// The first row under the key's header (and its separator line) where a list starts.
const LIST_TOP_Y: usize = 128;
/// The table's column titles, and its first row (the rows are 27px apart).
const TABLE_HEADER_Y: f32 = 153.0;
const TABLE_FIRST_ROW_Y: f32 = 182.0;

/// `n` fields named `{tag}:0`..`{tag}:{n-1}`, each a small JSON object —
/// enough real content to lay out, color and scroll through.
fn hash_fields(n: usize, tag: &str) -> Fields {
    (0..n)
        .map(|i| {
            field(
                &format!("{tag}:{i}"),
                Some(json!({
                    "id": i,
                    "name": format!("person number {i}"),
                    "active": i % 2 == 0,
                    "address": {"city": "Lyon", "zip": "69000", "geo": {"lat": 45.75, "lon": 4.85}},
                    "tags": ["a", "b", {"deep": [1, 2, 3]}],
                    "note": null,
                })),
            )
        })
        .collect()
}

/// Mirrors what `tabs/mod.rs` does around a real load: writes the tab's saved
/// open fields into the window (`restore_expanded_fields`) before the rows
/// are built with them (`build_all_values_table`).
fn show_values(window: &MainWindow, fields: &Fields, pos: Option<ScrollPos>, expanded: &FxHashSet<String>) {
    disarm_scroll(window);
    if let Some(pos) = pos {
        arm_all_scroll(window, pos);
    }
    restore_expanded_fields(window, expanded);
    let (chars, rows) = build_all_values_table(fields, &SearchFilter::default(), expanded);
    window.global::<KeyPanel>().set_all_values_field_chars(chars);
    window.global::<KeyPanel>().set_all_values_rows(ModelRc::new(VecModel::from(rows)));
}

/// Wheel-scrolls (the value list or the table, whichever `position` reads)
/// until it's gone far down, then lets the inertia settle.
fn wheel_down_to(h: &mut Harness, position: impl Fn(&MainWindow) -> f32) {
    h.window.window().dispatch_event(WindowEvent::PointerMoved { position: LogicalPosition::new(700.0, 400.0) });
    for step in 0..400 {
        h.window.window().dispatch_event(WindowEvent::PointerScrolled {
            position: LogicalPosition::new(700.0, 400.0),
            delta_x: 0.0,
            delta_y: -120.0,
        });
        if step % 5 == 0 {
            std::thread::sleep(std::time::Duration::from_millis(20));
            h.frame();
        }
        if position(&h.window) < -2700.0 {
            break;
        }
    }
    for _ in 0..80 {
        std::thread::sleep(std::time::Duration::from_millis(20));
        slint::platform::update_timers_and_animations();
    }
    h.frame();
}

/// Gets the table view on screen with `fields`, as a real load of a hash key
/// with a filter applied would.
fn table_ready(h: &mut Harness, fields: &Fields) {
    h.window.global::<KeyPanel>().set_view(KeyView::Table);
    apply_table(&h.window, build_table(fields, &[], false));
    assert!(h.window.global::<KeyPanel>().get_table_columns().row_count() > 5);
    h.frame();
    h.frame();
}

/// [`table_ready`] with `n` fields of [`hash_fields`], for the app's UI tests.
pub(crate) fn hash_table_ready(h: &mut Harness, n: usize, tag: &str) {
    table_ready(h, &hash_fields(n, tag));
}

/// Puts `n` fields of [`hash_fields`] in the key panel's table, as a query's result lands there.
pub(crate) fn show_hash_table(window: &MainWindow, n: usize, tag: &str) {
    window.global::<KeyPanel>().set_view(KeyView::Table);
    apply_table(window, build_table(&hash_fields(n, tag), &[], false));
}
