//! Headless input/render tests against the real window, on Slint's software renderer, by
//! area: [`table`], [`values`], [`header`], [`app`], [`dialogs`], [`connections`], [`cloud`] and [`tabs`].
//! Each test builds its own [`Harness`] (thread-local Slint context).

use crate::{KeyPanel, MainWindow, QueryTab, SavedQueries, SearchTab, TabStrip};
use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType, Rgb565Pixel};
use slint::platform::{Platform, PlatformError, PointerEventButton, WindowAdapter, WindowEvent};
use slint::{ComponentHandle, Model, ModelRc, VecModel};
use slint::{LogicalPosition, PhysicalSize};
use std::cell::RefCell;
use std::rc::Rc;

mod app;
mod cloud;
mod connections;
mod dialogs;
mod tabs;

/// What other threads hand to the UI thread (`invoke_from_event_loop`). Slint keeps one proxy
/// for the whole process, so this queue is shared by every test; see [`EventLoopTurn`].
static HANDED: parking_lot::Mutex<Vec<Box<dyn FnOnce() + Send>>> = parking_lot::Mutex::new(Vec::new());

struct TestPlatform(Rc<MinimalSoftwareWindow>);
impl Platform for TestPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, PlatformError> {
        Ok(self.0.clone())
    }

    // only the first platform's proxy is taken: Slint refuses a second one
    fn new_event_loop_proxy(&self) -> Option<Box<dyn slint::platform::EventLoopProxy>> {
        static GIVEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        (!GIVEN.swap(true, std::sync::atomic::Ordering::SeqCst)).then(|| Box::new(TestProxy) as Box<dyn slint::platform::EventLoopProxy>)
    }
}

struct TestProxy;
impl slint::platform::EventLoopProxy for TestProxy {
    fn quit_event_loop(&self) -> Result<(), slint::EventLoopError> {
        Ok(())
    }

    fn invoke_from_event_loop(&self, event: Box<dyn FnOnce() + Send>) -> Result<(), slint::EventLoopError> {
        HANDED.lock().push(event);
        Ok(())
    }
}

/// The test whose thread runs what is handed to the UI thread: one at a time, since
/// `crate::app::later` reaches the app of whichever thread runs it.
static EVENT_LOOP: parking_lot::Mutex<()> = parking_lot::Mutex::new(());
static EVENT_LOOP_THREAD: parking_lot::Mutex<Option<std::thread::ThreadId>> = parking_lot::Mutex::new(None);

