//! Open/download/copy: one row's own field, the table's ticked rows, or every
//! field of the key — each built off the UI thread, then opened, saved or
//! copied.

use std::sync::{Arc, Mutex};

use serde::Serialize;
use slint::{Model, Weak};
use tokio::runtime::Handle;

use crate::backend::LockExt;
use crate::value_actions::sanitize_filename;
use slint::ComponentHandle;
use crate::{ExportFormat, ExportTarget, KeyPanel, MainWindow};

use super::Ctx;
use super::field::{Field, FieldValue, Fields, find_field};
use super::table::{selected_rows_json, table_csv};

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
    let entries: Vec<FieldExportEntry> =
        fields.iter().map(|f| FieldExportEntry { field: &f.name, value: f.value() }).collect();
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
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(window) = weak.upgrade() {
                    crate::toast::show_copied(&window);
                }
            });
        }
        Err(e) => eprintln!("copy failed: {e}"),
    }
}

/// What to do with an export's text once it's built.
enum ExportAction {
    Open(&'static str),
    Download(String),
    Copy,
}

/// Builds an export's text on a blocking thread — serializing a big value
/// takes long enough to freeze the window otherwise — then opens, saves or
/// copies it.
fn run_export(rt: &Handle, weak: &Weak<MainWindow>, action: ExportAction, build: impl FnOnce() -> String + Send + 'static) {
    let weak = weak.clone();
    rt.spawn_blocking(move || {
        let text = build();
        match action {
            ExportAction::Open(extension) => {
                if let Err(e) = crate::value_actions::open_in_external_editor(&text, extension) {
                    eprintln!("open in editor failed: {e}");
                }
            }
            ExportAction::Download(filename) => {
                if let Err(e) = crate::value_actions::save_to_file(&text, &filename) {
                    eprintln!("download failed: {e}");
                }
            }
            ExportAction::Copy => copy_text(&weak, &text),
        }
    });
}

/// What the open/download/copy pills act on: the loaded fields and the key
/// they came from. A row pill and the whole-key pill differ only in which
/// text they build and where it goes.
#[derive(Clone)]
struct Exporter {
    rt: Handle,
    weak: Weak<MainWindow>,
    current: Arc<Mutex<Fields>>,
    current_key: Arc<Mutex<String>>,
}

impl Exporter {
    /// Exports one row's own field, as pretty JSON or escaped. `action` gets the
    /// field's file-name-safe name to build a download name from.
    fn field(&self, name: &str, escaped: bool, action: impl FnOnce(String) -> ExportAction) {
        let Some(field) = find_field(&self.current, name) else { return };
        let action = action(sanitize_filename(&field.name));
        let text = if escaped { escaped_json } else { pretty_json };
        run_export(&self.rt, &self.weak, action, move || text(&field));
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
        let fields: Fields = self.current.lock_recover().clone();
        let escaped = format == ExportFormat::Escaped;
        let stem = sanitize_filename(&self.current_key.lock_recover());
        let action = match target {
            ExportTarget::Open => ExportAction::Open(if escaped { "txt" } else { "json" }),
            ExportTarget::Download => ExportAction::Download(format!("{stem}.selection.{}", if escaped { "escaped.txt" } else { "json" })),
            ExportTarget::Copy => ExportAction::Copy,
        };
        run_export(&self.rt, &self.weak, action, move || {
            let rows = selected_rows_json(&fields, &indices);
            if escaped {
                serde_json::to_string(&serde_json::to_string(&rows).unwrap_or_default()).unwrap_or_default()
            } else {
                serde_json::to_string_pretty(&rows).unwrap_or_default()
            }
        });
    }

    /// Saves the table the filter leaves as a CSV file: the ticked rows when
    /// there are any, every row otherwise.
    fn table_csv(&self) {
        let fields: Fields = self.current.lock_recover().clone();
        if fields.is_empty() {
            return;
        }
        let indices = self.selected_rows();
        let stem = sanitize_filename(&self.current_key.lock_recover());
        let filename = if indices.is_empty() { format!("{stem}.csv") } else { format!("{stem}.selection.csv") };
        run_export(&self.rt, &self.weak, ExportAction::Download(filename), move || {
            table_csv(&fields, (!indices.is_empty()).then_some(indices.as_slice()))
        });
    }

    /// Exports every field of the loaded key.
    fn all_fields(&self, format: ExportFormat, target: ExportTarget) {
        let fields: Fields = self.current.lock_recover().clone();
        if fields.is_empty() {
            return;
        }
        let escaped = format == ExportFormat::Escaped;
        let stem = sanitize_filename(&self.current_key.lock_recover());
        let action = match target {
            ExportTarget::Open => ExportAction::Open(if escaped { "txt" } else { "json" }),
            ExportTarget::Download => ExportAction::Download(format!("{stem}.{}", if escaped { "escaped.txt" } else { "json" })),
            ExportTarget::Copy => ExportAction::Copy,
        };
        run_export(&self.rt, &self.weak, action, move || all_fields_json(&fields, escaped));
    }
}

/// Wires the KeyPanel's open/download/copy callbacks: one row's own field, the
/// table's ticked rows or CSV, every field of the key, and the copy buttons.
pub(super) fn init(window: &MainWindow, ctx: &Ctx) {
    let exporter = Exporter { rt: ctx.rt.clone(), weak: ctx.weak.clone(), current: ctx.shared.current.clone(), current_key: ctx.shared.current_key.clone() };
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
            exporter.field(&name, escaped, |stem| ExportAction::Download(format!("{stem}.{}", if escaped { "escaped.txt" } else { "json" })))
        }
    });
    panel.on_export_selection({
        let exporter = exporter.clone();
        move |format, target| exporter.selection(format, target)
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
mod tests {
    use super::*;
    use super::super::highlight::head_chars;
    use super::super::pretty_text::{COMPACT_PREVIEW_CHAR_BUDGET, colored_paragraphs_for};
    use serde_json::{Value, json};

    #[test]
    fn a_text_field_serializes_as_a_json_string_and_previews_only_its_head() {
        let text = "x".repeat(1000);
        let field = Field::new("f".into(), text.clone(), None);
        assert_eq!(pretty_json(&field), format!("\"{text}\""));
        assert_eq!(escaped_json(&Field::new("f".into(), "a\"b".into(), None)), r#""\"a\\\"b\"""#);

        let preview = colored_paragraphs_for(&FieldValue::Text(head_chars(&text, COMPACT_PREVIEW_CHAR_BUDGET)), false, COMPACT_PREVIEW_CHAR_BUDGET);
        assert_eq!(preview.len(), 1);
        assert_eq!(preview[0].0.chars().count(), COMPACT_PREVIEW_CHAR_BUDGET);
        assert_eq!(head_chars("héllo", 2), "hé");
    }

    #[test]
    fn every_field_is_exported_as_one_array() {
        let fields: Fields = vec![
            Arc::new(Field::new("a".into(), "plain".into(), None)),
            Arc::new(Field::new("b".into(), "{\"x\":1}".into(), serde_json::from_str("{\"x\":1}").ok())),
        ];
        let parsed: Value = serde_json::from_str(&all_fields_json(&fields, false)).unwrap();
        assert_eq!(parsed, json!([{"field": "a", "value": "plain"}, {"field": "b", "value": {"x": 1}}]));
    }
}
