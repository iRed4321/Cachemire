//! Table view: the fields as rows, the top-level keys they share as columns
//! (see `build_table`), plus CSV/JSON export of those rows.

use std::sync::Arc;

use rayon::prelude::*;
use rust_i18n::t;
use serde_json::Value;
use slint::private_unstable_api::re_exports::StyledText;
use slint::{Model, ModelRc, SharedString, VecModel};

use crate::table_text::{cell_chars, cell_text, column_width, push_csv_row, uncapped_column_width};
use slint::ComponentHandle;
use crate::{KeyPanel, MainWindow, TableRowData};

use super::color::color_for_kind;
use super::field::Field;
use super::highlight::{head_chars, highlight_needle_matches};
use super::pretty_text::{ColoredParagraph, colored_paragraphs_for};

/// How many rows decide the columns: the first ones that are JSON objects.
const TABLE_SAMPLE: usize = 10;
/// A value with hundreds of keys would mean hundreds of columns of cells.
const MAX_TABLE_COLUMNS: usize = 30;
/// Rows a table shows at most: a filter that opens up arrays can produce far
/// more than are worth building cells for.
pub(super) const MAX_TABLE_ROWS: usize = 3000;
/// Characters a single cell holds at most: a cell is one line, clipped at its
/// column's edge, so this only bounds the work for a huge value.
const TABLE_CELL_BUDGET: usize = 400;
/// Rows looked at when guessing widths: the first ones.
const TABLE_WIDTH_ROWS: usize = 100;
/// The most characters a column asks for beyond its starting width.
const TABLE_WANT_CHARS: usize = 200;

pub(super) struct TableData {
    /// "Field" first, then one per key column
    columns: Vec<String>,
    /// per column, in zoom-independent px
    widths: Vec<f32>,
    /// per column, how much wider than `widths` it would like to be, if the view has room
    wants: Vec<f32>,
    /// per row: its field's name, and one cell for each column after "Field"
    rows: Vec<(String, Vec<StyledText>)>,
    /// said in the filter bar when the table doesn't show every row
    note: String,
}

impl TableData {
    fn empty() -> Self {
        Self { columns: Vec::new(), widths: Vec::new(), wants: Vec::new(), rows: Vec::new(), note: String::new() }
    }
}

/// One table row before its cells exist: the field it came from, and the value
/// its cells are read from — `None` for a field that isn't JSON.
type TableSourceRow<'a> = (&'a Field, Option<&'a Value>);

/// The rows of a table over `fields`: one per field, except that a field whose
/// value is an array contributes one per item (under the field's name) — so
/// filtering down to an array of records, `records`, lists those records.
fn table_source_rows(fields: &[Arc<Field>]) -> Vec<TableSourceRow<'_>> {
    let mut rows = Vec::new();
    for field in fields {
        match &field.json {
            Some(Value::Array(items)) if !items.is_empty() => {
                rows.extend(items.iter().map(|item| (field.as_ref(), Some(item))));
            }
            json => rows.push((field.as_ref(), json.as_ref())),
        }
    }
    rows
}

/// A table column: a key of the rows that are objects, or — when no row is an
/// object (a filter like `records.*.label` leaves plain values) — the rows'
/// values themselves.
enum Column {
    Key(String),
    Whole,
}

impl Column {
    fn title(&self) -> &str {
        match self {
            Column::Key(key) => key,
            Column::Whole => "value",
        }
    }

    fn cell<'a>(&self, row: Option<&'a Value>) -> Option<&'a Value> {
        match (self, row) {
            (Column::Key(key), Some(Value::Object(object))) => object.get(key),
            (Column::Whole, value) => value,
            _ => None,
        }
    }
}

/// The columns for `rows`: the top-level keys present in every one of the
/// first [`TABLE_SAMPLE`] object rows, in the order the first has them; or the
/// single "value" column when there are rows but none is an object.
fn table_columns(rows: &[TableSourceRow]) -> Vec<Column> {
    let mut objects = rows.iter().filter_map(|(_, value)| match value {
        Some(Value::Object(object)) => Some(object),
        _ => None,
    });
    let Some(first) = objects.next() else {
        return if rows.iter().any(|(_, value)| value.is_some()) { vec![Column::Whole] } else { Vec::new() };
    };
    let mut keys: Vec<String> = first.keys().cloned().collect();
    for object in objects.take(TABLE_SAMPLE - 1) {
        keys.retain(|key| object.contains_key(key));
    }
    keys.truncate(MAX_TABLE_COLUMNS);
    keys.into_iter().map(Column::Key).collect()
}