/// This thread's turn to run what is handed to the UI thread, until dropped.
struct EventLoopTurn(#[allow(dead_code)] parking_lot::MutexGuard<'static, ()>);

impl EventLoopTurn {
    fn take() -> Self {
        let turn = EventLoopTurn(EVENT_LOOP.lock());
        // what an earlier test left was for its own window
        HANDED.lock().clear();
        *EVENT_LOOP_THREAD.lock() = Some(std::thread::current().id());
        turn
    }
}

impl Drop for EventLoopTurn {
    fn drop(&mut self) {
        *EVENT_LOOP_THREAD.lock() = None;
    }
}

pub(crate) const WIDTH: usize = 1300;
pub(crate) const HEIGHT: usize = 800;

/// A headless window at 1300x800, shown and ready for input/render.
pub(crate) struct Harness {
    software: Rc<MinimalSoftwareWindow>,
    pub(crate) window: MainWindow,
    pub(crate) buffer: Vec<Rgb565Pixel>,
}

impl Harness {
    pub(crate) fn new() -> Self {
        let software = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
        software.set_size(PhysicalSize::new(WIDTH as u32, HEIGHT as u32));
        slint::platform::set_platform(Box::new(TestPlatform(software.clone()))).unwrap();
        let window = MainWindow::new().unwrap();
        window.show().unwrap();
        slint::select_bundled_translation("en").unwrap();
        // the key panel only shows/accepts input once a connection is picked
        // and a tab is open — every test here interacts with that panel
        window.global::<crate::ConnectionState>().set_view(crate::ConnectionView { connected: true, ..Default::default() });
        window.global::<TabStrip>().set_tabs(ModelRc::new(VecModel::from(vec![crate::TabData {
            key: "users".into(),
            active: true,
            label: "users".into(),
            search: false,
            query: false,
        }])));
        window.global::<TabStrip>().set_active_index(0);
        window.global::<KeyPanel>().set_badge("HASH".into());
        window.global::<KeyPanel>().set_loaded(true);
        window.global::<KeyPanel>().set_name("users".into());
        Self { software, window, buffer: vec![Rgb565Pixel::default(); WIDTH * HEIGHT] }
    }

    /// Runs what other threads handed to the UI thread when it's this test's turn (see
    /// [`EventLoopTurn`]), then renders what changed.
    pub(crate) fn frame(&mut self) {
        if *EVENT_LOOP_THREAD.lock() == Some(std::thread::current().id()) {
            let handed = std::mem::take(&mut *HANDED.lock());
            for job in handed {
                job();
            }
        }
        slint::platform::update_timers_and_animations();
        let Self { software, buffer, .. } = self;
        software.draw_if_needed(|r| {
            r.render(buffer, WIDTH);
        });
    }

    /// Lets pending animations/inertia finish, rendering along the way.
    pub(crate) fn settle(&mut self) {
        for _ in 0..30 {
            std::thread::sleep(std::time::Duration::from_millis(20));
            self.frame();
        }
    }

    pub(crate) fn click(&mut self, x: f32, y: f32) {
        let position = LogicalPosition::new(x, y);
        self.window.window().dispatch_event(WindowEvent::PointerMoved { position });
        self.window.window().dispatch_event(WindowEvent::PointerPressed { position, button: PointerEventButton::Left });
        self.window.window().dispatch_event(WindowEvent::PointerReleased { position, button: PointerEventButton::Left });
    }
}

/// Waits past the double-click time, so the next press is a fresh click.
pub(crate) fn wait_out_double_click(h: &mut Harness) {
    std::thread::sleep(std::time::Duration::from_millis(550));
    h.frame();
}

/// An app state in a directory of its own, its secrets in memory.
fn test_state() -> std::sync::Arc<cachemire_core::state::AppState> {
    std::sync::Arc::new(cachemire_core::state::AppState::for_tests(&cachemire_core::test_support::temp_dir("headless")))
}

/// The app context the windows' callbacks reach through `crate::app::with`, made the thread's
/// own, with its turn to run what comes back from other threads; keep the runtime and the turn.
fn install_app(
    h: &Harness,
    state: &std::sync::Arc<cachemire_core::state::AppState>,
) -> (Rc<crate::app::App>, tokio::runtime::Runtime, EventLoopTurn) {
    let turn = EventLoopTurn::take();
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let rt = runtime.handle().clone();
    let app = crate::app::install(crate::app::App {
        window: h.window.as_weak(),
        key_detail: crate::key_detail::init(&h.window, state.clone(), rt.clone()),
        keyspace: crate::keyspace::Keyspace::new(&h.window, state.clone(), rt.clone()),
        state: state.clone(),
        rt,
        tabs: Default::default(),
        searches: Default::default(),
        queries: Default::default(),
        transfer: Default::default(),
        cloud: Default::default(),
    });
    (app, runtime, turn)
}

/// Clicks each point of the `xs` by `ys` grid until `found` says the click did it, calling
/// `between` after each miss (to reopen a window a click outside it closed); the point found.
pub(crate) fn sweep(
    h: &mut Harness,
    xs: std::ops::Range<i32>,
    ys: std::ops::Range<i32>,
    step: usize,
    mut between: impl FnMut(&Harness),
    found: impl Fn(&Harness) -> bool,
) -> Option<(f32, f32)> {
    for y in ys.step_by(step) {
        for x in xs.clone().step_by(step) {
            h.click(x as f32, y as f32);
            if found(h) {
                return Some((x as f32, y as f32));
            }
            between(h);
        }
    }
    None
}

/// Renders until `done` holds (what a background task hands back has arrived), for a few
/// seconds at most; whether it did.
fn wait_for(h: &mut Harness, done: impl Fn(&Harness) -> bool) -> bool {
    for _ in 0..300 {
        h.frame();
        if done(h) {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    false
}
