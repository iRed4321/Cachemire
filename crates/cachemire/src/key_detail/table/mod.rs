//! Table view: the fields as rows, the top-level keys they share as columns
//! (see `build_table`), plus CSV/JSON export of those rows.

use std::cmp::Ordering;
use std::sync::Arc;

use rayon::prelude::*;
use rust_i18n::t;
use serde_json::Value;
use slint::private_unstable_api::re_exports::StyledText;
use slint::{Model, ModelRc, SharedString, VecModel};

use crate::table_text::{cell_chars, cell_text, column_width, push_csv_row, uncapped_column_width};
use crate::{KeyPanel, MainWindow, TableRowData};
use slint::ComponentHandle;

use super::color::color_for_kind;
use super::field::{Field, Fields};
use super::highlight::{head_chars, highlight_needle_matches};
use super::pretty_text::{ColoredParagraph, MAX_SHOWN_STRING_BYTES, colored_paragraphs_for};
use super::search_filter::SearchFilter;

/// How many rows decide the columns: the first ones that are JSON objects.
const TABLE_SAMPLE: usize = 100;
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
/// What a sortable column's ordering triangle takes from its title, in zoom-independent px.
const SORT_TRIANGLE_PX: f32 = 24.0;

pub(super) struct TableData {
    /// "Field" first, then one per key column
    columns: Vec<String>,
    /// per column, in zoom-independent px
    widths: Vec<f32>,
    /// per column, how much wider than `widths` it would like to be, if the view has room
    wants: Vec<f32>,
    /// per column, whether the rows can be ordered by it (its cells are plain strings, numbers or booleans)
    sortable: Vec<bool>,
    /// the order the rows are in, `None` for the fields' own
    sort: Option<ColumnSort>,
    /// per row: its field's name, and one cell for each column after "Field"
    rows: Vec<(String, Vec<StyledText>)>,
    /// said in the filter bar when the table doesn't show every row
    note: String,
}

impl TableData {
    fn empty() -> Self {
        Self {
            columns: Vec::new(),
            widths: Vec::new(),
            wants: Vec::new(),
            sortable: Vec::new(),
            sort: None,
            rows: Vec::new(),
            note: String::new(),
        }
    }
}

/// The fields a table is made of: those of `fields` the search bar's badges in `filter` leave.
/// The table and every export of its rows go through here, so their row indices agree.
pub(super) fn table_fields(fields: &[Arc<Field>], filter: &SearchFilter) -> Fields {
    fields.par_iter().filter(|f| filter.matches(f)).cloned().collect()
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

/// The order asked for a table's rows: by the column with this title (`None` for "Field"),
/// naturally, ascending or descending; kept by title so it follows the column when the filter changes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct TableSort {
    column: Option<String>,
    descending: bool,
}

/// The order the window's table headers ask for, `None` when no column is picked.
pub(super) fn table_sort(window: &MainWindow) -> Option<TableSort> {
    let panel = window.global::<KeyPanel>();
    let index = usize::try_from(panel.get_table_sort_column()).ok()?;
    let column = match index {
        0 => None,
        _ => Some(panel.get_table_columns().row_data(index)?.to_string()),
    };
    Some(TableSort { column, descending: panel.get_table_sort_descending() })
}

/// A [`TableSort`] once its column is found among the table's: its index (0 is "Field").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ColumnSort {
    column: usize,
    descending: bool,
}

/// What a row is ordered by in a column; a row without a plain value there goes last.
enum SortKey<'a> {
    Number(f64),
    Bool(bool),
    Text(&'a str),
    Empty,
}

impl SortKey<'_> {
    fn rank(&self) -> u8 {
        match self {
            SortKey::Number(_) => 0,
            SortKey::Bool(_) => 1,
            SortKey::Text(_) => 2,
            SortKey::Empty => 3,
        }
    }

    /// Numbers by value, booleans false first, strings in natural order (`a2` before `a10`);
    /// different kinds in that order.
    fn compare(&self, other: &SortKey) -> Ordering {
        match (self, other) {
            (SortKey::Number(a), SortKey::Number(b)) => a.total_cmp(b),
            (SortKey::Bool(a), SortKey::Bool(b)) => a.cmp(b),
            (SortKey::Text(a), SortKey::Text(b)) => natord::compare(a, b),
            _ => self.rank().cmp(&other.rank()),
        }
    }
}

