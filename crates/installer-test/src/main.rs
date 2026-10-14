//! A stand-in for Cachemire, packed in the real MSI to try installing, updating and removing it:
//! a message box with the app's icon and the version it was built as, and a file in the app's
//! data folder, for the uninstaller's "delete my data" checkbox.
#![cfg_attr(windows, windows_subsystem = "windows")]

const VERSION: &str = match option_env!("CACHEMIRE_VERSION") {
    Some(version) => version,
    None => env!("CARGO_PKG_VERSION"),
};

fn main() {
    if let Some(app_data) = std::env::var_os("APPDATA") {
        let dir = std::path::Path::new(&app_data).join("Cachemire");
        let _ = std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(dir.join("installer-test.txt"), VERSION));
    }
    let text = format!("Installer test build of Cachemire {VERSION}.\n\nThis is not the real app: it only stands in for it, to try the installer.");
    show(&text);
}

#[cfg(windows)]
fn show(text: &str) {
    use std::ffi::c_void;

    // MSGBOXPARAMSW
    #[repr(C)]
    struct Params {
        size: u32,
        owner: *mut c_void,
        instance: *mut c_void,
        text: *const u16,
        caption: *const u16,
        style: u32,
        icon: *const u16,
        help_id: usize,
        callback: *const c_void,
        language: u32,
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetModuleHandleW(name: *const u16) -> *mut c_void;
    }
    #[link(name = "user32")]
    unsafe extern "system" {
        fn MessageBoxIndirectW(params: *const Params) -> i32;
    }
    const MB_USERICON: u32 = 0x80;

    let wide = |s: &str| s.encode_utf16().chain([0]).collect::<Vec<u16>>();
    let (text, caption) = (wide(text), wide("Cachemire"));
    let params = Params {
        size: size_of::<Params>() as u32,
        owner: std::ptr::null_mut(),
        // SAFETY: a null name asks for the running exe's own module, always loaded
        instance: unsafe { GetModuleHandleW(std::ptr::null()) },
        text: text.as_ptr(),
        caption: caption.as_ptr(),
        style: MB_USERICON,
        // MAKEINTRESOURCE(1): the exe's icon resource (see build.rs)
        icon: std::ptr::without_provenance(1),
        help_id: 0,
        callback: std::ptr::null(),
        language: 0,
    };
    // SAFETY: the strings and params outlive the call, which returns once the box is closed
    unsafe { MessageBoxIndirectW(&params) };
}

#[cfg(not(windows))]
fn show(text: &str) {
    println!("{text}");
}
