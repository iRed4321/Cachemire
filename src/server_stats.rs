//! Keeps the "memory used" figure next to the keyspace panel's key count
//! current for the connected server, on a timer since it changes while the
//! app just sits there.

use crate::{ConnectionState};
use crate::backend::LockExt;
use std::sync::Arc;
use std::time::Duration;

use slint::ComponentHandle;
use tokio::runtime::Handle;

use crate::backend::server_info::{self, format_bytes};
use crate::backend::state::AppState;
use crate::MainWindow;

const REFRESH_INTERVAL: Duration = Duration::from_secs(3);

pub fn init(window: &MainWindow, state: Arc<AppState>, rt: Handle) {
    let weak = window.as_weak();
    rt.spawn(async move {
        // what the window currently shows, so an unchanged reading never
        // wakes the UI thread
        let mut shown = String::new();
        loop {
            // Only poll a server actually connected this session (has an
            // open pool) — the store's "active" connection may be from a
            // previous run, and polling it would silently connect at startup.
            let active_id = state.connections.lock_recover().active_connection_id().to_string();
            let memory = if state.redis_connections.is_connected(&active_id) {
                // a transient failure leaves the last value shown
                server_info::used_memory(&state, Some(&active_id)).await.ok().map(format_bytes)
            } else {
                Some(String::new())
            };

            if let Some(memory) = memory
                && memory != shown
            {
                shown.clone_from(&memory);
                let weak = weak.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(window) = weak.upgrade() {
                        window.global::<ConnectionState>().set_memory(memory.into());
                    }
                });
            }
            tokio::time::sleep(REFRESH_INTERVAL).await;
        }
    });
}
