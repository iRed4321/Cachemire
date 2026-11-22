use super::*;

#[test]
fn json_is_laid_out_without_touching_its_strings_and_numbers() {
    let compact = r#"{"a":[1.50,{"b":"x , y"}],"e":{},"l":[]}"#;
    let pretty = "{\n  \"a\": [\n    1.50,\n    {\n      \"b\": \"x , y\"\n    }\n  ],\n  \"e\": {},\n  \"l\": []\n}";
    assert_eq!(relayout(compact, true), pretty);
    assert_eq!(relayout(pretty, false), compact, "and back on one line");
}

#[test]
fn the_format_button_lays_out_each_kind_and_leaves_what_is_not_valid() {
    assert_eq!(format(r#"{"a":1}"#, EditKind::Json), "{\n  \"a\": 1\n}");
    assert_eq!(format(r#"{"a":"#, EditKind::Json), r#"{"a":"#);
    assert_eq!(format("a:   {b: 1}", EditKind::Yaml), "a:\n  b: 1");
    assert_eq!(format("a: [1", EditKind::Yaml), "a: [1");
    assert_eq!(format("<a><b>1</b></a>", EditKind::Xml), "<a>\n  <b>1</b>\n</a>");
    assert_eq!(format("<a>text<b/></a>", EditKind::Xml), "<a>text<b/></a>", "mixed content is left as written");
    assert_eq!(format(r#"{"a":1}"#, EditKind::Text), r#"{"a":1}"#, "raw text is never laid out");
}

#[test]
fn the_editor_shows_json_pretty_and_text_as_a_json_string() {
    // (text, shown as a string, written back as typed)
    assert_eq!(editor_view(r#"{"a":1}"#, true), ("{\n  \"a\": 1\n}".to_string(), false, false));
    assert_eq!(editor_view("{\n \"a\": 1\n}", true), ("{\n  \"a\": 1\n}".to_string(), false, true), "JSON on several lines");
    assert_eq!(editor_view("plain \"text\"", true), (r#""plain \"text\"""#.to_string(), true, false));
    // a number is JSON in a hash field, text in a string key
    assert_eq!(editor_view("42", true), ("42".to_string(), false, false));
    assert_eq!(editor_view("42", false), ("\"42\"".to_string(), true, false));
    assert_eq!(editor_view("[1]", false), ("[\n  1\n]".to_string(), false, false));
}

#[test]
fn the_other_kinds_show_the_value_as_it_is_stored() {
    for kind in [EditKind::Yaml, EditKind::Xml, EditKind::Text] {
        assert_eq!(view_as("a: 1\r\nb: 2", true, kind), ("a: 1\nb: 2".to_string(), false, false), "{kind:?}");
        assert_eq!(view_as(r#"{"a":1}"#, true, kind).0, r#"{"a":1}"#, "JSON is not laid out as {kind:?}");
    }
    assert_eq!(view_as(r#"{"a":1}"#, true, EditKind::Json), editor_view(r#"{"a":1}"#, true));
}

#[test]
fn a_text_fits_when_its_wrapped_lines_do() {
    assert!(fits("one\ntwo", 80, 2));
    assert!(!fits("one\ntwo\nthree", 80, 2));
    assert!(fits("abcdefgh", 4, 2), "eight characters wrap to two lines of four");
    assert!(!fits("abcdefghi", 4, 2));
    assert!(fits("", 80, 1), "an empty line is still a line");
}

#[test]
fn the_numbered_lines_follow_the_text() {
    let lines = VecModel::<SharedString>::default();
    let shown = |lines: &VecModel<SharedString>| lines.iter().map(|line| line.to_string()).collect::<Vec<_>>();
    update_lines(&lines, "a\nb\nc");
    assert_eq!(shown(&lines), ["a", "b", "c"]);
    update_lines(&lines, "a\nx\ny\nc");
    assert_eq!(shown(&lines), ["a", "x", "y", "c"], "lines added in the middle");
    update_lines(&lines, "a\nc");
    assert_eq!(shown(&lines), ["a", "c"], "and removed");
    update_lines(&lines, "");
    assert_eq!(shown(&lines), [""]);
    let long: String = (0..500).map(|i| format!("line {i}\n")).collect();
    update_lines(&lines, &long);
    assert_eq!(lines.row_count(), 501, "a big change resets them all");
}
