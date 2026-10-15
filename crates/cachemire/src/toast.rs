//! Places the transient toasts ("Copied", "Refreshed") next to the pointer, i.e. next to the
//! button that was just clicked. Slint has no way to ask where the pointer
//! is, so the last cursor position is tracked from winit's window events.

use std::cell::Cell;

use rust_i18n::t;

use slint::ComponentHandle;
use slint::winit_030::winit::event::WindowEvent;

use crate::MainWindow;

thread_local! {
    // window coordinates in logical pixels; negative until the pointer has
    // been seen, which the window treats as "unknown"
    static POINTER: Cell<(f32, f32)> = const { Cell::new((-1.0, -1.0)) };
}

/// Gives the UI its first copy of the display scale.
pub fn init(window: &MainWindow) {
    window.set_device_scale(window.window().scale_factor());
}

/// Keeps the UI's copy of the display scale and the pointer position current, from the
/// window's winit events (main.rs's hook). UI thread, where `show_copied` reads it back.
pub fn on_window_event(main: &MainWindow, window: &slint::Window, event: &WindowEvent) {
    let device_scale = window.scale_factor();
    if main.get_device_scale() != device_scale {
        main.set_device_scale(device_scale);
    }
    if let WindowEvent::CursorMoved { position, .. } = event {
        let scale = window.scale_factor();
        POINTER.set((position.x as f32 / scale, position.y as f32 / scale));
    }
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
