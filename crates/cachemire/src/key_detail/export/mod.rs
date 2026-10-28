//! Open/download/copy: one row's own field, the table's ticked rows, or every
//! field of the key — each built off the UI thread, then opened, saved or
//! copied.

use parking_lot::Mutex;
use std::sync::Arc;

use rustc_hash::FxHashMap;
use serde::Serialize;
use slint::{Model, Weak};
use tokio::runtime::Handle;

use crate::value_actions::sanitize_filename;
use crate::{ExportFormat, ExportTarget, KeyPanel, MainWindow};
use cachemire_core::explorer;
use cachemire_core::key_name::KeyName;
use cachemire_core::state::AppState;
use slint::ComponentHandle;

use super::field::{Field, FieldValue, Fields};
use super::search_filter::SearchFilter;
use super::table::{selected_rows_json, table_cell_copy_text, table_csv, table_fields, table_sort};
use super::{Ctx, PanelContent};

/// A field's value as pretty (indented) JSON.
pub(super) fn pretty_json(field: &Field) -> String {
    serde_json::to_string_pretty(&field.value()).unwrap_or_default()
}

/// A field's compact JSON, re-escaped as a JSON string literal.
pub(super) fn escaped_json(field: &Field) -> String {
    let compact = serde_json::to_string(&field.value()).unwrap_or_default();
    serde_json::to_string(&compact).unwrap_or_default()
}

/// Borrowed view of one field for `all_fields_json`, so exporting every field
/// never copies a single name or value, however large.
#[derive(Serialize)]
struct FieldExportEntry<'a> {
    field: &'a str,
    value: FieldValue<'a>,
}

/// The whole key as one JSON array (`[{"field": ..., "value": ...}]`) for the key
/// header's export pill; `escaped` gives it compact, as a JSON string literal.
fn all_fields_json(fields: &[Arc<Field>], escaped: bool) -> String {
    let entries: Vec<FieldExportEntry> = fields.iter().map(|f| FieldExportEntry { field: &f.name, value: f.value() }).collect();
    if escaped {
        serde_json::to_string(&serde_json::to_string(&entries).unwrap_or_default()).unwrap_or_default()
    } else {
        serde_json::to_string_pretty(&entries).unwrap_or_default()
    }
}

/// Copies to the clipboard and raises the window's transient "Copied" toast
/// so every copy pill in the UI gives the same visible confirmation. Fine to
/// call from any thread.
fn copy_text(weak: &Weak<MainWindow>, text: &str) {
    match crate::value_actions::copy_to_clipboard(text) {
        Ok(()) => {
            let weak = weak.clone();
            let _ = weak.upgrade_in_event_loop(move |window| {
                crate::toast::show_copied(&window);
            });
        }
        Err(e) => tracing::warn!("copy failed: {e}"),
    }
}

