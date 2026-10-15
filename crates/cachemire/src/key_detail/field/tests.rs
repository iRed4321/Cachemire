use serde_json::json;

use super::*;

fn names(fields: &Fields) -> Vec<&str> {
    fields.iter().map(|f| f.name.as_str()).collect()
}

#[test]
fn a_page_keeps_the_order_of_its_names_and_parses_what_is_json() {
    let values = json!({"b": "{\"x\":1}", "a": "plain text", "c": "[1,2]"});
    let fields = build_fields(values, FxHashMap::default(), &["c".into(), "a".into(), "b".into()]);
    assert_eq!(names(&fields), ["c", "a", "b"]);
    assert_eq!(fields[0].json, Some(json!([1, 2])));
    assert_eq!(fields[1].json, None);
    assert_eq!(fields[1].raw, "plain text");
    assert_eq!(fields[2].json, Some(json!({"x": 1})));
}

#[test]
fn an_oversized_field_is_kept_by_its_size_and_a_gone_one_is_dropped() {
    let oversized: FxHashMap<String, usize> = [("big".to_string(), 500_000)].into_iter().collect();
    let fields = build_fields(json!({"small": "1"}), oversized, &["small".into(), "big".into(), "deleted".into()]);
    assert_eq!(names(&fields), ["small", "big"]);
    assert_eq!(fields[1].too_long, Some(500_000));
    assert_eq!(fields[1].raw, "");
}

#[test]
fn a_reply_that_isnt_an_object_gives_no_field() {
    assert!(build_fields(json!(["a"]), FxHashMap::default(), &["a".into()]).is_empty());
}

#[test]
fn a_string_key_shows_as_json_only_when_it_holds_an_object_or_array() {
    assert_eq!(single_value_field(json!("{\"a\":1}")).json, Some(json!({"a": 1})));
    assert_eq!(single_value_field(json!("[true]")).json, Some(json!([true])));
    for scalar in ["42", "\"quoted\"", "true", "not json"] {
        let field = single_value_field(json!(scalar));
        assert_eq!(field.json, None, "{scalar}");
        assert_eq!(field.raw, scalar);
    }
    let list = single_value_field(json!(["x", "y"]));
    assert_eq!((list.raw.as_str(), list.json), (r#"["x","y"]"#, Some(json!(["x", "y"]))));
}

#[test]
fn a_text_value_serializes_as_a_json_string() {
    let text = Field::from_value("f".into(), json!("a \"quote\""));
    assert_eq!(serde_json::to_value(text.value()).unwrap(), json!("a \"quote\""));
    let number = Field::from_value("n".into(), json!(1.5));
    assert_eq!(serde_json::to_value(number.value()).unwrap(), json!(1.5));
}

#[test]
fn searches_compare_against_the_lowercased_name_and_value() {
    let field = Field::new("UserName".into(), "ÉLODIE".into(), None);
    assert_eq!((field.lower_name(), field.lower_raw()), ("username", "élodie"));
}

#[test]
fn plain_and_escaped_renderings_are_kept_apart() {
    let mut renderings = PrettyRenderings::default();
    renderings.insert("f".into(), true, Arc::new(vec![("escaped".into(), Default::default())]));
    assert!(!renderings.contains("f", false));
    let mut rendered = 0;
    renderings.plain_or_insert_with("f", || {
        rendered += 1;
        vec![("plain".into(), Default::default())]
    });
    renderings.plain_or_insert_with("f", || {
        rendered += 1;
        Vec::new()
    });
    assert_eq!(rendered, 1, "a rendering that exists is not made again");
    assert_eq!(renderings.get("f", false).unwrap()[0].0, "plain");
    assert_eq!(renderings.get("f", true).unwrap()[0].0, "escaped");
    renderings.clear();
    assert!(!renderings.contains("f", true));
}
