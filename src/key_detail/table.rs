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
mod tests {
    use super::*;
    use super::super::field::Fields;
    use super::super::test_support::{field, records_field};
    use super::super::value_list::{filter_fields, parse_filter};
    use serde_json::json;

    fn column_titles(fields: &[Arc<Field>]) -> Vec<String> {
        table_columns(&table_source_rows(fields)).iter().map(|c| c.title().to_string()).collect()
    }

    #[test]
    fn columns_are_the_top_level_keys_every_sampled_object_has() {
        let fields = vec![
            field("a", Some(json!({"id": 1, "name": "x", "tags": [1], "extra": true}))),
            field("plain", None), // not an object: not part of the sample
            field("b", Some(json!({"name": "y", "id": 2, "tags": []}))),
            field("c", Some(json!({"id": 3, "name": "z"}))),
        ];
        assert_eq!(column_titles(&fields), vec!["id", "name"]);
        assert!(column_titles(&[field("only-text", None)]).is_empty());
        assert!(column_titles(&[]).is_empty());
    }

    #[test]
    fn only_the_first_ten_objects_decide_the_columns() {
        let mut fields: Vec<Arc<Field>> = (0..10).map(|i| field(&format!("f{i}"), Some(json!({"id": i, "name": "n"})))).collect();
        // the 11th lacks "name", but isn't sampled
        fields.push(field("late", Some(json!({"id": 99}))));
        assert_eq!(column_titles(&fields), vec!["id", "name"]);
    }

    #[test]
    fn a_field_holding_an_array_contributes_a_row_per_item() {
        let fields = vec![
            field("f1", Some(json!([{"a": 1, "b": "x"}, {"a": 2, "b": "y"}]))),
            field("f2", Some(json!([{"a": 3, "b": "z"}]))),
        ];
        let table = build_table(&fields, &[], false);
        assert_eq!(table.columns, vec!["Field", "a", "b"]);
        assert_eq!(table.rows.iter().map(|(name, _)| name.as_str()).collect::<Vec<_>>(), vec!["f1", "f1", "f2"]);
    }

    #[test]
    fn plain_values_get_one_value_column() {
        let fields = vec![field("f1", Some(json!(["a", "b"]))), field("f2", Some(json!(3)))];
        let table = build_table(&fields, &[], false);
        assert_eq!(table.columns, vec!["Field", "value"]);
        assert_eq!(table.rows.len(), 3);
    }

    #[test]
    fn a_huge_table_is_cut_off_and_says_so() {
        let big = json!((0..MAX_TABLE_ROWS + 500).map(|i| json!({"n": i})).collect::<Vec<_>>());
        let table = build_table(&[field("f", Some(big))], &[], false);
        assert_eq!(table.rows.len(), MAX_TABLE_ROWS);
        assert_eq!(table.note, format!("first {MAX_TABLE_ROWS} of {} rows", MAX_TABLE_ROWS + 500));
        assert!(build_table(&[field("f", Some(json!([{"n": 1}])))], &[], false).note.is_empty());
    }

    #[test]
    fn value_and_a_trailing_star_lay_the_values_own_properties_out_as_columns() {
        let fields: Fields = (0..3).map(records_field).chain([field("text", None)]).collect();
        let top_level = ["Field", "field_index", "generated", "records"];
        let table_columns_of = |query: &str| {
            let (kept, _) = filter_fields(&fields, parse_filter(query).unwrap().as_ref());
            (build_table(&kept, &[], false).columns, kept.len())
        };

        // the value itself, and its properties: the columns the table has by default
        assert_eq!(build_table(&fields, &[], false).columns, top_level);
        for query in ["$", "value"] {
            assert_eq!(table_columns_of(query), (top_level.iter().map(|c| c.to_string()).collect(), 4), "{query}: text fields stay");
        }
        for query in ["*", "value.*"] {
            assert_eq!(table_columns_of(query), (top_level.iter().map(|c| c.to_string()).collect(), 3), "{query}: plain text has no properties");
        }

        // `value.` is optional in front of a path, and `.*` on an array lists its items
        let records = ["Field", "record_id", "label", "details"];
        for query in ["records", "value.records", "records.*", "value.records.*"] {
            let (columns, kept) = table_columns_of(query);
            assert_eq!((columns, kept), (records.iter().map(|c| c.to_string()).collect(), 3), "{query}");
        }
        assert_eq!(table_columns_of("value.nothing").1, 0);

        // a real key called `value` wins over the alias
        let real = field("v", Some(json!({"value": {"a": 1}, "b": 2})));
        let (kept, _) = filter_fields(&[real], parse_filter("value").unwrap().as_ref());
        assert_eq!(kept[0].json, Some(json!({"a": 1})));
    }