/// What to do with an export's text once it's built.
enum ExportAction {
    Open(&'static str),
    Download(String),
    Copy,
}

/// `fields` with the full values `read` (by name) in place of those left out for
/// their size; a field gone from the server since stays empty.
fn with_full_values(fields: Fields, read: Vec<(String, Option<String>)>) -> Fields {
    let mut read: FxHashMap<String, String> = read.into_iter().filter_map(|(name, value)| Some((name, value?))).collect();
    if read.is_empty() {
        return fields;
    }
    fields
        .into_iter()
        .map(|field| match field.too_long.and_then(|_| read.remove(&field.name)) {
            Some(raw) => {
                let json = serde_json::from_str(&raw).ok();
                Arc::new(Field::new(field.name.clone(), raw, json))
            }
            None => field,
        })
        .collect()
}

/// Builds an export's text from `fields` (narrowed by `search` to the table's) on a blocking
/// thread, then opens, saves or copies it. A download asks where to save first; the values
/// of `key` left out for their size are then read in full.
fn run_export(
    exporter: &Exporter,
    key: String,
    fields: Fields,
    search: Option<SearchFilter>,
    action: ExportAction,
    build: impl FnOnce(Fields) -> String + Send + 'static,
) {
    let weak = exporter.weak.clone();
    let state = exporter.state.clone();
    exporter.rt.spawn(async move {
        let save_to = match &action {
            ExportAction::Download(filename) => match rfd::AsyncFileDialog::new().set_file_name(filename).save_file().await {
                Some(file) => Some(file.path().to_path_buf()),
                None => return,
            },
            _ => None,
        };
        let fields = match search {
            Some(filter) => match tokio::task::spawn_blocking(move || table_fields(&fields, &filter)).await {
                Ok(fields) => fields,
                Err(_) => return,
            },
            None => fields,
        };
        let unread: Vec<String> = fields.iter().filter(|f| f.too_long.is_some()).map(|f| f.name.clone()).collect();
        let read = if unread.is_empty() {
            Vec::new()
        } else {
            match explorer::hash_full_values(&state, None, &KeyName::from_id(&key), &unread).await {
                Ok(values) => unread.into_iter().zip(values).collect(),
                Err(e) => {
                    tracing::warn!("reading the values left out for their size failed: {e}");
                    return;
                }
            }
        };
        let _ = tokio::task::spawn_blocking(move || {
            let text = build(with_full_values(fields, read));
            match (action, save_to) {
                (ExportAction::Open(extension), _) => {
                    if let Err(e) = crate::value_actions::open_in_external_editor(&text, extension) {
                        tracing::warn!("open in editor failed: {e}");
                    }
                }
                (ExportAction::Download(_), Some(path)) => {
                    if let Err(e) = std::fs::write(&path, text) {
                        tracing::warn!("download to {} failed: {e}", path.display());
                    }
                }
                (ExportAction::Download(_), None) => {}
                (ExportAction::Copy, _) => copy_text(&weak, &text),
            }
        })
        .await;
    });
}

/// What the open/download/copy pills act on: the loaded fields and the key
/// they came from. A row pill and the whole-key pill differ only in which
/// text they build and where it goes.
#[derive(Clone)]
struct Exporter {
    rt: Handle,
    weak: Weak<MainWindow>,
    state: Arc<AppState>,
    content: Arc<Mutex<PanelContent>>,
}

impl Exporter {
    /// The fields shown, and their key.
    fn shown(&self) -> (Fields, String) {
        let content = self.content.lock();
        (content.current.clone(), content.key.clone())
    }

    /// The fields shown, the search bar's badges that narrow them to the table's rows
    /// (see [`table_fields`]), and their key.
    fn table_source(&self) -> (Fields, SearchFilter, String) {
        let content = self.content.lock();
        (content.current.clone(), content.search.clone(), content.key.clone())
    }

    /// Exports one row's own field, as pretty JSON or escaped. `action` gets the
    /// field's file-name-safe name to build a download name from.
    fn field(&self, name: &str, escaped: bool, action: impl FnOnce(String) -> ExportAction) {
        let (field, key) = {
            let content = self.content.lock();
            let Some(field) = content.field(name) else { return };
            (field, content.key.clone())
        };
        let action = action(sanitize_filename(&field.name));
        let text = if escaped { escaped_json } else { pretty_json };
        run_export(self, key, vec![field], None, action, move |fields| fields.first().map(|field| text(field)).unwrap_or_default());
    }

    /// The indices of the table's ticked rows, in table order.
    fn selected_rows(&self) -> Vec<usize> {
        let Some(window) = self.weak.upgrade() else { return Vec::new() };
        window.global::<KeyPanel>().get_table_selected().iter().enumerate().filter(|(_, on)| *on).map(|(i, _)| i).collect()
    }

    /// The selected table rows as one JSON array, in `format` (pretty, or that
    /// array's compact text as a JSON string), then opened, saved or copied.
    fn selection(&self, format: ExportFormat, target: ExportTarget) {
        let indices = self.selected_rows();
        if indices.is_empty() {
            return;
        }
        let (fields, search, key) = self.table_source();
        let sort = self.weak.upgrade().and_then(|window| table_sort(&window));
        let stem = sanitize_filename(&key);
        let escaped = format == ExportFormat::Escaped;
        let action = match target {
            ExportTarget::Open => ExportAction::Open(if escaped { "txt" } else { "json" }),
            ExportTarget::Download => ExportAction::Download(format!("{stem}.selection.{}", if escaped { "escaped.txt" } else { "json" })),
            ExportTarget::Copy => ExportAction::Copy,
        };
        run_export(self, key, fields, Some(search), action, move |fields| {
            let rows = selected_rows_json(&fields, &indices, sort);
            if escaped {
                serde_json::to_string(&serde_json::to_string(&rows).unwrap_or_default()).unwrap_or_default()
            } else {
                serde_json::to_string_pretty(&rows).unwrap_or_default()
            }
        });
    }

