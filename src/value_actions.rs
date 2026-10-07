//! Generic "do something with this text" actions backing the JSON viewer's
//! toolbar: open in the system's default editor for the file type, save to
//! a chosen file, or copy to the clipboard. None of this is Redis-specific.

use std::io::Write;

use crate::backend::error::AppError;
use crate::backend::now_ms;

/// Writes `text` to a temp file and opens it in a real editor — not via
/// `xdg-open`: MIME sniffing often routes JSON-looking content to a
/// browser regardless of extension. Tries known GUI editors, falling back to the OS default.
pub fn open_in_external_editor(text: &str, extension: &str) -> Result<(), AppError> {
    let path = std::env::temp_dir().join(format!("cachemire-value-{}.{extension}", now_ms()));
    std::fs::write(&path, text).map_err(|e| AppError::File(e.to_string()))?;

    #[cfg(target_os = "windows")]
    const EDITORS: &[&str] = &["code", "notepad++", "notepad"];
    #[cfg(unix)]
    const EDITORS: &[&str] = &[
        "code", "codium", "zed", "subl", "gedit", "kate", "gnome-text-editor", "xed", "featherpad", "mousepad",
        "leafpad", "notepadqq",
    ];

    for editor in EDITORS {
        if std::process::Command::new(editor).arg(&path).spawn().is_ok() {
            return Ok(());
        }
    }
    open::that_detached(&path).map_err(|e| AppError::System(e.to_string()))
}

/// Opens a native "Save As" dialog pre-filled with `suggested_name`, then
/// writes `text` to wherever the user picked. Does nothing (not an error) if
/// the user cancels the dialog.
pub fn save_to_file(text: &str, suggested_name: &str) -> Result<(), AppError> {
    let Some(path) = rfd::FileDialog::new().set_file_name(suggested_name).save_file() else {
        return Ok(());
    };
    let mut file = std::fs::File::create(&path).map_err(|e| AppError::File(e.to_string()))?;
    file.write_all(text.as_bytes()).map_err(|e| AppError::File(e.to_string()))
}

fn clipboard_error(error: arboard::Error) -> AppError {
    AppError::System(error.to_string())
}

/// On X11 the app itself serves clipboard contents, so `Clipboard` must
/// outlive the copy — kept as one shared instance for the process. Wayland
/// uses arboard's `wayland-data-control` backend; other platforms need neither.
#[cfg(unix)]
pub fn copy_to_clipboard(text: &str) -> Result<(), AppError> {
    use std::sync::Mutex;

    static CLIPBOARD: Mutex<Option<arboard::Clipboard>> = Mutex::new(None);

    let mut guard = CLIPBOARD.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if guard.is_none() {
        *guard = Some(arboard::Clipboard::new().map_err(clipboard_error)?);
    }
    guard.as_mut().expect("set above").set_text(text).map_err(clipboard_error)
}

#[cfg(not(unix))]
pub fn copy_to_clipboard(text: &str) -> Result<(), AppError> {
    let mut clipboard = arboard::Clipboard::new().map_err(clipboard_error)?;
    clipboard.set_text(text).map_err(clipboard_error)
}

/// Whether the clipboard currently holds text: a right-click "Paste" row in a
/// text box shows only then (unlike a copy, a fresh `Clipboard` per read needs
/// nothing kept alive afterward on any platform, so there's no X11 split here).
pub fn clipboard_has_text() -> bool {
    arboard::Clipboard::new().and_then(|mut c| c.get_text()).is_ok_and(|text| !text.is_empty())
}

/// `name` made safe as a downloaded file's stem: anything but a letter,
/// digit, `-` or `_` becomes `_`; empty falls back to `value`, and a name
/// Windows reserves (`CON`, `COM1`, …) gets a leading `_`.
pub fn sanitize_filename(name: &str) -> String {
    const RESERVED: &[&str] = &["CON", "PRN", "AUX", "NUL"];
    let cleaned: String = name.chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
    let upper = cleaned.to_ascii_uppercase();
    let numbered = upper.len() == 4
        && (upper.starts_with("COM") || upper.starts_with("LPT"))
        && upper.as_bytes()[3].is_ascii_digit();
    if cleaned.is_empty() {
        "value".to_string()
    } else if RESERVED.contains(&upper.as_str()) || numbered {
        format!("_{cleaned}")
    } else {
        cleaned
    }
}
