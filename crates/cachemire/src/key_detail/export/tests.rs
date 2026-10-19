use super::super::highlight::head_chars;
use super::super::pretty_text::{COMPACT_PREVIEW_CHAR_BUDGET, colored_paragraphs_for};
use super::*;
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