/// One cell, always a single line: strings in the string color (line breaks become
/// spaces), numbers and booleans as in the value view, nested values as compact
/// colored JSON; the search badges' matches are highlighted.
fn table_cell_text(value: Option<&Value>, needles: &[String], case_sensitive: bool) -> StyledText {
    let paragraphs: Vec<ColoredParagraph> = match value {
        None => vec![(String::new(), Vec::new())],
        Some(Value::String(text)) => {
            let line: String = head_chars(text, TABLE_CELL_BUDGET).replace(['\n', '\r'], " ");
            let spans = color_for_kind("string").map(|color| vec![(0..line.len(), color)]).unwrap_or_default();
            vec![(line, spans)]
        }
        Some(Value::Null) => vec![("null".to_string(), Vec::new())],
        Some(other) => colored_paragraphs_for(other, false, TABLE_CELL_BUDGET),
    };
    highlight_needle_matches(paragraphs, needles, case_sensitive)
}

/// Builds the table for `fields` (see [`table_source_rows`] and [`table_columns`]),
/// empty when there's nothing to tabulate. Cell coloring is spread across cores;
/// `needles` are highlighted in every cell, case-sensitively per `case_sensitive`.
pub(super) fn build_table(fields: &[Arc<Field>], needles: &[String], case_sensitive: bool) -> TableData {
    let mut rows = table_source_rows(fields);
    let total_rows = rows.len();
    rows.truncate(MAX_TABLE_ROWS);
    let columns = table_columns(&rows);
    if columns.is_empty() {
        return TableData::empty();
    }

    let cells: Vec<(String, Vec<StyledText>)> = rows
        .par_iter()
        .map(|(field, value)| {
            (field.name.clone(), columns.iter().map(|column| table_cell_text(column.cell(*value), needles, case_sensitive)).collect())
        })
        .collect();

    let mut widths = Vec::with_capacity(columns.len() + 1);
    let mut wants = Vec::with_capacity(columns.len() + 1);
    let field_chars = rows.iter().take(TABLE_WIDTH_ROWS).map(|(field, _)| field.name.chars().count()).max().unwrap_or(0);
    widths.push(column_width(field_chars.max("Field".len())));
    wants.push(0.0);
    for column in &columns {
        let lengths: Vec<usize> = rows
            .iter()
            .take(TABLE_WIDTH_ROWS)
            .map(|(_, value)| cell_chars(column.cell(*value)))
            .filter(|&chars| chars > 0)
            .collect();
        let (width, want) = column_sizes(lengths, column.title().chars().count());
        widths.push(width);
        wants.push(want);
    }

    let mut titles = Vec::with_capacity(columns.len() + 1);
    titles.push(t!("Field").into_owned());
    titles.extend(columns.iter().map(|column| column.title().to_string()));
    let note = if total_rows > rows.len() { t!("first %{shown} of %{total} rows", shown = rows.len(), total = total_rows).into_owned() } else { String::new() };
    TableData { columns: titles, widths, wants, rows: cells, note }
}

/// A column's starting width (that of its typical cell, or its title if wider, within the usual
/// bounds) and how much wider its longest cell would like it, from the lengths of its non-empty cells.
fn column_sizes(mut lengths: Vec<usize>, title_chars: usize) -> (f32, f32) {
    lengths.sort_unstable();
    let typical = lengths.get((lengths.len().saturating_sub(1) as f32 * 0.9).round() as usize).copied().unwrap_or(0);
    let longest = lengths.last().copied().unwrap_or(0);
    let width = column_width(typical.max(title_chars));
    let wanted = uncapped_column_width(longest.min(TABLE_WANT_CHARS));
    (width, (wanted - width).max(0.0))
}

