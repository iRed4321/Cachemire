//! Places the transient toasts ("Copied", "Refreshed") next to the pointer, i.e. next to the
//! button that was just clicked. Slint has no way to ask where the pointer
//! is, so the last cursor position is tracked from winit's window events.

use std::cell::Cell;

use rust_i18n::t;

use slint::ComponentHandle;
use slint::winit_030::winit::event::WindowEvent;
use slint::winit_030::{EventResult, WinitWindowAccessor};

use crate::MainWindow;

thread_local! {
    // window coordinates in logical pixels; negative until the pointer has
    // been seen, which the window treats as "unknown"
    static POINTER: Cell<(f32, f32)> = const { Cell::new((-1.0, -1.0)) };
}

/// Starts tracking the pointer. Everything Slint touches runs on the UI
/// thread, which is also where `show_copied` reads the position back.
pub fn init(window: &MainWindow) {
    let weak = window.as_weak();
    window.set_device_scale(window.window().scale_factor());
    window.window().on_winit_window_event(move |window, event| {
        // (this hook is the only one a window has: it also keeps the UI's copy of the display scale current)
        let device_scale = window.scale_factor();
        if let Some(main) = weak.upgrade()
            && main.get_device_scale() != device_scale
        {
            main.set_device_scale(device_scale);
        }
        if let WindowEvent::CursorMoved { position, .. } = event {
            let scale = window.scale_factor();
            POINTER.set((position.x as f32 / scale, position.y as f32 / scale));
        }
        EventResult::Propagate
    });
}

/// Raises the "Copied" toast at the pointer. UI thread only.
pub fn show_copied(window: &MainWindow) {
    show(window, &t!("Copied"));
}

/// Raises the "Refreshed" toast at the pointer. UI thread only.
pub fn show_refreshed(window: &MainWindow) {
    show(window, &t!("Refreshed"));
}

pub fn show(window: &MainWindow, text: &str) {
    let (x, y) = POINTER.get();
    window.invoke_show_toast(text.into(), x, y);
}
