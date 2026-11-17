//! A key's values copied or opened in an editor: one field as its row shows it, every field
//! as one array, the table's ticked rows or a cell, the key's name and a selected text.

use super::formats::show_seeded;
use super::links::{links_ready, show_connection};
use super::*;
use crate::{ExportFormat, ExportTarget, KeyView};
use serde_json::{Value, json};

/// The app showing hash `key` with `fields`, loaded.
fn shown(key: &str, fields: &[(&str, Value)]) -> (Harness, Rc<crate::app::App>, tokio::runtime::Runtime, EventLoopTurn) {
    let (mut h, app, rt, turn, _) = links_ready(&[("a", "redis://cache.test:6379")]);
    show_connection(&h, "a");
    app.key_detail.seed_hash("a", key, fields);
    show_seeded(&mut h, &app, &rt, key);
    (h, app, rt, turn)
}

fn json_of(text: &str) -> Value {
    serde_json::from_str(text).unwrap()
}

#[test]
fn a_field_is_copied_or_opened_as_its_row_shows_it() {
    let doc = json!({"name": "marker-one", "n": 1});
    let (mut h, _app, rt, _turn) = shown("export:one", &[("doc", doc.clone())]);
    let panel = h.window.global::<KeyPanel>();
    crate::toast::take_last();
    panel.invoke_copy_value("doc".into(), 0);
    let (target, text) = handed_out(&mut h, &rt, "marker-one");
    assert_eq!(target, "clipboard");
    assert_eq!(text, "{\n  \"name\": \"marker-one\",\n  \"n\": 1\n}", "pretty JSON");
    assert!(wait_for(&mut h, |_| crate::toast::take_last() == "Copied"), "the copy is confirmed");

    h.window.global::<KeyPanel>().invoke_copy_value("doc".into(), 1);
    let (_, escaped) = handed_out(&mut h, &rt, "marker-one");
    assert_eq!(json_of(json_of(&escaped).as_str().unwrap()), doc, "a JSON string holding the compact JSON");

    h.window.global::<KeyPanel>().invoke_open_value("doc".into(), 2);
    assert_eq!(handed_out(&mut h, &rt, "marker-one"), ("yaml".to_string(), "name: marker-one\nn: 1".to_string()), "as YAML");
}

#[test]
fn every_field_is_copied_as_one_array_or_opened_as_escaped_text() {
    let (mut h, _app, rt, _turn) = shown("export:all", &[("a", json!("marker-all")), ("b", json!({"x": 1}))]);
    let panel = h.window.global::<KeyPanel>();
    panel.invoke_export_all_fields(ExportFormat::Json, ExportTarget::Copy);
    let (target, text) = handed_out(&mut h, &rt, "marker-all");
    assert_eq!(target, "clipboard");
    assert_eq!(json_of(&text), json!([{"field": "a", "value": "marker-all"}, {"field": "b", "value": {"x": 1}}]));

    h.window.global::<KeyPanel>().invoke_export_all_fields(ExportFormat::Escaped, ExportTarget::Open);
    let (target, text) = handed_out(&mut h, &rt, "marker-all");
    assert_eq!(target, "txt", "escaped text opens as plain text");
    assert_eq!(json_of(json_of(&text).as_str().unwrap())[0]["value"], "marker-all");
}

/// Shows the key as a table, and waits for its rows.
fn as_table(h: &mut Harness, rt: &tokio::runtime::Runtime, rows: usize) {
    let panel = h.window.global::<KeyPanel>();
    panel.set_view(KeyView::Table);
    panel.invoke_view_changed();
    for _ in 0..500 {
        rt.block_on(async { tokio::time::sleep(std::time::Duration::from_millis(2)).await });
        h.frame();
        if h.window.global::<KeyPanel>().get_table_rows().row_count() == rows {
            return;
        }
    }
    panic!("the table never showed {rows} rows");
}

#[test]
fn the_ticked_table_rows_are_copied_as_one_array_and_a_cell_alone() {
    let orders = [
        ("1", json!({"id": 1, "total": 10, "tag": "marker-table"})),
        ("2", json!({"id": 2, "total": 25, "tag": "marker-table"})),
        ("3", json!({"id": 3, "total": 7, "tag": "marker-table"})),
    ];
    let (mut h, _app, rt, _turn) = shown("export:orders", &orders);
    as_table(&mut h, &rt, 3);
    let panel = h.window.global::<KeyPanel>();
    panel.get_table_selected().set_row_data(0, true);
    panel.get_table_selected().set_row_data(2, true);
    panel.invoke_export_selection(ExportFormat::Json, ExportTarget::Copy);
    let (_, text) = handed_out(&mut h, &rt, "marker-table");
    assert_eq!(json_of(&text), json!([orders[0].1, orders[2].1]), "the ticked rows, in the table's order");

    let columns: Vec<String> = h.window.global::<KeyPanel>().get_table_columns().iter().map(|c| c.to_string()).collect();
    let tag = columns.iter().position(|c| c == "tag").unwrap() as i32;
    h.window.global::<KeyPanel>().invoke_copy_table_cell(1, tag);
    assert_eq!(handed_out(&mut h, &rt, "marker-table"), ("clipboard".to_string(), "marker-table".to_string()));
}

#[test]
fn nothing_ticked_in_the_table_copies_nothing() {
    let (mut h, _app, rt, _turn) = shown("export:none", &[("1", json!({"id": "marker-none"})), ("2", json!({"id": 2}))]);
    as_table(&mut h, &rt, 2);
    h.window.global::<KeyPanel>().invoke_export_selection(ExportFormat::Json, ExportTarget::Copy);
    rt.block_on(async { tokio::time::sleep(std::time::Duration::from_millis(50)).await });
    h.frame();
    assert!(!crate::value_actions::HANDED.lock().iter().any(|(_, text)| text.contains("marker-none")));
}

#[test]
fn the_key_name_and_a_selected_text_are_copied_as_they_are() {
    let (mut h, _app, rt, _turn) = shown("marker:key name", &[("f", json!("v"))]);
    h.window.global::<KeyPanel>().invoke_copy_key_name();
    assert_eq!(handed_out(&mut h, &rt, "marker:key"), ("clipboard".to_string(), "marker:key name".to_string()));

    h.window.global::<KeyPanel>().invoke_copy_selection("marker-selected words".into());
    assert_eq!(handed_out(&mut h, &rt, "marker-selected").1, "marker-selected words");
}
