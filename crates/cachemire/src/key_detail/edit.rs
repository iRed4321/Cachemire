//! Editing a value: an open row's field as pretty JSON in the value editor, colored as
//! it's typed, then written back to its hash field or string key once it's valid JSON.

use std::cell::RefCell;
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;

use rust_i18n::t;
use serde_json::Value;
use slint::private_unstable_api::re_exports::StyledText;
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};

use crate::error::Localize;
use crate::{KeyPanel, MainWindow, ValueEditor};
use cachemire_core::explorer;
use cachemire_core::key_name::KeyName;
use cachemire_core::state::AppState;

use super::Ctx;
use super::color::color_for_kind;
use super::load::LoadRequest;
use super::pretty_text::{ColorSpans, ColoredParagraph};
use super::scroll::{current_scroll, set_field_expanded};

/// What the editor writes to: a key of a connection, and the hash field (`None`
/// for a string key's whole value).
#[derive(Clone)]
struct EditTarget {
    connection: String,
    key: String,
    field: Option<String>,
}

/// What a write did to the field the editor was open on.
enum Written {
    Value,
    Renamed(String),
    Deleted,
}

/// Why a write didn't happen: the server refused it, or it failed.
enum WriteError {
    Refused(String),
    Failed(cachemire_core::error::Error),
}

impl From<cachemire_core::error::Error> for WriteError {
    fn from(error: cachemire_core::error::Error) -> Self {
        WriteError::Failed(error)
    }
}

/// The end of the number starting at `at`.
fn number_end(bytes: &[u8], at: usize) -> usize {
    at + bytes[at..].iter().take_while(|b| b.is_ascii_digit() || matches!(b, b'-' | b'+' | b'.' | b'e' | b'E')).count()
}

/// The end of the string whose opening quote is at `at`: past its closing quote, or
/// at the end of its line when it isn't closed there.
fn string_end(bytes: &[u8], at: usize) -> usize {
    let mut i = at + 1;
    while i < bytes.len() && bytes[i] != b'"' && bytes[i] != b'\n' {
        i += if bytes[i] == b'\\' && bytes.get(i + 1).is_some_and(|b| *b != b'\n') { 2 } else { 1 };
    }
    if bytes.get(i) == Some(&b'"') { i + 1 } else { i.min(bytes.len()) }
}

/// The colored runs of `text`, JSON that may not be valid yet: strings (a key when a
/// `:` follows), numbers, `true`/`false`, and punctuation; anything else is left plain.
fn tokens(text: &str) -> Vec<(Range<usize>, &'static str)> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let (end, kind) = match bytes[i] {
            b'"' => {
                let end = string_end(bytes, i);
                let key = text[end..].trim_start().starts_with(':');
                (end, if key { "key" } else { "string" })
            }
            b'-' | b'0'..=b'9' => (number_end(bytes, i), "number"),
            b'{' | b'}' | b'[' | b']' | b',' | b':' => (i + 1, "punct"),
            _ if bytes[i..].starts_with(b"true") => (i + 4, "bool"),
            _ if bytes[i..].starts_with(b"false") => (i + 5, "bool"),
            _ => {
                i += 1;
                continue;
            }
        };
        out.push((i..end, kind));
        i = end;
    }
    out
}

/// `text` colored the way the value view colors JSON, a paragraph per line.
fn highlight(text: &str) -> StyledText {
    let mut runs = tokens(text).into_iter().peekable();
    let mut paragraphs: Vec<ColoredParagraph> = Vec::new();
    let mut start = 0;
    for line in text.split('\n') {
        let end = start + line.len();
        let mut spans = ColorSpans::new();
        // no run crosses a line: strings stop at the end of theirs
        while let Some((range, kind)) = runs.next_if(|(range, _)| range.start < end) {
            if let Some(color) = color_for_kind(kind) {
                spans.push((range.start - start..range.end.min(end) - start, color));
            }
        }
        paragraphs.push((line.to_string(), spans));
        start = end + 1;
    }
    StyledText::from_colored_paragraphs(paragraphs)
}

/// Why `text` isn't valid JSON, "" when it is.
fn check(text: &str) -> String {
    match serde_json::from_str::<Value>(text) {
        Ok(_) => String::new(),
        Err(e) => t!("Not valid JSON: line %{line}, column %{column}", line = e.line(), column = e.column()).into_owned(),
    }
}

/// `text` pretty-printed when it's valid JSON, as it is otherwise.
fn format(text: &str) -> String {
    match serde_json::from_str::<Value>(text) {
        Ok(value) => serde_json::to_string_pretty(&value).unwrap_or_else(|_| text.to_string()),
        Err(_) => text.to_string(),
    }
}