    /// Copies the table's cell at `row` and `column` (0 is "Field") whole, off the UI thread:
    /// the table is laid out again from the same fields it was built from.
    fn table_cell(&self, row: usize, column: usize) {
        let (fields, search, _) = self.table_source();
        let sort = self.weak.upgrade().and_then(|window| table_sort(&window));
        let weak = self.weak.clone();
        self.rt.spawn_blocking(move || {
            let fields = table_fields(&fields, &search);
            if let Some(text) = table_cell_copy_text(&fields, row, column, sort).filter(|text| !text.is_empty()) {
                copy_text(&weak, &text);
            }
        });
    }

    /// Saves the table the filter leaves as a CSV file: the ticked rows when
    /// there are any, every row otherwise.
    fn table_csv(&self) {
        let (fields, search, key) = self.table_source();
        if fields.is_empty() {
            return;
        }
        let stem = sanitize_filename(&key);
        let indices = self.selected_rows();
        let sort = self.weak.upgrade().and_then(|window| table_sort(&window));
        let filename = if indices.is_empty() { format!("{stem}.csv") } else { format!("{stem}.selection.csv") };
        run_export(self, key, fields, Some(search), ExportAction::Download(filename), move |fields| {
            table_csv(&fields, (!indices.is_empty()).then_some(indices.as_slice()), sort)
        });
    }

    /// Exports every field of the loaded key.
    fn all_fields(&self, format: ExportFormat, target: ExportTarget) {
        let (fields, key) = self.shown();
        if fields.is_empty() {
            return;
        }
        let stem = sanitize_filename(&key);
        let escaped = format == ExportFormat::Escaped;
        let action = match target {
            ExportTarget::Open => ExportAction::Open(if escaped { "txt" } else { "json" }),
            ExportTarget::Download => ExportAction::Download(format!("{stem}.{}", if escaped { "escaped.txt" } else { "json" })),
            ExportTarget::Copy => ExportAction::Copy,
        };
        run_export(self, key, fields, None, action, move |fields| all_fields_json(&fields, escaped));
    }
}

/// Wires the KeyPanel's open/download/copy callbacks: one row's own field, the
/// table's ticked rows or CSV, every field of the key, and the copy buttons.
pub(super) fn init(window: &MainWindow, ctx: &Ctx) {
    let exporter = Exporter { rt: ctx.rt.clone(), weak: ctx.weak.clone(), state: ctx.state.clone(), content: ctx.shared.content.clone() };
    let panel = window.global::<KeyPanel>();

    panel.on_open_value({
        let exporter = exporter.clone();
        move |name, escaped| exporter.field(&name, escaped, |_| ExportAction::Open(if escaped { "txt" } else { "json" }))
    });
    panel.on_copy_value({
        let exporter = exporter.clone();
        move |name, escaped| exporter.field(&name, escaped, |_| ExportAction::Copy)
    });
    panel.on_download_value({
        let exporter = exporter.clone();
        move |name, escaped| {
            exporter
                .field(&name, escaped, |stem| ExportAction::Download(format!("{stem}.{}", if escaped { "escaped.txt" } else { "json" })))
        }
    });
    panel.on_export_selection({
        let exporter = exporter.clone();
        move |format, target| exporter.selection(format, target)
    });
    panel.on_copy_table_cell({
        let exporter = exporter.clone();
        move |row, column| {
            if let (Ok(row), Ok(column)) = (usize::try_from(row), usize::try_from(column)) {
                exporter.table_cell(row, column);
            }
        }
    });
    panel.on_export_table_csv({
        let exporter = exporter.clone();
        move || exporter.table_csv()
    });
    panel.on_export_all_fields(move |format, target| exporter.all_fields(format, target));

    // Right-click → "Copy selection": the text comes straight from the
    // StyledText's own `selected-text` (same plain text Ctrl+C would copy).
    panel.on_copy_selection({
        let weak = ctx.weak.clone();
        move |text| {
            if !text.is_empty() {
                copy_text(&weak, &text);
            }
        }
    });

    // header copy button: the full key name of whatever's loaded. The button is
    // only shown once a key is loaded, but guard anyway so the placeholder text
    // can't end up on the clipboard.
    panel.on_copy_key_name({
        let weak = ctx.weak.clone();
        move || {
            let Some(window) = weak.upgrade() else { return };
            if !window.global::<KeyPanel>().get_loaded() {
                return;
            }
            copy_text(&weak, &window.global::<KeyPanel>().get_name());
        }
    });
}

#[cfg(test)]
mod tests;
