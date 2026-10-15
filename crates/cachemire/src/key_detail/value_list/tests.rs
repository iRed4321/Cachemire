use super::*;
use super::super::test_support::{field, records_field};
use serde_json::json;

#[test]
fn a_row_starts_open_only_when_its_field_is_in_the_expanded_set() {
    let fields = vec![field("a", None), field("b", None), field("c", None)];
    let expanded = FxHashSet::from_iter(["b".to_string(), "gone".to_string()]);
    let (_, rows) = build_all_values_table(&fields, &SearchFilter::default(), &expanded);
    let opened: Vec<&str> = rows.iter().filter(|r| r.initial_expanded).map(|r| r.field.as_str()).collect();
    assert_eq!(opened, vec!["b"]);
}

#[test]
fn filtering_replaces_each_fields_value_with_what_the_path_matches() {
    let fields: Fields = (0..3).map(records_field).chain([field("text", None), field("nomatch", Some(json!({"x": 1})))]).collect();

    // one match: the field's value becomes it; fields without a match drop out
    let path = parse_filter("records").unwrap();
    let (kept, matches) = filter_fields(&fields, path.as_ref());
    assert_eq!((kept.len(), matches), (3, 3));
    assert!(matches!(&kept[0].json, Some(Value::Array(items)) if items.len() == 2));
    assert_eq!(filter_status(path.as_ref(), matches, kept.len()), "3 matches in 3 fields");

    // several matches: an array of them
    let path = parse_filter("records.*.label").unwrap();
    let (kept, matches) = filter_fields(&fields, path.as_ref());
    assert_eq!((kept.len(), matches), (3, 6));
    assert_eq!(kept[0].json, Some(json!(["a", "b"])));

    // a matched string is plain text, like a non-JSON field
    let path = parse_filter("records[0].label").unwrap();
    let (kept, _) = filter_fields(&fields, path.as_ref());
    assert_eq!((kept[0].raw.as_str(), kept[0].json.is_none()), ("a", true));

    // no path: everything as it was; `$`: the values themselves, text included
    assert_eq!(filter_fields(&fields, None).0.len(), 5);
    assert_eq!(filter_fields(&fields, parse_filter("$").unwrap().as_ref()).0.len(), 5);
    assert_eq!(filter_status(None, 5, 5), "");
    assert_eq!(filter_status(parse_filter("zzz").unwrap().as_ref(), 0, 0), "No match");
    assert_eq!(filter_status(parse_filter("a").unwrap().as_ref(), 1, 1), "1 match in 1 field");
    assert!(parse_filter("  ").unwrap().is_none());
    assert!(parse_filter("a[").is_err());
}

#[test]
fn a_string_key_holding_json_can_be_filtered_like_a_hash_field() {
    use super::super::field::single_value_field;
    let field = single_value_field(Value::String(r#"{"a": [{"b": 1}, {"b": 2}]}"#.to_string()));
    assert!(field.json.is_some());
    assert_eq!(field.name, "(value)");
    let plain = single_value_field(Value::String("just text".to_string()));
    assert!(plain.json.is_none());
    let number_text = single_value_field(Value::String("123".to_string()));
    assert!(number_text.json.is_none(), "a bare number stays text");
    let (kept, matches) = filter_fields(&[Arc::new(field)], parse_filter("a.*.b").unwrap().as_ref());
    assert_eq!((kept[0].json.clone(), matches), (Some(json!([1, 2])), 2));
}

#[test]
fn the_header_gives_each_types_size_and_its_ttl() {
    use crate::backend::redis_value_type::RedisValueType as T;
    assert_eq!(meta_text(&T::Hash, -1, 12, false, None), "12 fields · no ttl");
    assert_eq!(meta_text(&T::List, 30, 5000, true, None), "5000+ items · ttl 30s", "a + says the list was cut");
    assert_eq!(meta_text(&T::Zset, 0, 3, false, None), "3 items · ttl 0s");
    assert_eq!(meta_text(&T::Stream, -1, 2, false, None), "2 entries · no ttl");
    assert!(meta_text(&T::String, -1, 1, true, None).starts_with("value cut at "));
    assert_eq!(meta_text(&T::String, -1, 1, false, None), "no ttl");
    assert_eq!(meta_text(&T::Other("ReJSON-RL".into()), 5, 1, false, None), "ttl 5s");
}
