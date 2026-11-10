//! The whole app against a real Redis server, each test on an emptied database of its own
//! (see `TestRedis`): connecting walks the keyspace into the tree, then [`tree`] and
//! [`values`] act as the user does. A plain `cargo test` skips them: `cargo xtask e2e` runs them.

use super::*;
use crate::{ConnectionState, KeyTree, ValueEditor};
use cachemire_core::test_support::TestRedis;
use std::sync::Arc;
use std::time::{Duration, Instant};

mod tree;
mod values;

/// The app connected to a database the test filled, its keyspace walked into the tree.
struct Running {
    h: Harness,
    _app: Rc<crate::app::App>,
    rt: tokio::runtime::Runtime,
    redis: TestRedis,
    _turn: EventLoopTurn,
}

impl Running {
    /// Connects to a fresh database once `seed` has filled it, and waits for the tree.
    fn start(seed: impl FnOnce(&mut TestRedis)) -> Self {
        let mut redis = TestRedis::claim();
        seed(&mut redis);
        let state = Arc::new(redis.state());
        let h = Harness::new();
        h.window.global::<TabStrip>().set_tabs(ModelRc::new(VecModel::from(Vec::<crate::TabData>::new())));
        h.window.global::<ConnectionState>().set_view(Default::default());
        let (app, rt, turn) = install_app(&h, &state);
        crate::tabs::init(&app, &h.window);
        crate::keyspace::init(&app, &h.window);
        crate::connections::init(&app, &h.window);
        crate::connections::connect_to(&h.window, state, rt.handle(), "e2e".into(), true);
        let mut running = Self { h, _app: app, rt, redis, _turn: turn };
        running.until("the connection", |h| h.window.global::<ConnectionState>().get_view().connected);
        running.until("the walk", |h| h.window.global::<KeyTree>().get_footer().ends_with("keys scanned"));
        running
    }

    /// Runs the background work and the window until `done` holds; panics after a while.
    fn until(&mut self, what: &str, done: impl Fn(&Harness) -> bool) {
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(15) {
            self.rt.block_on(async { tokio::time::sleep(Duration::from_millis(2)).await });
            self.h.frame();
            if done(&self.h) {
                return;
            }
        }
        panic!("timed out waiting for {what}");
    }

    /// The tree's rows by full name, in their order.
    fn rows(&self) -> Vec<String> {
        self.h.window.global::<KeyTree>().get_rows().iter().map(|row| row.full_name.to_string()).collect()
    }

    /// Clicks the tree's row `name`: a folder opens or closes, a key opens in its tab.
    fn click_row(&mut self, name: &str) {
        self.h.window.global::<KeyTree>().invoke_row_clicked(name.into());
        self.h.frame();
    }

    /// Opens key `name` from the tree and waits until the key panel shows it.
    fn open_key(&mut self, name: &str) {
        self.click_row(name);
        self.until("the key to load", |h| {
            h.window.global::<KeyPanel>().get_key_id() == name && h.window.global::<KeyTree>().get_loading_key().is_empty()
        });
    }

    /// The key panel's rows: each field (or item) shown.
    fn fields(&self) -> Vec<String> {
        self.h.window.global::<KeyPanel>().get_all_values_rows().iter().map(|row| row.field.to_string()).collect()
    }

    /// Waits for the value editor to close: its write is done.
    fn editor_closes(&mut self) {
        self.until("the editor to close", |h| !h.window.global::<ValueEditor>().get_open());
    }
}
