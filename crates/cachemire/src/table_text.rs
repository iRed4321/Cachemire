//! What the table views (the key panel's and the query tab's) share: how a JSON cell is
//! written as text, how wide a column starts out, and one CSV row.

use serde_json::Value;

// Only a first guess at column widths (the user drags them anyway): one monospace
// character at the base font size, and a cell's horizontal padding.
const CHAR_PX: f32 = 7.5;
const CELL_PAD_PX: f32 = 20.0;
const MIN_CHARS: usize = 6;
const MAX_CHARS: usize = 40;

/// A column's starting width, in zoom-independent px, for its widest content of `chars` characters.
pub fn column_width(chars: usize) -> f32 {
    chars.clamp(MIN_CHARS, MAX_CHARS) as f32 * CHAR_PX + CELL_PAD_PX
}

/// A column's width, in zoom-independent px, for `chars` characters with no cap.
pub fn uncapped_column_width(chars: usize) -> f32 {
    chars.max(MIN_CHARS) as f32 * CHAR_PX + CELL_PAD_PX
}

/// Rough width of a cell in characters, for the first guess at column widths.
pub fn cell_chars(value: Option<&Value>) -> usize {
    match value {
        None | Some(Value::Null) => 0,
        Some(Value::String(text)) => text.chars().count(),
        Some(other) => serde_json::to_string(other).map_or(30, |text| text.chars().count()),
    }
}

/// A cell's value as text: strings as they are, null and missing empty, and
/// numbers, booleans and nested values as compact JSON.
pub fn cell_text(value: Option<&Value>) -> String {
    match value {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(text)) => text.clone(),
        Some(other) => serde_json::to_string(other).unwrap_or_default(),
    }
}

/// One CSV row (RFC 4180): fields holding a comma, quote or line break are quoted
/// with quotes doubled; lines end in CRLF.
pub fn push_csv_row<S: AsRef<str>>(out: &mut String, cells: impl IntoIterator<Item = S>) {
    for (i, cell) in cells.into_iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        let cell = cell.as_ref();
        if cell.contains(['"', ',', '\n', '\r']) {
            out.push('"');
            out.push_str(&cell.replace('"', "\"\""));
            out.push('"');
        } else {
            out.push_str(cell);
        }
    }
    out.push_str("\r\n");
}
