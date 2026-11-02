use super::super::highlight::head_chars;
use super::super::pretty_text::{COMPACT_PREVIEW_CHAR_BUDGET, colored_paragraphs_for};
use super::super::search_filter::SearchTerm;
use super::super::test_support::field;
use super::*;
use crate::SearchScope;
use serde_json::{Value, json};

#[test]
fn a_text_field_serializes_as_a_json_string_and_previews_only_its_head() {
    let text = "x".repeat(1000);
    let field = Field::new("f".into(), text.clone(), None);
    assert_eq!(pretty_json(&field), format!("\"{text}\""));
    assert_eq!(escaped_json(&Field::new("f".into(), "a\"b".into(), None)), r#""\"a\\\"b\"""#);

    let preview =
        colored_paragraphs_for(&FieldValue::Text(head_chars(&text, COMPACT_PREVIEW_CHAR_BUDGET)), false, COMPACT_PREVIEW_CHAR_BUDGET);
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
    let escaped: String = serde_json::from_str(&all_fields_json(&fields, true)).unwrap();
    assert_eq!(serde_json::from_str::<Value>(&escaped).unwrap(), parsed, "escaped, it is the same array as a string");
}

#[test]
fn an_export_reads_in_full_only_the_values_left_out_for_their_size() {
    let fields: Fields = vec![
        Arc::new(Field::new("small".into(), "1".into(), Some(json!(1)))),
        Arc::new(Field::too_long("big".into(), 900_000)),
        Arc::new(Field::too_long("gone".into(), 800_000)),
    ];
    let read = vec![
        ("big".to_string(), Some("{\"huge\":true}".to_string())),
        ("gone".to_string(), None),
        ("small".to_string(), Some("changed".to_string())),
    ];
    let full = with_full_values(fields, read);
    assert_eq!(full[0].raw, "1", "a value read with its page is kept as it was");
    assert_eq!(full[1].json, Some(json!({"huge": true})));
    assert_eq!(full[1].too_long, None);
    assert_eq!((full[2].raw.as_str(), full[2].too_long), ("", Some(800_000)), "deleted since: still empty");
}

/// With search badges narrowing the table, a ticked row exports, and a cell copies, the row
/// the table shows at that place rather than the one among every shown field.
#[test]
fn table_exports_take_their_rows_from_the_fields_the_search_leaves() {
    let runtime = tokio::runtime::Builder::new_current_thread().build().unwrap();
    let search = SearchFilter::new(vec![SearchTerm { text: "kept".into(), scope: SearchScope::Fields }], false);
    let content = PanelContent {
        key: "k".into(),
        current: vec![field("skipped", Some(json!({"id": 1}))), field("kept", Some(json!({"id": 2})))],
        search,
        ..Default::default()
    };
    let exporter = Exporter {
        rt: runtime.handle().clone(),
        weak: Weak::default(),
        state: Arc::new(AppState::for_tests(&cachemire_core::test_support::temp_dir("table-exports"))),
        content: Arc::new(Mutex::new(content)),
    };
    let (shown, search, key) = exporter.table_source();
    assert_eq!(key, "k");
    let fields = table_fields(&shown, &search);
    assert_eq!(table_cell_copy_text(&fields, &fields, 0, 0, None).as_deref(), Some("kept"), "the table's first row");
    assert_eq!(selected_rows_json(&fields, &fields, &[0], None), json!([{"id": 2}]));
    assert_eq!(table_cell_copy_text(&fields, &fields, 0, 1, None).as_deref(), Some("2"));
    let csv = table_csv(&fields, &fields, Some(&[0]), None);
    assert!(csv.contains("kept") && !csv.contains("skipped"), "{csv}");
}

/// A value left out for its size fills one table row; read in full for an export, an array
/// there stays that one row, so the ticked rows after it are still the ones exported.
#[test]
fn a_value_read_in_full_for_an_export_keeps_the_rows_in_their_places() {
    let shown: Fields = vec![Arc::new(Field::too_long("big".into(), 900_000)), field("after", Some(json!({"id": 2})))];
    let full = with_full_values(shown.clone(), vec![("big".to_string(), Some("[{\"id\":10},{\"id\":11}]".to_string()))]);
    assert_eq!(selected_rows_json(&shown, &full, &[1], None), json!([{"id": 2}]), "the row ticked after the big one");
    assert_eq!(selected_rows_json(&shown, &full, &[0], None), json!([[{"id": 10}, {"id": 11}]]), "the big one whole");
    assert_eq!(table_csv(&shown, &full, Some(&[1]), None), "Field,id\r\nafter,2\r\n");
    assert_eq!(table_csv(&shown, &full, None, None).lines().count(), 3, "the header and the two rows shown");
}

/// Copying a cell whose row is a value left out for its size asks for that value only, and
/// copies the cell from it once read in full; the "Field" cell needs nothing read.
#[test]
fn copying_a_cell_of_a_value_left_out_reads_that_value_only() {
    let shown: Fields = vec![
        Arc::new(Field::too_long("big".into(), 900_000)),
        field("after", Some(json!({"id": 2}))),
        Arc::new(Field::too_long("other".into(), 800_000)),
    ];
    assert_eq!(table_cell_left_out(&shown, 0, 1, None).as_deref(), Some("big"));
    assert_eq!(table_cell_left_out(&shown, 0, 0, None), None, "the field's name is already there");
    assert_eq!(table_cell_left_out(&shown, 1, 1, None), None, "a value read with its page");
    let full = with_full_values(shown.clone(), vec![("big".to_string(), Some("{\"id\":10}".to_string()))]);
    assert_eq!(table_cell_copy_text(&shown, &full, 0, 1, None).as_deref(), Some("10"));
    assert_eq!(table_cell_copy_text(&shown, &shown, 0, 1, None).as_deref(), Some(""), "empty until read");
    assert_eq!(table_cell_copy_text(&shown, &full, 1, 1, None).as_deref(), Some("2"));
}
