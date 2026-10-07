//! Headless input/render tests against the real window, on Slint's software renderer,
//! by area: [`table`], [`values`], [`header`] and [`app`]. Each test builds its own
//! [`Harness`] (thread-local Slint context). All `#[ignore]`d; run with --ignored.

#![cfg(test)]

use slint::ComponentHandle;
use crate::{SavedQueries, TabStrip, SearchTab, QueryTab, KeyPanel};
use super::*;
use super::scroll::arm_all_scroll;
use super::value_list::build_all_values_table;
use crate::SearchScope;
use slint::private_unstable_api::re_exports::StyledText;
use serde_json::json;
use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType, Rgb565Pixel};
use slint::platform::{Platform, PlatformError, PointerEventButton, WindowAdapter, WindowEvent};
use slint::{LogicalPosition, PhysicalSize};
use std::cell::RefCell;
use std::rc::Rc;
use rustc_hash::FxHashSet;
use test_support::field;

mod app;
mod header;
mod table;
mod values;

struct TestPlatform(Rc<MinimalSoftwareWindow>);
impl Platform for TestPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, PlatformError> {
        Ok(self.0.clone())
    }
}

const WIDTH: usize = 1300;
const HEIGHT: usize = 800;

/// A headless window at 1300x800, shown and ready for input/render.
struct Harness {
    software: Rc<MinimalSoftwareWindow>,
    window: MainWindow,
    buffer: Vec<Rgb565Pixel>,
}

impl Harness {
    fn new() -> Self {
        let software = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
        software.set_size(PhysicalSize::new(WIDTH as u32, HEIGHT as u32));
        slint::platform::set_platform(Box::new(TestPlatform(software.clone()))).unwrap();
        let window = MainWindow::new().unwrap();
        window.show().unwrap();
        slint::select_bundled_translation("en").unwrap();
        // the key panel only shows/accepts input once a connection is picked
        // and a tab is open — every test here interacts with that panel
        window.global::<crate::ConnectionState>().set_view(crate::ConnectionView { connected: true, ..Default::default() });
        window.global::<TabStrip>().set_tabs(ModelRc::new(VecModel::from(vec![crate::TabData { key: "users".into(), active: true, label: "users".into(), search: false, query: false }])));
        window.global::<TabStrip>().set_active_index(0);
        window.global::<KeyPanel>().set_badge("HASH".into());
        window.global::<KeyPanel>().set_loaded(true);
        window.global::<KeyPanel>().set_name("users".into());
        Self { software, window, buffer: vec![Rgb565Pixel::default(); WIDTH * HEIGHT] }
    }

    fn frame(&mut self) {
        slint::platform::update_timers_and_animations();
        let Self { software, buffer, .. } = self;
        software.draw_if_needed(|r| {
            r.render(buffer, WIDTH);
        });
    }

    /// Lets pending animations/inertia finish, rendering along the way.
    fn settle(&mut self) {
        for _ in 0..30 {
            std::thread::sleep(std::time::Duration::from_millis(20));
            self.frame();
        }
    }

    fn click(&mut self, x: f32, y: f32) {
        let position = LogicalPosition::new(x, y);
        self.window.window().dispatch_event(WindowEvent::PointerMoved { position });
        self.window.window().dispatch_event(WindowEvent::PointerPressed { position, button: PointerEventButton::Left });
        self.window.window().dispatch_event(WindowEvent::PointerReleased { position, button: PointerEventButton::Left });
    }

    /// Opens the key panel's "More" section (the search bar and the JSON path
    /// filter) with a click on the chevron at the extreme right of the key's header.
    fn open_more(&mut self) {
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
const MORE_ROW_Y: std::ops::Range<i32> = 138..164;
/// The search box, and the JSON filter box, on that row.
const SEARCH_BOX_X: f32 = 700.0;
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

/// Mirrors what `tabs.rs` does around a real load: writes the tab's saved
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
        h.window.window().dispatch_event(WindowEvent::PointerScrolled { position: LogicalPosition::new(700.0, 400.0), delta_x: 0.0, delta_y: -120.0 });
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

/// Waits past the double-click time, so the next press is a fresh click.
fn wait_out_double_click(h: &mut Harness) {
    std::thread::sleep(std::time::Duration::from_millis(550));
    h.frame();
}
