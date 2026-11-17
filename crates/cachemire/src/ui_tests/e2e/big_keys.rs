//! Hashes on a real server shown a page at a time: scrolling adds pages, a search reads the
//! rest, the Field arrow flips the order, a value too big for its page is read on demand; and
//! the table view, its sorting and its ticked rows.

use super::*;
use crate::KeyView;

const FIELDS: usize = 450;

/// Hash `big`: `FIELDS` fields `f000`... holding `v`, except `f420`, which holds "needle".
fn big(redis: &mut TestRedis) {
    let mut args: Vec<String> = vec!["HSET".into(), "big".into()];
    for i in 0..FIELDS {
        args.push(format!("f{i:03}"));
        args.push(if i == 420 { "needle".into() } else { "v".into() });
    }
    redis.run(&args.iter().map(String::as_str).collect::<Vec<_>>());
}

fn near_end(r: &mut Running) {
    r.h.window.global::<KeyPanel>().invoke_near_end();
}

#[test]
#[ignore = "needs a Redis server: cargo xtask e2e"]
fn a_big_hash_shows_a_page_and_scrolling_near_its_end_adds_the_next() {
    let mut r = Running::start(big);
    r.open_key("big");
    assert_eq!(r.fields().len(), 200, "a page of fields");
    assert_eq!(r.fields()[0], "f000");

    near_end(&mut r);
    r.until("the second page", |h| h.window.global::<KeyPanel>().get_all_values_rows().row_count() == 400);
    near_end(&mut r);
    r.until("the last page", |h| h.window.global::<KeyPanel>().get_all_values_rows().row_count() == FIELDS);
    assert_eq!(r.fields().last().map(String::as_str), Some("f449"));
    near_end(&mut r);
    r.rt.block_on(async { tokio::time::sleep(Duration::from_millis(100)).await });
    r.h.frame();
    assert_eq!(r.fields().len(), FIELDS, "past the end there is nothing more");
}

#[test]
#[ignore = "needs a Redis server: cargo xtask e2e"]
fn a_search_reads_through_the_pages_not_loaded_yet() {
    let mut r = Running::start(big);
    r.open_key("big");
    let panel = r.h.window.global::<KeyPanel>();
    panel.set_filter_query("needle".into());
    panel.invoke_filter_query_submitted();
    r.until("the match on the third page", |h| {
        h.window.global::<KeyPanel>().get_all_values_rows().iter().map(|row| row.field.to_string()).collect::<Vec<_>>() == ["f420"]
    });
}

#[test]
#[ignore = "needs a Redis server: cargo xtask e2e"]
fn the_field_arrow_reads_the_hash_again_from_its_other_end() {
    let mut r = Running::start(big);
    r.open_key("big");
    r.h.window.global::<KeyPanel>().invoke_sort_toggled();
    assert!(r.h.window.global::<KeyPanel>().get_sort_descending());
    r.until("the hash from its end", |h| {
        h.window.global::<KeyPanel>().get_all_values_rows().row_data(0).is_some_and(|row| row.field == "f449")
    });
    assert_eq!(r.fields().len(), 200);
}

/// Hash `files` with two fields too big to be read with their page, and a small one.
fn files(redis: &mut TestRedis) {
    let huge = "x".repeat(cachemire_core::explorer::MAX_FIELD_BYTES + 1000);
    redis.run(&["HSET", "files", "a.bin", &huge, "b.bin", &huge, "readme", "small"]);
}

fn too_long(r: &Running, field: &str) -> String {
    let rows = r.h.window.global::<KeyPanel>().get_all_values_rows();
    rows.iter().find(|row| row.field == field).map(|row| row.too_long.to_string()).unwrap_or_default()
}

#[test]
#[ignore = "needs a Redis server: cargo xtask e2e"]
fn a_value_too_big_for_its_page_shows_its_size_until_it_is_asked_for() {
    let mut r = Running::start(files);
    r.open_key("files");
    assert!(!too_long(&r, "a.bin").is_empty(), "its size is shown instead");
    assert!(too_long(&r, "readme").is_empty());

    r.h.window.global::<KeyPanel>().invoke_load_value("a.bin".into());
    r.until("the value read", |h| {
        h.window.global::<KeyPanel>().get_all_values_rows().iter().any(|row| row.field == "a.bin" && row.too_long.is_empty())
    });
    let row = r.h.window.global::<KeyPanel>().get_all_values_rows().iter().find(|row| row.field == "a.bin").unwrap();
    assert!(row.initial_expanded && !row.loading, "its row opens on it");

    // deleted by someone else meanwhile: said so, and the row stays as it was
    r.redis.run(&["HDEL", "files", "b.bin"]);
    crate::toast::take_last();
    r.h.window.global::<KeyPanel>().invoke_load_value("b.bin".into());
    r.until("the answer", |h| {
        h.window.global::<KeyPanel>().get_all_values_rows().iter().any(|row| row.field == "b.bin" && !row.loading)
            && !crate::toast::take_last().is_empty()
    });
    assert!(!too_long(&r, "b.bin").is_empty());
}