/// The table `fields` make (same columns as [`build_table`]) as CSV: every
/// row, not just [`MAX_TABLE_ROWS`] — or, with `only`, just those indices,
/// in order. Empty when there's nothing to tabulate.
pub(super) fn table_csv(fields: &[Arc<Field>], only: Option<&[usize]>) -> String {
    let rows = table_source_rows(fields);
    let columns = table_columns(&rows);
    if columns.is_empty() {
        return String::new();
    }
    let mut out = String::new();
    push_csv_row(&mut out, std::iter::once(t!("Field").into_owned()).chain(columns.iter().map(|c| c.title().to_string())));
    let mut push_row = |(field, value): &TableSourceRow| {
        push_csv_row(&mut out, std::iter::once(field.name.clone()).chain(columns.iter().map(|c| cell_text(c.cell(*value)))));
    };
    match only {
        Some(indices) => indices.iter().filter_map(|&i| rows.get(i)).for_each(&mut push_row),
        None => rows.iter().for_each(&mut push_row),
    }
    out
}

/// A table row as JSON, for copying: the row's value, or a non-JSON field's text.
fn table_row_json(row: &TableSourceRow) -> Value {
    match row {
        (_, Some(value)) => (*value).clone(),
        (field, None) => Value::String(field.raw.clone()),
    }
}

/// The rows at `indices` of the table `fields` make (the same order the table
/// shows them in), each as its JSON, in one array — the "copy selection" text.
pub(super) fn selected_rows_json(fields: &[Arc<Field>], indices: &[usize]) -> Value {
    let mut rows = table_source_rows(fields);
    rows.truncate(MAX_TABLE_ROWS);
    Value::Array(indices.iter().filter_map(|&i| rows.get(i)).map(table_row_json).collect())
}

pub(super) fn apply_table(window: &MainWindow, table: TableData) {
    let total: f32 = table.widths.iter().sum();
    let total_want: f32 = table.wants.iter().sum();
    let columns: Vec<SharedString> = table.columns.into_iter().map(SharedString::from).collect();
    let rows: Vec<TableRowData> = table
        .rows
        .into_iter()
        .map(|(field, cells)| TableRowData { field: field.into(), cells: ModelRc::new(VecModel::from(cells)) })
        .collect();
    window.global::<KeyPanel>().set_table_note(table.note.into());
    // a new table starts with nothing selected
    reset_table_selection(window, rows.len());
    window.global::<KeyPanel>().set_table_columns(ModelRc::new(VecModel::from(columns)));
    window.global::<KeyPanel>().set_table_col_widths(ModelRc::new(VecModel::from(table.widths)));
    window.global::<KeyPanel>().set_table_total_w(total);
    window.global::<KeyPanel>().set_table_col_wants(ModelRc::new(VecModel::from(table.wants)));
    window.global::<KeyPanel>().set_table_total_want(total_want);
    window.global::<KeyPanel>().set_table_rows(ModelRc::new(VecModel::from(rows)));
}

/// Nothing selected, for a table of `rows` rows.
pub(super) fn reset_table_selection(window: &MainWindow, rows: usize) {
    window.global::<KeyPanel>().set_table_selected(ModelRc::new(VecModel::from(vec![false; rows])));
    window.global::<KeyPanel>().set_table_selected_count(0);
}

/// Sets the rows from `from` to `to` (either order) to the state row `from` is in.
pub(super) fn select_table_range(window: &MainWindow, from: i32, to: i32) {
    let selected = window.global::<KeyPanel>().get_table_selected();
    let (Ok(from), Ok(to)) = (usize::try_from(from), usize::try_from(to)) else { return };
    let Some(on) = selected.row_data(from) else { return };
    for row in from.min(to)..=from.max(to).min(selected.row_count().saturating_sub(1)) {
        selected.set_row_data(row, on);
    }
    let count = selected.iter().filter(|&row| row).count();
    window.global::<KeyPanel>().set_table_selected_count(count as i32);
}

/// Empties the table view's data (it isn't kept while the view is off, or
/// when no key is loaded).
pub fn clear_table(window: &MainWindow) {
    apply_table(window, TableData::empty());
}

#[cfg(test)]
mod tests;
