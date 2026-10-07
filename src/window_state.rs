//! The window's place on the screen (`window.json`, next to the settings): kept while the
//! app runs, restored at the next start: same rect, or maximized on the same screen.

use std::cell::Cell;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use slint::winit_030::{WinitWindowAccessor, winit};
use slint::{ComponentHandle, PhysicalPosition, PhysicalSize, Timer, TimerMode};

use crate::MainWindow;
use crate::backend::persist::{self, Loaded};

const FILE: &str = "window.json";
const MIN_SIZE: (u32, u32) = (400, 300);
/// How often the window's place is looked at.
const POLL_INTERVAL: Duration = Duration::from_millis(100);
/// How many looks the window gets to settle into its saved place.
const SETTLE_TICKS: u32 = 30;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
struct Rect {
    x: i32,
    y: i32,
    width: u32,
    height: u32,
}

impl Rect {
    fn contains(&self, x: i64, y: i64) -> bool {
        (self.x as i64..self.x as i64 + self.width as i64).contains(&x) && (self.y as i64..self.y as i64 + self.height as i64).contains(&y)
    }

    fn center(&self) -> (i64, i64) {
        (self.x as i64 + self.width as i64 / 2, self.y as i64 + self.height as i64 / 2)
    }

    /// Whether at least a `w` by `h` part of this rect lies inside `other`.
    fn overlaps(&self, other: &Rect, w: i64, h: i64) -> bool {
        let overlap_w = (self.x as i64 + self.width as i64).min(other.x as i64 + other.width as i64) - (self.x as i64).max(other.x as i64);
        let overlap_h = (self.y as i64 + self.height as i64).min(other.y as i64 + other.height as i64) - (self.y as i64).max(other.y as i64);
        overlap_w >= w && overlap_h >= h
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WindowState {
    // the rect while the window is neither maximized nor minimized
    normal: Option<Rect>,
    maximized: bool,
    // the rect it covers while maximized: tells which screen it is maximized on
    maximized_rect: Option<Rect>,
}

impl WindowState {
    /// Where the window starts: its normal rect, brought onto the screen it was maximized on.
    fn start_rect(&self) -> Option<Rect> {
        let normal = self.normal.or_else(|| {
            let big = self.maximized_rect?;
            Some(Rect { x: big.x, y: big.y, width: big.width * 4 / 5, height: big.height * 4 / 5 })
        })?;
        let normal = Rect { width: normal.width.max(MIN_SIZE.0), height: normal.height.max(MIN_SIZE.1), ..normal };
        let Some(screen) = self.maximized_rect.filter(|_| self.maximized) else { return Some(normal) };
        let (cx, cy) = normal.center();
        if screen.contains(cx, cy) {
            return Some(normal);
        }
        let (width, height) = (normal.width.min(screen.width), normal.height.min(screen.height));
        Some(Rect { x: screen.x + (screen.width - width) as i32 / 2, y: screen.y + (screen.height - height) as i32 / 2, width, height })
    }
}

struct Tracker {
    path: PathBuf,
    state: Cell<WindowState>,
    // what the window is brought to once it exists: the OS may first rescale it
    target: Cell<Option<Rect>>,
    wants_maximized: bool,
    settling_ticks: Cell<u32>,
    matching_ticks: Cell<u32>,
}

impl Tracker {
    fn save(&self) {
        if let Err(e) = persist::save(&self.path, &self.state.get()) {
            eprintln!("can't save the window's place: {e}");
        }
    }

    /// One look at the window: while it settles into its saved place, nothing is recorded.
    fn tick(&self, window: &winit::window::Window) {
        if self.settling_ticks.get() > 0 {
            self.settle(window);
        } else {
            self.record(window);
        }
    }

    /// Brings the window to the saved rect, or waits for it to be maximized, until it stays
    /// there for two looks in a row (or gives up after a few seconds).
    fn settle(&self, window: &winit::window::Window) {
        let first = self.settling_ticks.get() == SETTLE_TICKS;
        self.settling_ticks.set(self.settling_ticks.get() - 1);
        if first {
            self.target.set(self.target.get().map(|rect| on_a_screen(window, rect)));
        }
        let settled = if self.wants_maximized {
            window.is_maximized()
        } else if let Some(target) = self.target.get() {
            // (Wayland can't tell a window's position: only its size is compared there)
            let at_target = window.outer_position().map(|p| (p.x, p.y) == (target.x, target.y)).unwrap_or(true)
                && (window.inner_size().width, window.inner_size().height) == (target.width, target.height);
            if at_target {
                self.matching_ticks.set(self.matching_ticks.get() + 1);
            } else {
                self.matching_ticks.set(0);
                window.set_outer_position(winit::dpi::PhysicalPosition::new(target.x, target.y));
                let _ = window.request_inner_size(winit::dpi::PhysicalSize::new(target.width, target.height));
            }
            self.matching_ticks.get() >= 2
        } else {
            true
        };
        if settled {
            self.settling_ticks.set(0);
        }
    }

    /// Takes the window's current place into the state, and saves it when it changed.
    fn record(&self, window: &winit::window::Window) {
        if window.is_minimized().unwrap_or(false) || window.fullscreen().is_some() {
            return;
        }
        let position = window.outer_position().unwrap_or_default();
        let size = window.inner_size();
        if size.width == 0 || size.height == 0 {
            return;
        }
        let rect = Rect { x: position.x, y: position.y, width: size.width, height: size.height };
        let mut state = self.state.get();
        state.maximized = window.is_maximized();
        if state.maximized {
            state.maximized_rect = Some(rect);
        } else {
            state.normal = Some(rect);
        }
        if state != self.state.get() {
            self.state.set(state);
            self.save();
        }
    }
}

/// Restores the window's saved place, before it is shown, and starts keeping it up to date.
/// Call `save` on the result once the event loop has ended.
pub fn install(window: &MainWindow) -> impl Fn() + use<> {
    let path = crate::backend::state::resolve_data_file().with_file_name(FILE);
    let state = match persist::load::<WindowState>(&path) {
        Loaded::Read(state) => state,
        Loaded::Missing => WindowState::default(),
        Loaded::Unusable(why) => {
            eprintln!("{} can't be read ({why})", path.display());
            WindowState::default()
        }
    };
    let start = state.start_rect();
    if let Some(rect) = start {
        window.window().set_size(PhysicalSize::new(rect.width, rect.height));
        window.window().set_position(PhysicalPosition::new(rect.x, rect.y));
    }
    if state.maximized {
        window.window().set_maximized(true);
    }

    let tracker = Rc::new(Tracker {
        path,
        state: Cell::new(state),
        target: Cell::new(start),
        wants_maximized: state.maximized,
        settling_ticks: Cell::new(if start.is_some() { SETTLE_TICKS } else { 0 }),
        matching_ticks: Cell::new(0),
    });
    // polled rather than driven by winit's move/resize events: it is the one way that
    // is certain to see every change, a maximize and a drag included
    let timer = Timer::default();
    let weak = window.as_weak();
    timer.start(TimerMode::Repeated, POLL_INTERVAL, {
        let tracker = tracker.clone();
        move || {
            if let Some(window) = weak.upgrade() {
                window.window().with_winit_window(|w| tracker.tick(w));
            }
        }
    });
    std::mem::forget(timer);
    move || tracker.save()
}

/// `rect`, or when it lies outside every screen (one was unplugged since), centered on the primary one.
fn on_a_screen(window: &winit::window::Window, rect: Rect) -> Rect {
    let screens: Vec<Rect> = window
        .available_monitors()
        .map(|m| Rect { x: m.position().x, y: m.position().y, width: m.size().width, height: m.size().height })
        .collect();
    if screens.is_empty() || screens.iter().any(|screen| rect.overlaps(screen, 100, 40.min(rect.height as i64))) {
        return rect;
    }
    let Some(primary) = window.primary_monitor().or_else(|| window.available_monitors().next()) else { return rect };
    let (screen_size, screen_position) = (primary.size(), primary.position());
    let width = rect.width.min(screen_size.width);
    let height = rect.height.min(screen_size.height);
    Rect {
        x: screen_position.x + (screen_size.width - width) as i32 / 2,
        y: screen_position.y + (screen_size.height - height) as i32 / 2,
        width,
        height,
    }
}
