use serde_json::json;

use super::*;

/// The colored runs of each line of `text` as `kind`: their text and what they are.
fn runs(text: &str, kind: EditKind) -> Vec<Vec<(String, &'static str)>> {
    let paint = crate::theme::paint();
    let name = |color: slint::Color| match color {
        c if c == paint.key => "key",
        c if c == paint.string => "string",
        c if c == paint.number => "number",
        c if c == paint.bool => "bool",
        c if c == paint.punct => "punct",
        c if c == paint.escape => "escape",
        _ => "?",
    };
    colored_lines(text, kind)
        .into_iter()
        .map(|(line, spans)| spans.into_iter().map(|(range, color)| (line[range].to_string(), name(color))).collect())
        .collect()
}

/// `runs` of a single line, as plain pairs.
fn line_runs(text: &str, kind: EditKind) -> Vec<(String, &'static str)> {
    runs(text, kind).into_iter().next().unwrap_or_default()
}

fn pairs(runs: &[(&str, &'static str)]) -> Vec<(String, &'static str)> {
    runs.iter().map(|(text, kind)| (text.to_string(), *kind)).collect()
}

fn text_field(raw: &str) -> Field {
    Field::new("f".into(), raw.into(), None)
}

fn json_field(value: serde_json::Value) -> Field {
    Field::from_value("f".into(), value)
}

fn text_of(paragraphs: &[ColoredParagraph]) -> String {
    paragraphs.iter().map(|(text, _)| text.as_str()).collect::<Vec<_>>().join("\n")
}

#[test]
fn the_formats_follow_the_order_of_the_menus() {
    let all = [ShowFormat::Json, ShowFormat::Escaped, ShowFormat::Yaml, ShowFormat::Xml, ShowFormat::Text];
    assert_eq!(all.len(), ShowFormat::COUNT);
    for (index, format) in all.into_iter().enumerate() {
        assert_eq!(ShowFormat::from_index(index as i32), format);
        assert_eq!(format.index(), index);
    }
    assert_eq!(ShowFormat::from_index(-1), ShowFormat::Json, "an index out of the menus is JSON");
    assert_eq!(ShowFormat::from_index(99), ShowFormat::Json);
    assert_eq!(all.map(ShowFormat::extension), ["json", "txt", "yaml", "xml", "txt"]);

    let kinds = [EditKind::Json, EditKind::Yaml, EditKind::Xml, EditKind::Text];
    for (index, kind) in kinds.into_iter().enumerate() {
        assert_eq!(EditKind::from_index(index as i32), kind);
        assert_eq!(kind.index(), index as i32);
    }
    assert_eq!(EditKind::from_index(42), EditKind::Json);
}

#[test]
fn the_editor_opens_on_the_kind_of_the_rows_format() {
    let text = text_field("a: 1");
    assert_eq!(edit_kind_for(&text, ShowFormat::Json), EditKind::Json);
    assert_eq!(edit_kind_for(&text, ShowFormat::Escaped), EditKind::Json);
    assert_eq!(edit_kind_for(&text, ShowFormat::Yaml), EditKind::Yaml);
    assert_eq!(edit_kind_for(&text, ShowFormat::Xml), EditKind::Xml);
    assert_eq!(edit_kind_for(&text, ShowFormat::Text), EditKind::Text);

    // a JSON document shown as YAML is still JSON in Redis: it is edited as JSON
    let document = json_field(json!({"a": 1}));
    assert_eq!(edit_kind_for(&document, ShowFormat::Yaml), EditKind::Json);
    assert_eq!(edit_kind_for(&document, ShowFormat::Text), EditKind::Text);
    // a number is not a document
    let number = Field::hash_value("f".into(), "42".into(), false);
    assert_eq!(edit_kind_for(&number, ShowFormat::Yaml), EditKind::Yaml);
}

#[test]
fn a_json_document_is_written_as_yaml_in_its_own_order() {
    let yaml = to_yaml(&json!({"zebra": 1, "apple": "x", "list": [1, {"deep": null}], "text": "two\nlines"})).unwrap();
    assert_eq!(yaml, "zebra: 1\napple: x\nlist:\n- 1\n- deep: null\ntext: |-\n  two\n  lines");
    assert_eq!(to_yaml(&json!([])).unwrap(), "[]");
}

#[test]
fn each_format_shows_its_own_text_of_a_value() {
    let document = json_field(json!({"a": 1, "b": [true]}));
    assert_eq!(shown_text(&document, ShowFormat::Json), "{\n  \"a\": 1,\n  \"b\": [\n    true\n  ]\n}");
    assert_eq!(shown_text(&document, ShowFormat::Escaped), r#""{\"a\":1,\"b\":[true]}""#);
    assert_eq!(shown_text(&document, ShowFormat::Yaml), "a: 1\nb:\n- true");
    assert_eq!(shown_text(&document, ShowFormat::Text), r#"{"a":1,"b":[true]}"#);
    assert_eq!(shown_text(&document, ShowFormat::Xml), r#"{"a":1,"b":[true]}"#, "JSON isn't turned into XML");

    let yaml = text_field("a: 1\nb: 2\n");
    assert_eq!(shown_text(&yaml, ShowFormat::Yaml), "a: 1\nb: 2\n", "YAML text is shown as written");
    assert_eq!(shown_text(&yaml, ShowFormat::Text), "a: 1\nb: 2\n");
    assert_eq!(shown_text(&yaml, ShowFormat::Json), r#""a: 1\nb: 2\n""#, "as JSON, text is a string");

    let xml = text_field("<a><b>1</b></a>");
    assert_eq!(shown_text(&xml, ShowFormat::Xml), "<a>\n  <b>1</b>\n</a>", "XML on one line is indented");
    assert_eq!(shown_text(&xml, ShowFormat::Text), "<a><b>1</b></a>");
    let written = text_field("<a>\n<b>1</b>\n</a>");
    assert_eq!(shown_text(&written, ShowFormat::Xml), "<a>\n<b>1</b>\n</a>", "XML on several lines is shown as written");
}

#[test]
fn an_open_row_has_a_paragraph_per_line_of_its_format() {
    let yaml = text_field("a: 1\r\nb:\r\n  - x\r\n");
    let paragraphs = paragraphs_for(&yaml, ShowFormat::Yaml);
    assert_eq!(text_of(&paragraphs), "a: 1\nb:\n  - x\n", "no line keeps its carriage return");
    assert_eq!(paragraphs.len(), 4);
    assert!(!paragraphs[0].1.is_empty(), "YAML is colored");

    let text = paragraphs_for(&yaml, ShowFormat::Text);
    assert_eq!(text.len(), 4);
    assert!(text.iter().all(|(_, spans)| spans.is_empty()), "raw text has no colors");

    let document = json_field(json!({"a": [1]}));
    assert_eq!(text_of(&paragraphs_for(&document, ShowFormat::Json)), "{\n  \"a\": [\n    1\n  ]\n}");
    assert_eq!(text_of(&paragraphs_for(&document, ShowFormat::Escaped)), r#""{\"a\":[1]}""#);
    assert_eq!(text_of(&paragraphs_for(&document, ShowFormat::Yaml)), "a:\n- 1");
    assert_eq!(text_of(&paragraphs_for(&text_field("<a><b/></a>"), ShowFormat::Xml)), "<a>\n  <b/>\n</a>");
}

#[test]
fn json_is_colored_even_before_it_is_valid() {
    assert_eq!(
        line_runs(r#"{"a": [1, true, "s"], "open"#, EditKind::Json),
        pairs(&[
            ("{", "punct"),
            ("\"a\"", "key"),
            (":", "punct"),
            ("[", "punct"),
            ("1", "number"),
            (",", "punct"),
            ("true", "bool"),
            (",", "punct"),
            ("\"s\"", "string"),
            ("]", "punct"),
            (",", "punct"),
            ("\"open", "string"),
        ])
    );
    assert_eq!(line_runs("-1.5e3 false null", EditKind::Json), pairs(&[("-1.5e3", "number"), ("false", "bool")]));
}

#[test]
fn yaml_keys_scalars_lists_and_comments_are_colored() {
    assert_eq!(line_runs("name: api", EditKind::Yaml), pairs(&[("name", "key"), (":", "punct"), ("api", "string")]));
    assert_eq!(line_runs("replicas: 3", EditKind::Yaml), pairs(&[("replicas", "key"), (":", "punct"), ("3", "number")]));
    assert_eq!(line_runs("on: TRUE", EditKind::Yaml), pairs(&[("on", "key"), (":", "punct"), ("TRUE", "bool")]));
    assert_eq!(line_runs("owner: ~", EditKind::Yaml), pairs(&[("owner", "key"), (":", "punct")]), "null keeps the text's color");
    assert_eq!(line_runs("deps:", EditKind::Yaml), pairs(&[("deps", "key"), (":", "punct")]));
    assert_eq!(line_runs("  - auth", EditKind::Yaml), pairs(&[("-", "punct"), ("auth", "string")]));
    assert_eq!(
        line_runs("- - k: v", EditKind::Yaml),
        pairs(&[("-", "punct"), ("-", "punct"), ("k", "key"), (":", "punct"), ("v", "string")])
    );
    assert_eq!(line_runs("# a comment", EditKind::Yaml), pairs(&[("# a comment", "punct")]));
    assert_eq!(line_runs("---", EditKind::Yaml), pairs(&[("---", "punct")]));
    assert_eq!(
        line_runs("image: \"a:b # no\" # yes", EditKind::Yaml),
        pairs(&[("image", "key"), (":", "punct"), ("\"a:b # no\"", "string"), ("# yes", "punct")])
    );
    assert_eq!(line_runs("'quoted key': 1", EditKind::Yaml), pairs(&[("'quoted key'", "key"), (":", "punct"), ("1", "number")]));
    assert_eq!(line_runs("url: http://x:80/y", EditKind::Yaml), pairs(&[("url", "key"), (":", "punct"), ("http://x:80/y", "string")]));
    assert_eq!(line_runs("ref: *anchor", EditKind::Yaml), pairs(&[("ref", "key"), (":", "punct"), ("*anchor", "punct")]));
    assert_eq!(line_runs("é: ü", EditKind::Yaml), pairs(&[("é", "key"), (":", "punct"), ("ü", "string")]), "offsets are bytes");
}

#[test]
fn a_yaml_flow_value_is_colored_as_json_and_block_text_as_a_string() {
    assert_eq!(
        line_runs("flow: {\"b\": [true, 2]}", EditKind::Yaml),
        pairs(&[
            ("flow", "key"),
            (":", "punct"),
            ("{", "punct"),
            ("\"b\"", "key"),
            (":", "punct"),
            ("[", "punct"),
            ("true", "bool"),
            (",", "punct"),
            ("2", "number"),
            ("]", "punct"),
            ("}", "punct"),
        ])
    );
    let block = runs("notes: |\n  Rolled: out\n\n  more\nlast: x", EditKind::Yaml);
    assert_eq!(block[0], pairs(&[("notes", "key"), (":", "punct"), ("|", "punct")]));
    assert_eq!(block[1], pairs(&[("Rolled: out", "string")]), "a colon in block text makes no key");
    assert!(block[2].is_empty());
    assert_eq!(block[3], pairs(&[("more", "string")]));
    assert_eq!(block[4], pairs(&[("last", "key"), (":", "punct"), ("x", "string")]), "the block ends where the indent does");
}

#[test]
fn xml_tags_attributes_comments_and_entities_are_colored() {
    assert_eq!(
        line_runs("<user id=\"1\" a='x>y'>Ada &amp; co</user>", EditKind::Xml),
        pairs(&[
            ("<", "punct"),
            ("user", "key"),
            ("id", "bool"),
            ("=", "punct"),
            ("\"1\"", "string"),
            ("a", "bool"),
            ("=", "punct"),
            ("'x>y'", "string"),
            (">", "punct"),
            ("&amp;", "escape"),
            ("<", "punct"),
            ("/", "punct"),
            ("user", "key"),
            (">", "punct"),
        ])
    );
    assert_eq!(line_runs("<?xml version=\"1.0\"?>", EditKind::Xml), pairs(&[("<?xml version=\"1.0\"?>", "punct")]));
    assert_eq!(line_runs("<e/>", EditKind::Xml), pairs(&[("<", "punct"), ("e", "key"), ("/", "punct"), (">", "punct")]));
    assert_eq!(line_runs("<b><![CDATA[x<y]]></b>", EditKind::Xml)[3], ("<![CDATA[x<y]]>".to_string(), "string"));
    assert_eq!(line_runs("a & b", EditKind::Xml), Vec::new(), "a lone ampersand is text");

    // a comment over two lines is colored on both
    let comment = runs("<a><!-- one\ntwo --></a>", EditKind::Xml);
    assert_eq!(comment[0].last().unwrap(), &("<!-- one".to_string(), "punct"));
    assert_eq!(comment[1][0], ("two -->".to_string(), "punct"));
}

#[test]
fn coloring_never_fails_on_text_cut_anywhere() {
    let samples = [
        "",
        "\n",
        "<",
        "<!--",
        "<![CDATA[",
        "<?",
        "&",
        "- ",
        "-",
        ":",
        "a:",
        "\"",
        "'",
        "  ",
        "<a b=",
        "<a b='",
        "é<é a=\"é\">é&é;",
        "|",
        "k: |",
        "#",
        "{",
        "k: [1,",
        "   \n  x",
    ];
    for kind in [EditKind::Json, EditKind::Yaml, EditKind::Xml, EditKind::Text] {
        for sample in samples {
            let paragraphs = colored_lines(sample, kind);
            assert_eq!(paragraphs.len(), sample.split('\n').count(), "{kind:?} {sample:?}");
            for (line, spans) in &paragraphs {
                assert!(
                    spans.iter().all(|(range, _)| range.start < range.end
                        && line.is_char_boundary(range.start)
                        && line.is_char_boundary(range.end))
                );
            }
        }
    }
    assert!(colored_lines("a: 1\n<b/>", EditKind::Text).iter().all(|(_, spans)| spans.is_empty()));
}

#[test]
fn a_collapsed_row_shows_its_lines_end_to_end_with_their_colors() {
    let (line, spans) = colored_single_line("# top\nserver:\n  host: local\n\n  port: 80\n", EditKind::Yaml);
    assert_eq!(line, "# top server: host: local port: 80");
    let colored: Vec<&str> = spans.iter().map(|(range, _)| &line[range.clone()]).collect();
    assert_eq!(colored, ["# top", "server", ":", "host", ":", "local", "port", ":", "80"]);

    let (line, spans) = colored_single_line("<a>\n  <b>1</b>\n</a>", EditKind::Xml);
    assert_eq!(line, "<a> <b>1</b> </a>");
    assert_eq!(&line[spans[1].0.clone()], "a");

    let (line, spans) = colored_single_line("  two words \n next ", EditKind::Text);
    assert_eq!((line.as_str(), spans.len()), ("two words next", 0));
    assert_eq!(colored_single_line("", EditKind::Yaml).0, "");
}

#[test]
fn xml_on_one_line_gets_a_tag_per_line() {
    let xml = r#"<?xml version="1.0"?><user id="1"><name>Ada</name><e/><!-- note --><address><city>Paris</city><empty></empty></address><bio><![CDATA[a<b]]></bio></user>"#;
    assert_eq!(
        indent_xml(xml).unwrap(),
        "<?xml version=\"1.0\"?>\n<user id=\"1\">\n  <name>Ada</name>\n  <e/>\n  <!-- note -->\n  <address>\n    <city>Paris</city>\n    <empty></empty>\n  </address>\n  <bio><![CDATA[a<b]]></bio>\n</user>"
    );
    assert_eq!(indent_xml("<a b='x>y'><c/></a>").unwrap(), "<a b='x>y'>\n  <c/>\n</a>", "a > in a value ends no tag");
}

#[test]
fn xml_that_cannot_be_indented_safely_is_left_alone() {
    assert_eq!(indent_xml("<a>text<b/></a>"), None, "text beside an element");
    assert_eq!(indent_xml("<a><b/>tail</a>"), None);
    assert_eq!(indent_xml("<a><b></a>"), None, "a tag left open");
    assert_eq!(indent_xml("</a>"), None, "a closing tag with nothing to close");
    assert_eq!(indent_xml("<a><b"), None, "a tag cut short");
    assert_eq!(indent_xml("<a/>trailing"), None);
}

#[test]
fn the_format_button_lays_xml_out_again_whatever_its_lines() {
    assert_eq!(format_xml("  <a>\n<b>1</b>\n      <c><d/></c></a>\n").unwrap(), "<a>\n  <b>1</b>\n  <c>\n    <d/>\n  </c>\n</a>");
    assert_eq!(format_xml("<a>mixed<b/></a>"), None);
}

#[test]
fn the_format_button_rewrites_yaml_unless_that_would_lose_something() {
    assert_eq!(format_yaml("b:   {x: 1, y: [1,2]}\na:    text\n").unwrap(), "b:\n  x: 1\n  y:\n  - 1\n  - 2\na: text");
    assert_eq!(format_yaml("a: 1   \n# kept\nb:   2  ").unwrap(), "a: 1\n# kept\nb:   2", "with a comment, only trailing spaces go");
    assert_eq!(format_yaml("base: &b {x: 1}\nuse: *b  ").unwrap(), "base: &b {x: 1}\nuse: *b", "anchors are kept as written");
    assert_eq!(format_yaml("a: [1"), None, "not valid: left as it is");
}

#[test]
fn an_xml_document_reads_as_an_object_of_its_children_and_attributes() {
    let user =
        structured_value(r#"<user id="7" active="true"><name>Ada</name><address><city>Paris</city></address><e/>  </user>"#).unwrap();
    assert_eq!(user, json!({"@id": "7", "@active": "true", "name": "Ada", "address": {"city": "Paris"}, "e": ""}));

    let repeated = structured_value("<list><tag>a</tag><tag>b</tag><tag>c</tag><other>x</other></list>").unwrap();
    assert_eq!(repeated, json!({"tag": ["a", "b", "c"], "other": "x"}));

    let mixed = structured_value(r#"<p lang="en">hello <b>bold</b></p>"#).unwrap();
    assert_eq!(mixed, json!({"@lang": "en", "b": "bold", "#text": "hello"}));
}

#[test]
fn a_root_of_one_repeated_element_reads_as_a_list_of_them() {
    let books = structured_value(r#"<catalog><book id="1"><t>A</t></book><book id="2"><t>B</t></book></catalog>"#).unwrap();
    assert_eq!(books, json!([{"@id": "1", "t": "A"}, {"@id": "2", "t": "B"}]));
}

#[test]
fn only_structured_text_reads_as_a_value() {
    assert_eq!(
        structured_value("id: 2\ntags:\n  - a\nnested:\n  ok: true\n").unwrap(),
        json!({"id": 2, "tags": ["a"], "nested": {"ok": true}})
    );
    assert_eq!(structured_value("- a\n- b: 1\n").unwrap(), json!(["a", {"b": 1}]));
    assert_eq!(structured_value("just some text"), None);
    assert_eq!(structured_value("two lines\nof text"), None);
    assert_eq!(structured_value("12:30"), None, "no key: a scalar");
    assert_eq!(structured_value(""), None);
    assert_eq!(structured_value("<not xml"), None);
    assert_eq!(structured_value("<leaf>only text</leaf>"), None, "an element with nothing but text has no columns");
    assert_eq!(structured_value("a: [1"), None, "YAML that doesn't parse");

    assert_eq!(yaml_document("a: 1"), Some(json!({"a": 1})));
    assert_eq!(yaml_document("plain"), None);
    assert_eq!(yaml_document("42"), None);
}

#[test]
fn each_kind_says_where_text_is_not_valid() {
    assert_eq!(check(r#"{"a": 1}"#, EditKind::Json), "");
    assert_eq!(check("{\n  \"a\": \n}", EditKind::Json), "Not valid JSON: line 3, column 1");
    assert_eq!(check("a: 1\nb:\n  - x", EditKind::Yaml), "");
    assert_eq!(check("a: [1", EditKind::Yaml), "Not valid YAML: line 2, column 1");
    assert_eq!(check("<a><b>1</b></a>", EditKind::Xml), "");
    assert_eq!(check("<a><b></a>", EditKind::Xml), "Not valid XML: line 1, column 7");
    assert_eq!(check("", EditKind::Xml), "Not valid XML: line 1, column 1");
    for anything in ["", "{", "<a", "a: [1"] {
        assert_eq!(check(anything, EditKind::Text), "", "raw text is always valid");
    }
}