fn sort_key<'a>(value: Option<&'a Value>) -> SortKey<'a> {
    match value {
        Some(Value::Number(number)) => number.as_f64().map_or(SortKey::Empty, SortKey::Number),
        Some(Value::Bool(flag)) => SortKey::Bool(*flag),
        Some(Value::String(text)) => SortKey::Text(text),
        _ => SortKey::Empty,
    }
}

/// What `row` is ordered by under `sort`.
fn row_sort_key<'a>(row: &TableSourceRow<'a>, columns: &[Column], sort: ColumnSort) -> SortKey<'a> {
    let (field, value) = *row;
    match sort.column.checked_sub(1) {
        None => SortKey::Text(&field.name),
        Some(i) => sort_key(columns.get(i).and_then(|column| column.cell(value))),
    }
}

/// Orders `rows` by `sort`, keeping the fields' own order among equal ones.
fn sort_rows(rows: &mut [TableSourceRow], columns: &[Column], sort: ColumnSort) {
    rows.sort_by(|a, b| match (row_sort_key(a, columns, sort), row_sort_key(b, columns, sort)) {
        (SortKey::Empty, SortKey::Empty) => Ordering::Equal,
        (SortKey::Empty, _) => Ordering::Greater,
        (_, SortKey::Empty) => Ordering::Less,
        (a, b) if sort.descending => b.compare(&a),
        (a, b) => a.compare(&b),
    });
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

    /// Whether the rows can be ordered by this column: the first [`TABLE_SAMPLE`] cells that
    /// hold something are all plain strings, numbers or booleans, and there is one.
    fn sortable(&self, rows: &[TableSourceRow]) -> bool {
        let mut cells = rows.iter().filter_map(|(_, value)| self.cell(*value)).filter(|cell| !cell.is_null()).take(TABLE_SAMPLE).peekable();
        cells.peek().is_some() && cells.all(|cell| matches!(cell, Value::String(_) | Value::Number(_) | Value::Bool(_)))
    }
}

/// The rows and columns of the table over `fields`, with the columns that can be sorted
/// (index 0 is "Field") and the order the rows are put in: `sort`, unless its column is gone or can't be.
struct TableLayout<'a> {
    rows: Vec<TableSourceRow<'a>>,
    columns: Vec<Column>,
    sortable: Vec<bool>,
    sort: Option<ColumnSort>,
}

fn table_layout(fields: &[Arc<Field>], sort: Option<TableSort>) -> TableLayout<'_> {
    let mut rows = table_source_rows(fields);
    let columns = table_columns(&rows);
    let sortable: Vec<bool> = std::iter::once(true).chain(columns.iter().map(|column| column.sortable(&rows))).collect();
    let sort = sort.and_then(|TableSort { column, descending }| {
        let column = match column {
            None => 0,
            Some(title) => columns.iter().position(|c| c.title() == title)? + 1,
        };
        sortable[column].then_some(ColumnSort { column, descending })
    });
    if let Some(sort) = sort {
        sort_rows(&mut rows, &columns, sort);
    }
    TableLayout { rows, columns, sortable, sort }
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
    keys.into_iter().map(Column::Key).collect()
}

