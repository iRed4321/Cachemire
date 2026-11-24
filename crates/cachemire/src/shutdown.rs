//! Windows only: the window's answer to a shutdown or restart. By default Cachemire saves its
//! session and leaves; with the setting on, Windows waits and names it as still open, until the
//! user goes on anyway (then it saves and leaves too) or cancels.

use std::cell::{Cell, RefCell};

use slint::winit_030::winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use slint::winit_030::winit::window::Window;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::Recovery::{RESTART_NO_CRASH, RESTART_NO_HANG, RegisterApplicationRestart};
use windows_sys::Win32::System::Shutdown::{ShutdownBlockReasonCreate, ShutdownBlockReasonDestroy};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallWindowProcW, ENDSESSION_CLOSEAPP, ENDSESSION_CRITICAL, GWLP_WNDPROC, SetWindowLongPtrW, WM_ENDSESSION, WM_QUERYENDSESSION, WNDPROC,
};

use crate::t;

thread_local! {
    /// The window procedure winit gave the window, which every message goes on to (0: not hooked yet).
    static PREVIOUS: Cell<isize> = const { Cell::new(0) };
    static HOLD: Cell<bool> = const { Cell::new(false) };
    /// Saves what a normal quit saves (the session, the window's place).
    static ON_END: RefCell<Option<Box<dyn Fn()>>> = const { RefCell::new(None) };
}

/// Whether a shutdown waits for the user (the setting).
pub fn set_hold(hold: bool) {
    HOLD.set(hold);
}

/// Asks Windows to start the app again when an installer closed it to update it, and after a
/// restart if the user lets Windows reopen their apps; never after a crash or a hang.
pub fn register_restart() {
    // SAFETY: no command line (a null pointer is allowed), flags only
    let _ = unsafe { RegisterApplicationRestart(std::ptr::null(), RESTART_NO_CRASH | RESTART_NO_HANG) };
}

/// What to save when the session ends with the app still open.
pub fn on_end(save: impl Fn() + 'static) {
    ON_END.with_borrow_mut(|slot| *slot = Some(Box::new(save)));
}

/// Hooks the window's procedure, once; UI thread, from the window's first event.
pub fn install(window: &Window) {
    if PREVIOUS.get() != 0 {
        return;
    }
    let Ok(handle) = window.window_handle() else { return };
    if let RawWindowHandle::Win32(win32) = handle.as_raw() {
        let hwnd = win32.hwnd.get() as HWND;
        // SAFETY: this thread's own window; `procedure` hands every message on to the previous one
        let previous = unsafe { SetWindowLongPtrW(hwnd, GWLP_WNDPROC, procedure as usize as isize) };
        PREVIOUS.set(previous);
    }
}

unsafe extern "system" fn procedure(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match message {
        WM_QUERYENDSESSION => {
            // never held: a critical shutdown, nor an installer closing the app to update it
            if !HOLD.get() || (lparam as u32) & (ENDSESSION_CRITICAL | ENDSESSION_CLOSEAPP) != 0 {
                return 1;
            }
            let reason: Vec<u16> = t!("Cachemire is still open").encode_utf16().chain([0]).collect();
            // SAFETY: a window of this thread, and a nul-terminated text that outlives the call
            unsafe { ShutdownBlockReasonCreate(hwnd, reason.as_ptr()) };
            return 0;
        }
        WM_ENDSESSION => {
            // SAFETY: a window of this thread; nothing to undo when no reason was set
            unsafe { ShutdownBlockReasonDestroy(hwnd) };
            if wparam != 0 {
                ON_END.with_borrow(|save| {
                    if let Some(save) = save {
                        save();
                    }
                });
                std::process::exit(0);
            }
            return 0;
        }
        _ => {}
    }
    // SAFETY: PREVIOUS is the window procedure this one replaced, a valid WNDPROC
    let previous: WNDPROC = unsafe { std::mem::transmute::<isize, WNDPROC>(PREVIOUS.get()) };
    // SAFETY: the message is handed on unchanged, to the procedure of this window
    unsafe { CallWindowProcW(previous, hwnd, message, wparam, lparam) }
}
