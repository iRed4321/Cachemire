use serde_json::json;

use super::*;

fn names(fields: &Fields) -> Vec<&str> {
    fields.iter().map(|f| f.name.as_str()).collect()
}

#[test]
fn a_page_keeps_the_order_of_its_names_and_parses_what_is_json() {
    let values = json!({"b": "{\"x\":1}", "a": "plain text", "c": "[1,2]"});
    let fields = build_fields(values, FxHashMap::default(), &Default::default(), &["c".into(), "a".into(), "b".into()]);
    assert_eq!(names(&fields), ["c", "a", "b"]);
    assert_eq!(fields[0].json, Some(json!([1, 2])));
    assert_eq!(fields[1].json, None);
    assert_eq!(fields[1].raw, "plain text");
    assert_eq!(fields[2].json, Some(json!({"x": 1})));
}

#[test]
fn an_oversized_field_is_kept_by_its_size_and_a_gone_one_is_dropped() {
    let oversized: FxHashMap<String, usize> = [("big".to_string(), 500_000)].into_iter().collect();
    let fields = build_fields(json!({"small": "1"}), oversized, &Default::default(), &["small".into(), "big".into(), "deleted".into()]);
    assert_eq!(names(&fields), ["small", "big"]);
    assert_eq!(fields[1].too_long, Some(500_000));
    assert_eq!(fields[1].raw, "");
}

#[test]
fn a_reply_that_isnt_an_object_gives_no_field() {
    assert!(build_fields(json!(["a"]), FxHashMap::default(), &Default::default(), &["a".into()]).is_empty());
}

#[test]
fn a_string_key_shows_as_json_only_when_it_holds_an_object_or_array() {
    assert_eq!(single_value_field(json!("{\"a\":1}"), false).json, Some(json!({"a": 1})));
    assert_eq!(single_value_field(json!("[true]"), false).json, Some(json!([true])));
    for scalar in ["42", "\"quoted\"", "true", "not json"] {
        let field = single_value_field(json!(scalar), false);
        assert_eq!(field.json, None, "{scalar}");
        assert_eq!(field.raw, scalar);
    }
    let list = single_value_field(json!(["x", "y"]), false);
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
    assert_eq!((field.searched_name(false), field.searched_raw(false)), ("username", "e\u{301}lodie"));
}

#[test]
fn plain_and_escaped_renderings_are_kept_apart() {
    let mut renderings = PrettyRenderings::default();
    renderings.insert("f".into(), ShowFormat::Escaped, Arc::new(vec![("escaped".into(), Default::default())]));
    assert!(!renderings.contains("f", ShowFormat::Json));
    let mut rendered = 0;
    renderings.get_or_insert_with("f", ShowFormat::Json, || {
        rendered += 1;
        vec![("plain".into(), Default::default())]
    });
    renderings.get_or_insert_with("f", ShowFormat::Json, || {
        rendered += 1;
        Vec::new()
    });
    assert_eq!(rendered, 1, "a rendering that exists is not made again");
    assert_eq!(renderings.get("f", ShowFormat::Json).unwrap()[0].0, "plain");
    assert_eq!(renderings.get("f", ShowFormat::Escaped).unwrap()[0].0, "escaped");
    renderings.clear();
    assert!(!renderings.contains("f", ShowFormat::Escaped));
}

#[test]
fn every_format_keeps_its_own_rendering() {
    let mut renderings = PrettyRenderings::default();
    let formats = [ShowFormat::Json, ShowFormat::Escaped, ShowFormat::Yaml, ShowFormat::Xml, ShowFormat::Text];
    for format in formats {
        assert!(!renderings.contains("f", format));
        renderings.insert("f".into(), format, Arc::new(vec![(format!("{format:?}"), Default::default())]));
    }
    for format in formats {
        assert_eq!(renderings.get("f", format).unwrap()[0].0, format!("{format:?}"));
        assert!(!renderings.contains("other", format));
    }
    renderings.clear();
    assert!(formats.iter().all(|format| !renderings.contains("f", *format)));
}

#[test]
fn the_table_reads_json_first_then_what_yaml_or_xml_text_stands_for() {
    let document = Field::hash_value("f".into(), r#"{"a":1}"#.into(), false);
    assert_eq!(document.table_value(), Some(&json!({"a": 1})));
    assert_eq!(Field::new("f".into(), "a: 1\nb: [x]".into(), None).table_value(), Some(&json!({"a": 1, "b": ["x"]})));
    assert_eq!(Field::new("f".into(), "<a><b>1</b></a>".into(), None).table_value(), Some(&json!({"b": "1"})));
    assert_eq!(Field::new("f".into(), "plain text".into(), None).table_value(), None);
    assert_eq!(Field::too_long("f".into(), 9_000_000).table_value(), None, "a value not read has nothing to show");
    // a number in a hash field is JSON, and stays the number it is
    assert_eq!(Field::hash_value("f".into(), "42".into(), false).table_value(), Some(&json!(42)));
}