    #[test]
    fn filtering_to_an_array_of_records_makes_a_column_per_record_property() {
        let fields: Fields = (0..4).map(records_field).collect();
        // unfiltered: the fields' own top-level keys
        assert_eq!(build_table(&fields, &[], false).columns, vec!["Field", "field_index", "generated", "records"]);

        // `records`: one row per record, a column per property of the records
        let (filtered, _) = filter_fields(&fields, parse_filter("records").unwrap().as_ref());
        let table = build_table(&filtered, &[], false);
        assert_eq!(table.columns, vec!["Field", "record_id", "label", "details"]);
        assert_eq!(table.rows.len(), 8);

        // `records.*.label`: plain values
        let (labels, _) = filter_fields(&fields, parse_filter("records.*.label").unwrap().as_ref());
        assert_eq!(build_table(&labels, &[], false).columns, vec!["Field", "value"]);
    }

    #[test]
    fn the_table_exports_as_csv_with_every_row_and_proper_quoting() {
        let fields = vec![
            field("k1", Some(json!({"id": 1, "note": "plain", "tags": ["a", "b"], "ok": true}))),
            field("k,2", Some(json!({"id": 2, "note": "has \"quotes\", commas\nand a line break", "tags": [], "ok": null}))),
        ];
        assert_eq!(
            table_csv(&fields, None),
            "Field,id,note,tags,ok\r\n\
             k1,1,plain,\"[\"\"a\"\",\"\"b\"\"]\",true\r\n\
             \"k,2\",2,\"has \"\"quotes\"\", commas\nand a line break\",[],\r\n"
        );

        // a filter's records: one line each, under the field's name, past the table's row cap
        let big = json!((0..MAX_TABLE_ROWS + 5).map(|i| json!({"n": i})).collect::<Vec<_>>());
        let csv = table_csv(&[field("f", Some(big))], None);
        assert_eq!(csv.lines().count(), 1 + MAX_TABLE_ROWS + 5);
        assert!(csv.starts_with("Field,n\r\nf,0\r\n") && csv.ends_with(&format!("f,{}\r\n", MAX_TABLE_ROWS + 4)));

        assert!(table_csv(&[field("only-text", None)], None).is_empty());
    }

    #[test]
    fn csv_of_the_ticked_rows_has_the_same_header_and_only_those_rows_in_table_order() {
        let fields: Fields = (0..3).map(records_field).collect();
        let (records, _) = filter_fields(&fields, parse_filter("records").unwrap().as_ref());
        // six rows (two records per field); ticking the 2nd and the 5th, in any order
        let all = table_csv(&records, None);
        assert_eq!(all.lines().count(), 1 + 6);
        let picked = table_csv(&records, Some(&[4, 1]));
        let lines: Vec<&str> = picked.lines().collect();
        assert_eq!(lines[0], all.lines().next().unwrap(), "the same header as the full export");
        assert_eq!(lines.len(), 3);
        assert!(lines[1].starts_with("field_2,20,"), "{}", lines[1]); // index 4: the first record of field_2
        assert!(lines[2].starts_with("field_0,1,"), "{}", lines[2]); // index 1
        assert_eq!(table_csv(&records, Some(&[])).lines().count(), 1, "an empty selection is just the header");
        assert_eq!(table_csv(&records, Some(&[99])).lines().count(), 1, "an index past the rows is ignored");
    }

    #[test]
    fn selected_table_rows_become_one_json_array_in_table_order() {
        let fields: Fields = (0..3).map(records_field).chain([field("text", None)]).collect();
        // `records`: six rows, two records per field
        let (records, _) = filter_fields(&fields, parse_filter("records").unwrap().as_ref());
        let picked = selected_rows_json(&records, &[0, 3, 5]);
        assert_eq!(
            picked,
            json!([
                {"record_id": 0, "label": "a", "details": {"priority": 1}},
                {"record_id": 11, "label": "b", "details": {"priority": 2}},
                {"record_id": 21, "label": "b", "details": {"priority": 2}},
            ])
        );
        assert_eq!(selected_rows_json(&records, &[]), json!([]));
        assert_eq!(selected_rows_json(&records, &[99]), json!([]), "an index past the rows is ignored");

        // rows that are fields: a JSON field is its value, a text field its text
        let with_text: Fields = fields.iter().take(3).cloned().chain([Arc::new(Field::new("text".into(), "plain text".into(), None))]).collect();
        let picked = selected_rows_json(&with_text, &[1, 3]);
        assert_eq!(picked, json!([fields[1].json.clone().unwrap(), "plain text"]));
    }

    #[test]
    fn a_table_has_a_field_column_then_one_per_key_and_a_cell_per_column() {
        let fields = vec![
            field("k1", Some(json!({"id": 1, "nested": {"a": [1, 2]}, "s": "hello"}))),
            field("k2", Some(json!({"id": 2, "nested": {"a": []}, "s": "world\nline two"}))),
            field("odd", None),
        ];
        let table = build_table(&fields, &[], false);
        assert_eq!(table.columns, vec!["Field", "id", "nested", "s"]);
        assert_eq!(table.widths.len(), 4);
        assert_eq!(table.rows.len(), 3);
        assert!(table.rows.iter().all(|(_, cells)| cells.len() == 3));
        assert_eq!(table.rows[2].0, "odd");
        assert!(build_table(&[field("only-text", None)], &[], false).columns.is_empty());
    }
}