/// Makes `lines` the lines of `text`, changing only the rows between the lines they
/// start and end with alike, so the editor's line numbers aren't all made again.
fn update_lines(lines: &VecModel<SharedString>, text: &str) {
    let new: Vec<&str> = text.split('\n').collect();
    let old = lines.row_count();
    let same = |i: usize, j: usize| lines.row_data(i).is_some_and(|line| line.as_str() == new[j]);
    let head = (0..old.min(new.len())).take_while(|&i| same(i, i)).count();
    let tail = (0..(old - head).min(new.len() - head)).take_while(|&i| same(old - 1 - i, new.len() - 1 - i)).count();
    let (gone, added) = (old - head - tail, &new[head..new.len() - tail]);
    // past a few hundred rows, one reset beats moving the rest a row at a time
    if gone.max(added.len()) > 200 {
        lines.set_vec(new.into_iter().map(SharedString::from).collect::<Vec<_>>());
        return;
    }
    for (i, line) in added.iter().enumerate().take(gone) {
        lines.set_row_data(head + i, (*line).into());
    }
    for _ in added.len()..gone {
        lines.remove(head + added.len());
    }
    for (i, line) in added.iter().enumerate().skip(gone) {
        lines.insert(head + i, (*line).into());
    }
}

/// Whether `text` wrapped to `columns` characters takes at most `max_lines` lines.
fn fits(text: &str, columns: i32, max_lines: i32) -> bool {
    let columns = columns.max(1) as usize;
    let lines: usize = text.split('\n').map(|line| line.chars().count().div_ceil(columns).max(1)).sum();
    lines <= max_lines.max(0) as usize
}

impl Ctx {
    /// Opens the editor on field `name` of the shown key, as pretty JSON.
    fn edit_value(&self, name: &str, target: &RefCell<Option<EditTarget>>) {
        let Some(window) = self.weak.upgrade() else { return };
        let panel = window.global::<KeyPanel>();
        if !panel.get_editable() {
            return;
        }
        let field = self.shared.content.lock().loaded.iter().find(|f| f.name == name && f.too_long.is_none()).cloned();
        let Some(field) = field else { return };
        let hash = panel.get_badge() == "HASH";
        *target.borrow_mut() = Some(EditTarget {
            connection: window.connection_id().to_string(),
            key: panel.get_key_id().to_string(),
            field: hash.then(|| name.to_string()),
        });
        let editor = window.global::<ValueEditor>();
        editor.set_key_name(panel.get_name());
        editor.set_field(if hash { name.into() } else { Default::default() });
        editor.set_is_field(hash);
        editor.set_name(name.into());
        editor.set_text(serde_json::to_string_pretty(&field.value()).unwrap_or_default().into());
        editor.set_error(Default::default());
        editor.set_saving(false);
        editor.set_open(true);
    }

    /// Why the field edited can't be named `name` ("" when it can): it needs one, and
    /// not one of another field of the hash among those read.
    fn check_name(&self, name: &str, target: &RefCell<Option<EditTarget>>) -> String {
        let target = target.borrow();
        let Some((target, field)) = target.as_ref().and_then(|t| Some((t, t.field.as_deref()?))) else { return String::new() };
        if name.is_empty() {
            return t!("A field needs a name").into_owned();
        }
        if name == field {
            return String::new();
        }
        let taken = match self.shared.fetched.lock().get(&(target.connection.clone(), target.key.clone())) {
            Some(fetch) => match &fetch.all_names {
                Some(names) => names.iter().any(|n| n == name),
                None => fetch.fields.iter().any(|f| f.name == name),
            },
            None => self.shared.content.lock().loaded.iter().any(|f| f.name == name),
        };
        if taken { t!("This hash already has a field with this name").into_owned() } else { String::new() }
    }

    /// Writes `text` (valid JSON) to what the editor is open on, under the field name
    /// `name` for a hash: a JSON string as its text, anything else as compact JSON.
    fn save_value(&self, text: &str, name: &str, target: &RefCell<Option<EditTarget>>) {
        let Ok(value) = serde_json::from_str::<Value>(text) else { return };
        let Some(target) = target.borrow().clone() else { return };
        let raw = match value {
            Value::String(text) => text,
            other => other.to_string(),
        };
        let renamed = target.field.as_deref().filter(|field| *field != name).map(|_| name.to_string());
        self.run_write(target, t!("Saved").into_owned(), move |state, key, field| async move {
            let written: Result<Written, WriteError> = match (field, renamed) {
                (Some(from), Some(to)) => match explorer::rename_hash_field(&state, None, &key, &from, &to, &raw).await? {
                    true => Ok(Written::Renamed(to)),
                    false => Err(WriteError::Refused(t!("This hash already has a field with this name").into_owned())),
                },
                (field, _) => {
                    explorer::write_value(&state, None, &key, field.as_deref(), &raw).await?;
                    Ok(Written::Value)
                }
            };
            written
        });
    }

