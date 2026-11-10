//! Windows only: winit refuses every window move and resize once one of them didn't start
//! the OS's move loop (the button was up by the time it ran), until a WM_EXITSIZEMOVE that
//! never comes. That message is sent here before each move or resize, which clears the latch.

use slint::winit_030::WinitWindowAccessor;
use slint::winit_030::winit::dpi::PhysicalPosition;
use slint::winit_030::winit::event::{ElementState, MouseButton, WindowEvent};
use slint::winit_030::winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use slint::winit_030::winit::window::Window;
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::WindowsAndMessaging::{SendMessageW, WM_EXITSIZEMOVE};

/// main.slint's `resize-border-width` while the window isn't maximized, in logical pixels.
const RESIZE_BORDER: f64 = 5.0;

/// Clears winit's latch; does nothing when it is clear (winit then has nothing to undo).
pub fn clear(window: &Window) {
    let Ok(handle) = window.window_handle() else { return };
    if let RawWindowHandle::Win32(win32) = handle.as_raw() {
        // SAFETY: a message to this thread's own window, which winit handles synchronously
        unsafe { SendMessageW(win32.hwnd.get() as HWND, WM_EXITSIZEMOVE, 0, 0) };
    }
}

/// From the window's event hook, which runs before Slint: a left press on the resize
/// border, where Slint asks winit for a resize, clears the latch first. `cursor` keeps
/// the pointer's last position, which a press doesn't carry.
pub fn on_window_event(window: &slint::Window, event: &WindowEvent, cursor: &mut PhysicalPosition<f64>) {
    match event {
        WindowEvent::CursorMoved { position, .. } => *cursor = *position,
        WindowEvent::MouseInput { state: ElementState::Pressed, button: MouseButton::Left, .. } => {
            window.with_winit_window(|window| {
                let border = RESIZE_BORDER * window.scale_factor();
                let size = window.inner_size();
                let on_border = cursor.x < border
                    || cursor.y < border
                    || cursor.x > f64::from(size.width) - border
                    || cursor.y > f64::from(size.height) - border;
                if on_border && !window.is_maximized() {
                    clear(window);
                }
            });
        }
        _ => {}
    }
}