/// One cell, always a single line: strings in the string color (line breaks become spaces,
/// one too long to show a placeholder), numbers and booleans as in the value view, nested
/// values as compact colored JSON; the search badges' matches are highlighted.
fn table_cell_text(value: Option<&Value>, needles: &[String], case_sensitive: bool) -> StyledText {
    let paragraphs: Vec<ColoredParagraph> = match value {
        None => vec![(String::new(), Vec::new())],
        Some(Value::String(text)) if text.len() <= MAX_SHOWN_STRING_BYTES => {
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
/// empty when there's nothing to tabulate, with its rows in `sort`'s order (ignored if
/// that column can't be sorted). Cell coloring is spread across cores; `needles` are
/// highlighted in every cell, case-sensitively per `case_sensitive`.
pub(super) fn build_table(fields: &[Arc<Field>], needles: &[String], case_sensitive: bool, sort: Option<TableSort>) -> TableData {
    let TableLayout { mut rows, columns, sortable, sort } = table_layout(fields, sort);
    let total_rows = rows.len();
    rows.truncate(MAX_TABLE_ROWS);
    if columns.is_empty() {
        return TableData::empty();
    }

    let cells: Vec<(String, Vec<StyledText>)> = rows
        .par_iter()
        .map(|(field, value)| {
            (field.name.clone(), columns.iter().map(|column| table_cell_text(column.cell(*value), needles, case_sensitive)).collect())
        })
        .collect();

    let mut titles = Vec::with_capacity(columns.len() + 1);
    titles.push(t!("Field").into_owned());
    titles.extend(columns.iter().map(|column| column.title().to_string()));

    let mut widths = Vec::with_capacity(columns.len() + 1);
    let mut wants = Vec::with_capacity(columns.len() + 1);
    let field_chars = rows.iter().take(TABLE_WIDTH_ROWS).map(|(field, _)| field.name.chars().count()).max().unwrap_or(0);
    widths.push(column_width(field_chars).max(title_width(&titles[0], sortable[0])));
    wants.push(0.0);
    for (i, column) in columns.iter().enumerate() {
        let lengths: Vec<usize> =
            rows.iter().take(TABLE_WIDTH_ROWS).map(|(_, value)| cell_chars(column.cell(*value))).filter(|&chars| chars > 0).collect();
        let (width, want) = column_sizes(lengths, title_width(column.title(), sortable[i + 1]));
        widths.push(width);
        wants.push(want);
    }

    let note = if total_rows > rows.len() {
        t!("first %{shown} of %{total} rows", shown = rows.len(), total = total_rows).into_owned()
    } else {
        String::new()
    };
    TableData { columns: titles, widths, wants, sortable, sort, rows: cells, note }
}

/// The width a column's header needs for `title`, and for its ordering triangle when `sortable`.
fn title_width(title: &str, sortable: bool) -> f32 {
    column_width(title.chars().count()) + if sortable { SORT_TRIANGLE_PX } else { 0.0 }
}

/// A column's starting width (that of its typical cell within the usual bounds, or `title_width`
/// if wider) and how much wider its longest cell would like it, from the lengths of its non-empty cells.
fn column_sizes(mut lengths: Vec<usize>, title_width: f32) -> (f32, f32) {
    lengths.sort_unstable();
    let typical = lengths.get((lengths.len().saturating_sub(1) as f32 * 0.9).round() as usize).copied().unwrap_or(0);
    let longest = lengths.last().copied().unwrap_or(0);
    let width = column_width(typical).max(title_width);
    let wanted = uncapped_column_width(longest.min(TABLE_WANT_CHARS));
    (width, (wanted - width).max(0.0))
}

/// The table `fields` make (same columns as [`build_table`]) as CSV: every
/// row, not just [`MAX_TABLE_ROWS`] — or, with `only`, just those indices,
/// in order. Rows are in `sort`'s order. Empty when there's nothing to tabulate.
pub(super) fn table_csv(fields: &[Arc<Field>], only: Option<&[usize]>, sort: Option<TableSort>) -> String {
    let TableLayout { rows, columns, .. } = table_layout(fields, sort);
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
/// shows them in, per `sort`), each as its JSON, in one array — the "copy selection" text.
pub(super) fn selected_rows_json(fields: &[Arc<Field>], indices: &[usize], sort: Option<TableSort>) -> Value {
    let TableLayout { mut rows, .. } = table_layout(fields, sort);
    rows.truncate(MAX_TABLE_ROWS);
    Value::Array(indices.iter().filter_map(|&i| rows.get(i)).map(table_row_json).collect())
}

/// The cell at `row` and `column` (0 is "Field") of the table `fields` make, in `sort`'s
/// order, as text: the field's name, or the cell's value whole (see [`cell_text`]).
pub(super) fn table_cell_copy_text(fields: &[Arc<Field>], row: usize, column: usize, sort: Option<TableSort>) -> Option<String> {
    let TableLayout { rows, columns, .. } = table_layout(fields, sort);
    let (field, value) = *rows.get(row)?;
    match column.checked_sub(1) {
        None => Some(field.name.clone()),
        Some(i) => Some(cell_text(columns.get(i)?.cell(value))),
    }
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
    window.global::<KeyPanel>().set_table_sortable(ModelRc::new(VecModel::from(table.sortable)));
    window.global::<KeyPanel>().set_table_sort_column(table.sort.map_or(-1, |sort| sort.column as i32));
    window.global::<KeyPanel>().set_table_sort_descending(table.sort.is_some_and(|sort| sort.descending));
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