    /// Removes the hash field the editor is open on.
    fn delete_field(&self, target: &RefCell<Option<EditTarget>>) {
        let Some(target) = target.borrow().clone().filter(|t| t.field.is_some()) else { return };
        self.run_write(target, t!("Deleted").into_owned(), |state, key, field| async move {
            explorer::delete_hash_field(&state, None, &key, field.as_deref().unwrap_or_default()).await?;
            Ok(Written::Deleted)
        });
    }

    /// Runs `write` (given the app state, the key and the field) with the editor marked busy:
    /// once done, the editor closes and `done` is toasted, or it stays open with why it failed.
    fn run_write<F>(
        &self,
        target: EditTarget,
        done: String,
        write: impl FnOnce(Arc<AppState>, KeyName, Option<String>) -> F + Send + 'static,
    ) where
        F: Future<Output = Result<Written, WriteError>> + Send,
    {
        let Some(window) = self.weak.upgrade() else { return };
        window.global::<ValueEditor>().set_saving(true);
        let ctx = self.clone();
        self.rt.spawn(async move {
            let written = write(ctx.state.clone(), KeyName::from_id(&target.key), target.field.clone()).await;
            let _ = slint::invoke_from_event_loop(move || {
                let Some(window) = ctx.weak.upgrade() else { return };
                let editor = window.global::<ValueEditor>();
                editor.set_saving(false);
                match written {
                    Ok(written) => {
                        editor.set_open(false);
                        crate::toast::show(&window, &done);
                        ctx.show_written(&window, &target, written);
                    }
                    Err(WriteError::Refused(why)) => editor.set_error(why.into()),
                    Err(WriteError::Failed(e)) => editor.set_error(t!("Saving failed: %{error}", error = e.localized()).as_ref().into()),
                }
            });
        });
    }

    /// After a write to `target`: what was fetched of its key is dropped, and the key is
    /// read again if it's still shown, scrolled to the field written (if it's still there).
    fn show_written(&self, window: &MainWindow, target: &EditTarget, written: Written) {
        self.shared.fetched.lock().remove(&(target.connection.clone(), target.key.clone()));
        if window.global::<KeyPanel>().get_key_id() != target.key.as_str() {
            return;
        }
        let shown = match (written, &target.field) {
            (Written::Value, field) => field.clone(),
            (Written::Renamed(to), from) => {
                if let Some(from) = from {
                    set_field_expanded(window, from, false);
                }
                set_field_expanded(window, &to, true);
                Some(to)
            }
            (Written::Deleted, from) => {
                if let Some(from) = from {
                    set_field_expanded(window, from, false);
                }
                None
            }
        };
        if let Some(field) = shown {
            let scroll = window.global::<crate::KeyScroll>();
            scroll.set_reveal_pending(true);
            scroll.set_reveal_field(field.as_str().into());
            scroll.set_reveal_index(-1);
        }
        self.load_key(LoadRequest { key: target.key.clone(), scroll: current_scroll(window), reuse: false, announce: false });
    }
}

/// Wires the value editor and the rows' edit buttons.
pub(super) fn init(window: &MainWindow, ctx: &Ctx) {
    let target: Rc<RefCell<Option<EditTarget>>> = Rc::default();
    let editor = window.global::<ValueEditor>();
    editor.on_highlight(|text| highlight(&text));
    editor.on_check(|text| check(&text).into());
    editor.on_fits(|text, columns, max_lines| fits(&text, columns, max_lines));
    editor.on_sanitize(|text| if text.contains('\r') { text.replace('\r', "").into() } else { text });
    editor.on_format(|text| format(&text).into());
    let lines = Rc::new(VecModel::<SharedString>::default());
    editor.set_lines(ModelRc::from(lines.clone()));
    editor.on_text_changed(move |text| update_lines(&lines, &text));
    editor.on_check_name({
        let (ctx, target) = (ctx.clone(), target.clone());
        move |name| ctx.check_name(&name, &target).into()
    });
    editor.on_save({
        let (ctx, target) = (ctx.clone(), target.clone());
        move |text, name| ctx.save_value(&text, &name, &target)
    });
    editor.on_delete({
        let (ctx, target) = (ctx.clone(), target.clone());
        move || ctx.delete_field(&target)
    });
    window.global::<KeyPanel>().on_edit_value({
        let ctx = ctx.clone();
        move |field| ctx.edit_value(&field, &target)
    });
}
