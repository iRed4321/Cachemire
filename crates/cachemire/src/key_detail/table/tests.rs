use super::super::field::Fields;
use super::super::test_support::{field, records_field};
use super::super::value_list::{filter_fields, parse_filter};
use super::*;
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
fn only_the_first_sampled_objects_decide_the_columns() {
    let mut fields: Vec<Arc<Field>> = (0..TABLE_SAMPLE).map(|i| field(&format!("f{i}"), Some(json!({"id": i, "name": "n"})))).collect();
    // the one past the sample lacks "name", but isn't sampled
    fields.push(field("late", Some(json!({"id": 99}))));
    assert_eq!(column_titles(&fields), vec!["id", "name"]);
}

#[test]
fn a_field_holding_an_array_contributes_a_row_per_item() {
    let fields = vec![field("f1", Some(json!([{"a": 1, "b": "x"}, {"a": 2, "b": "y"}]))), field("f2", Some(json!([{"a": 3, "b": "z"}])))];
    let table = build_table(&fields, &[], false, None);
    assert_eq!(table.columns, vec!["Field", "a", "b"]);
    assert_eq!(table.rows.iter().map(|(name, _)| name.as_str()).collect::<Vec<_>>(), vec!["f1", "f1", "f2"]);
}

#[test]
fn plain_values_get_one_value_column() {
    let fields = vec![field("f1", Some(json!(["a", "b"]))), field("f2", Some(json!(3)))];
    let table = build_table(&fields, &[], false, None);
    assert_eq!(table.columns, vec!["Field", "value"]);
    assert_eq!(table.rows.len(), 3);
}

#[test]
fn a_huge_table_is_cut_off_and_says_so() {
    let big = json!((0..MAX_TABLE_ROWS + 500).map(|i| json!({"n": i})).collect::<Vec<_>>());
    let table = build_table(&[field("f", Some(big))], &[], false, None);
    assert_eq!(table.rows.len(), MAX_TABLE_ROWS);
    assert_eq!(table.note, format!("first {MAX_TABLE_ROWS} of {} rows", MAX_TABLE_ROWS + 500));
    assert!(build_table(&[field("f", Some(json!([{"n": 1}])))], &[], false, None).note.is_empty());
}