#[test]
#[ignore = "needs a Redis server: cargo xtask e2e"]
fn refresh_shows_what_another_client_wrote_to_the_key() {
    let mut r = Running::start(|redis| redis.run(&["HSET", "user:1", "name", "Ada"]));
    r.click_row("user:");
    r.open_key("user:1");
    r.redis.run(&["HSET", "user:1", "city", "London"]);
    r.h.window.global::<KeyPanel>().invoke_refresh_key();
    r.until("the new field", |h| h.window.global::<KeyPanel>().get_all_values_rows().row_count() == 2);
    assert_eq!(r.fields(), ["city", "name"]);
}

/// Hash `orders`: each field a JSON order.
fn orders(redis: &mut TestRedis) {
    redis.run(&["HSET", "orders", "1", r#"{"id":1,"total":10}"#, "2", r#"{"id":2,"total":25}"#, "3", r#"{"id":3,"total":7}"#]);
}

/// The table's rows, by field, in their order.
fn table_order(h: &Harness) -> Vec<String> {
    h.window.global::<KeyPanel>().get_table_rows().iter().map(|row| row.field.to_string()).collect()
}

fn show_table(r: &mut Running) {
    r.h.window.global::<KeyPanel>().set_view(KeyView::Table);
    r.h.window.global::<KeyPanel>().invoke_view_changed();
    r.until("the table", |h| h.window.global::<KeyPanel>().get_table_rows().row_count() == 3);
}

#[test]
#[ignore = "needs a Redis server: cargo xtask e2e"]
fn the_table_view_lays_json_values_out_in_columns_and_sorts_by_one() {
    let mut r = Running::start(orders);
    r.open_key("orders");
    show_table(&mut r);
    let columns: Vec<String> = r.h.window.global::<KeyPanel>().get_table_columns().iter().map(|c| c.to_string()).collect();
    assert_eq!(columns, ["Field", "id", "total"]);
    assert_eq!(table_order(&r.h), ["1", "2", "3"]);

    let total = columns.iter().position(|c| c == "total").unwrap() as i32;
    r.h.window.global::<KeyPanel>().invoke_table_sort_requested(total, true);
    r.until("the sorted table", |h| table_order(h) == ["2", "1", "3"]);
    r.h.window.global::<KeyPanel>().invoke_table_sort_requested(total, false);
    r.until("the other way", |h| table_order(h) == ["3", "1", "2"]);

    // the Field column flips the hash's own order: it is read again
    r.h.window.global::<KeyPanel>().invoke_table_sort_requested(0, true);
    assert!(r.h.window.global::<KeyPanel>().get_sort_descending());
    r.until("the hash from its end", |h| table_order(h) == ["3", "2", "1"]);
}

#[test]
#[ignore = "needs a Redis server: cargo xtask e2e"]
fn shift_click_ticks_a_range_of_table_rows_and_clear_unticks_them() {
    let mut r = Running::start(orders);
    r.open_key("orders");
    show_table(&mut r);
    let panel = r.h.window.global::<KeyPanel>();
    let selected = |panel: &KeyPanel| panel.get_table_selected().iter().collect::<Vec<bool>>();
    panel.get_table_selected().set_row_data(0, true);
    panel.invoke_table_select_range(0, 2);
    assert_eq!(selected(&panel), [true, true, true], "the range takes the first row's state");

    panel.invoke_table_clear_selection();
    assert_eq!(selected(&panel), [false, false, false]);
}

#[test]
#[ignore = "needs a Redis server: cargo xtask e2e"]
fn a_path_typed_lists_the_keys_the_values_hold() {
    let mut r = Running::start(orders);
    r.open_key("orders");
    let panel = r.h.window.global::<KeyPanel>();
    assert!(panel.invoke_suggest("$.".into(), 2));
    let keys: Vec<String> = panel.get_suggestions().iter().map(|row| format!("{}{}{}", row.before, row.matched, row.after)).collect();
    assert_eq!(keys, ["*", "id", "total"], "every item, then each key");
    assert!(panel.invoke_suggest("$.to".into(), 4));
    assert_eq!(panel.get_suggestions().row_count(), 1);

    let edit = panel.invoke_complete_suggestion("$.to".into(), 4, 0);
    assert_eq!((edit.text.as_str(), edit.cursor), ("$.total", 7), "Tab writes the key");
}