#[test]
fn value_and_a_trailing_star_lay_the_values_own_properties_out_as_columns() {
    let fields: Fields = (0..3).map(records_field).chain([field("text", None)]).collect();
    let top_level = ["Field", "field_index", "generated", "records"];
    let table_columns_of = |query: &str| {
        let (kept, _) = filter_fields(&fields, parse_filter(query).unwrap().as_ref());
        (build_table(&kept, &[], false, None).columns, kept.len())
    };

    // the value itself, and its properties: the columns the table has by default
    assert_eq!(build_table(&fields, &[], false, None).columns, top_level);
    for query in ["$", "value"] {
        assert_eq!(table_columns_of(query), (top_level.iter().map(|c| c.to_string()).collect(), 4), "{query}: text fields stay");
    }
    for query in ["*", "value.*"] {
        assert_eq!(
            table_columns_of(query),
            (top_level.iter().map(|c| c.to_string()).collect(), 3),
            "{query}: plain text has no properties"
        );
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
    assert_eq!(build_table(&fields, &[], false, None).columns, vec!["Field", "field_index", "generated", "records"]);

    // `records`: one row per record, a column per property of the records
    let (filtered, _) = filter_fields(&fields, parse_filter("records").unwrap().as_ref());
    let table = build_table(&filtered, &[], false, None);
    assert_eq!(table.columns, vec!["Field", "record_id", "label", "details"]);
    assert_eq!(table.rows.len(), 8);

    // `records.*.label`: plain values
    let (labels, _) = filter_fields(&fields, parse_filter("records.*.label").unwrap().as_ref());
    assert_eq!(build_table(&labels, &[], false, None).columns, vec!["Field", "value"]);
}

#[test]
fn the_table_exports_as_csv_with_every_row_and_proper_quoting() {
    let fields = vec![
        field("k1", Some(json!({"id": 1, "note": "plain", "tags": ["a", "b"], "ok": true}))),
        field("k,2", Some(json!({"id": 2, "note": "has \"quotes\", commas\nand a line break", "tags": [], "ok": null}))),
    ];
    assert_eq!(
        table_csv(&fields, None, None),
        "Field,id,note,tags,ok\r\n\
         k1,1,plain,\"[\"\"a\"\",\"\"b\"\"]\",true\r\n\
         \"k,2\",2,\"has \"\"quotes\"\", commas\nand a line break\",[],\r\n"
    );

    // a filter's records: one line each, under the field's name, past the table's row cap
    let big = json!((0..MAX_TABLE_ROWS + 5).map(|i| json!({"n": i})).collect::<Vec<_>>());
    let csv = table_csv(&[field("f", Some(big))], None, None);
    assert_eq!(csv.lines().count(), 1 + MAX_TABLE_ROWS + 5);
    assert!(csv.starts_with("Field,n\r\nf,0\r\n") && csv.ends_with(&format!("f,{}\r\n", MAX_TABLE_ROWS + 4)));

    assert!(table_csv(&[field("only-text", None)], None, None).is_empty());
}

#[test]
fn csv_of_the_ticked_rows_has_the_same_header_and_only_those_rows_in_table_order() {
    let fields: Fields = (0..3).map(records_field).collect();
    let (records, _) = filter_fields(&fields, parse_filter("records").unwrap().as_ref());
    // six rows (two records per field); ticking the 2nd and the 5th, in any order
    let all = table_csv(&records, None, None);
    assert_eq!(all.lines().count(), 1 + 6);
    let picked = table_csv(&records, Some(&[4, 1]), None);
    let lines: Vec<&str> = picked.lines().collect();
    assert_eq!(lines[0], all.lines().next().unwrap(), "the same header as the full export");
    assert_eq!(lines.len(), 3);
    assert!(lines[1].starts_with("field_2,20,"), "{}", lines[1]); // index 4: the first record of field_2
    assert!(lines[2].starts_with("field_0,1,"), "{}", lines[2]); // index 1
    assert_eq!(table_csv(&records, Some(&[]), None).lines().count(), 1, "an empty selection is just the header");
    assert_eq!(table_csv(&records, Some(&[99]), None).lines().count(), 1, "an index past the rows is ignored");
}

#[test]
fn selected_table_rows_become_one_json_array_in_table_order() {
    let fields: Fields = (0..3).map(records_field).chain([field("text", None)]).collect();
    // `records`: six rows, two records per field
    let (records, _) = filter_fields(&fields, parse_filter("records").unwrap().as_ref());
    let picked = selected_rows_json(&records, &[0, 3, 5], None);
    assert_eq!(
        picked,
        json!([
            {"record_id": 0, "label": "a", "details": {"priority": 1}},
            {"record_id": 11, "label": "b", "details": {"priority": 2}},
            {"record_id": 21, "label": "b", "details": {"priority": 2}},
        ])
    );
    assert_eq!(selected_rows_json(&records, &[], None), json!([]));
    assert_eq!(selected_rows_json(&records, &[99], None), json!([]), "an index past the rows is ignored");

    // rows that are fields: a JSON field is its value, a text field its text
    let with_text: Fields =
        fields.iter().take(3).cloned().chain([Arc::new(Field::new("text".into(), "plain text".into(), None))]).collect();
    let picked = selected_rows_json(&with_text, &[1, 3], None);
    assert_eq!(picked, json!([fields[1].json.clone().unwrap(), "plain text"]));
}

#[test]
fn a_table_has_a_field_column_then_one_per_key_and_a_cell_per_column() {
    let fields = vec![
        field("k1", Some(json!({"id": 1, "nested": {"a": [1, 2]}, "s": "hello"}))),
        field("k2", Some(json!({"id": 2, "nested": {"a": []}, "s": "world\nline two"}))),
        field("odd", None),
    ];
    let table = build_table(&fields, &[], false, None);
    assert_eq!(table.columns, vec!["Field", "id", "nested", "s"]);
    assert_eq!(table.widths.len(), 4);
    assert_eq!(table.rows.len(), 3);
    assert!(table.rows.iter().all(|(_, cells)| cells.len() == 3));
    assert_eq!(table.rows[2].0, "odd");
    assert!(build_table(&[field("only-text", None)], &[], false, None).columns.is_empty());
}
