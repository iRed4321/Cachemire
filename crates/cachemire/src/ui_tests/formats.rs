//! Values shown and edited as YAML, XML and raw text: the kind the editor opens on, what
//! picking another kind does to its text, its Format button, and the format each tab keeps.

use super::links::{links_ready, loaded, show_connection};
use super::*;
use crate::{ExportPrefs, ValueEditor};
use serde_json::json;

const YAML: &str = "name: api\nreplicas: 3\n";
const XML: &str = "<user id=\"1\"><name>Ada</name></user>";

/// The app showing a hash with a JSON, a YAML, an XML and a text field, loaded.
fn formats_ready() -> (Harness, Rc<crate::app::App>, tokio::runtime::Runtime, EventLoopTurn) {
    let (mut h, app, rt, turn, _) = links_ready(&[("a", "redis://cache.test:6379")]);
    show_connection(&h, "a");
    let fields = [("json", json!({"a": 1, "b": [true]})), ("yaml", json!(YAML)), ("xml", json!(XML)), ("text", json!("just \"text\""))];
    app.key_detail.seed_hash("a", "mixed", &fields);
    show_seeded(&mut h, &app, &rt, "mixed");
    (h, app, rt, turn)
}

/// Opens the tab of `key`, shown from what was seeded for it rather than read from a server.
pub(super) fn show_seeded(h: &mut Harness, app: &crate::app::App, rt: &tokio::runtime::Runtime, key: &str) {
    crate::tabs::select_key(app, key.into());
    app.key_detail.load(key.into(), Default::default(), true);
    assert!(loaded(h, rt));
    assert!(h.window.global::<KeyPanel>().get_all_values_rows().row_count() > 0, "{key} is shown");
}

/// Opens the editor on `field` from a row showing `format`: its kind and its text.
fn edit(h: &Harness, field: &str, format: i32) -> (i32, String) {
    h.window.global::<ValueEditor>().set_open(false);
    h.window.global::<KeyPanel>().invoke_edit_value(field.into(), format);
    let editor = h.window.global::<ValueEditor>();
    assert!(editor.get_open(), "the editor opens on {field}");
    (editor.get_kind(), editor.get_text().to_string())
}

/// Picks `kind` in the editor's menu: the text it then shows.
fn pick(h: &Harness, kind: i32) -> String {
    let editor = h.window.global::<ValueEditor>();
    editor.invoke_kind_picked(kind, editor.get_text());
    assert_eq!(editor.get_kind(), kind);
    editor.get_text().to_string()
}

#[test]
fn the_editor_opens_as_the_kind_its_row_shows() {
    let (h, _app, _rt, _turn) = formats_ready();
    assert_eq!(edit(&h, "yaml", 0), (0, "\"name: api\\nreplicas: 3\\n\"".to_string()), "as JSON, text is a string");
    assert_eq!(edit(&h, "yaml", 1).0, 0, "escaped JSON is edited as JSON");
    assert_eq!(edit(&h, "yaml", 2), (1, YAML.to_string()), "as YAML, the text itself");
    assert_eq!(edit(&h, "xml", 3), (2, XML.to_string()));
    assert_eq!(edit(&h, "text", 4), (3, "just \"text\"".to_string()));
    // a JSON document shown as YAML is still edited as the JSON it is
    assert_eq!(edit(&h, "json", 2), (0, "{\n  \"a\": 1,\n  \"b\": [\n    true\n  ]\n}".to_string()));
    assert_eq!(edit(&h, "json", 4), (3, "{\"a\":1,\"b\":[true]}".to_string()), "as raw text, what Redis holds");
}

#[test]
fn picking_another_kind_shows_the_same_text_another_way() {
    let (h, _app, _rt, _turn) = formats_ready();
    edit(&h, "yaml", 0);
    assert_eq!(pick(&h, 1), YAML, "the JSON string turns into its text");
    assert_eq!(pick(&h, 3), YAML, "which the other kinds leave alone");
    assert_eq!(pick(&h, 2), YAML);
    assert_eq!(pick(&h, 0), "\"name: api\\nreplicas: 3\\n\"", "and back into a JSON string");
    // nothing was changed: saving writes nothing and just closes
    let editor = h.window.global::<ValueEditor>();
    editor.invoke_save(editor.get_text(), "yaml".into());
    assert!(!editor.get_open());
}

#[test]
fn json_and_yaml_documents_are_converted_into_each_other() {
    let (h, _app, _rt, _turn) = formats_ready();
    edit(&h, "json", 0);
    assert_eq!(pick(&h, 1), "a: 1\nb:\n- true", "a JSON document becomes YAML");
    assert_eq!(pick(&h, 0), "{\n  \"a\": 1,\n  \"b\": [\n    true\n  ]\n}", "and YAML a JSON document");

    edit(&h, "yaml", 2);
    assert_eq!(pick(&h, 0), "{\n  \"name\": \"api\",\n  \"replicas\": 3\n}", "YAML text holding a mapping too");
    assert_eq!(pick(&h, 3), "{\n  \"name\": \"api\",\n  \"replicas\": 3\n}", "raw text takes it as typed");

    // text that is no document goes to JSON as a string
    edit(&h, "text", 4);
    assert_eq!(pick(&h, 0), "\"just \\\"text\\\"\"");
    assert_eq!(pick(&h, 4 - 1), "just \"text\"");
}

#[test]
fn the_editor_checks_colors_and_formats_as_its_kind() {
    let (h, _app, _rt, _turn) = formats_ready();
    let editor = h.window.global::<ValueEditor>();
    assert_eq!(editor.invoke_check("a: [1".into(), 0), "Not valid JSON: line 1, column 1");
    assert_eq!(editor.invoke_check("a: [1".into(), 1), "Not valid YAML: line 2, column 1");
    assert_eq!(editor.invoke_check("<a><b></a>".into(), 2), "Not valid XML: line 1, column 7");
    assert_eq!(editor.invoke_check("a: [1".into(), 3), "");
    assert_eq!(editor.invoke_check("a: 1".into(), 1), "");

    assert_eq!(editor.invoke_format("{\"a\":1}".into(), 0), "{\n  \"a\": 1\n}");
    assert_eq!(editor.invoke_format("a: {b: 1}".into(), 1), "a:\n  b: 1");
    assert_eq!(editor.invoke_format(XML.into(), 2), "<user id=\"1\">\n  <name>Ada</name>\n</user>");
    assert_eq!(editor.invoke_format(XML.into(), 3), XML);

    // each kind colors the same text its own way
    let colored: Vec<_> = (0..4).map(|kind| editor.invoke_highlight("k: \"v\" <t a='1'/>".into(), kind)).collect();
    assert!(colored[0] != colored[1] && colored[1] != colored[2] && colored[2] != colored[3]);
}

#[test]
fn an_open_row_is_served_in_the_format_asked() {
    let (mut h, _app, rt, _turn) = formats_ready();
    let panel = h.window.global::<KeyPanel>();
    let lines = |format: i32| {
        let count = panel.invoke_value_line_count("xml".into(), format, 200);
        let lines = panel.invoke_request_value_lines("xml".into(), "".into(), 0, format, 200, 0, count);
        (count, lines.iter().map(|line| line.len).collect::<Vec<_>>())
    };
    assert_eq!(lines(3), (3, vec![13, 18, 7]), "XML on one line is indented into three");
    assert_eq!(lines(4), (1, vec![XML.len() as i32]), "raw text is the one line it is");
    assert_eq!(lines(0).0, 1, "as JSON, a string");
    assert_eq!(panel.invoke_value_line_count("yaml".into(), 2, 200), 3, "two lines of YAML and the empty one that ends it");
    assert_eq!(panel.invoke_value_line_count("json".into(), 2, 200), 3, "a JSON document as YAML");

    // find works on the text of the format shown
    assert_eq!(panel.invoke_find_in_value("xml".into(), "ada".into(), 0, 3, 200).line, 1);
    assert_eq!(panel.invoke_find_in_value("xml".into(), "ada".into(), 0, 4, 200).line, 0);

    // asked ahead of time, a rendering is ready once made off the UI thread
    assert!(!panel.invoke_pretty_is_ready("text".into(), 2));
    panel.invoke_request_pretty_value_async("text".into(), 2);
    let mut ready = false;
    for _ in 0..500 {
        rt.block_on(async { tokio::time::sleep(std::time::Duration::from_millis(2)).await });
        h.frame();
        if h.window.global::<KeyPanel>().invoke_pretty_is_ready("text".into(), 2) {
            ready = true;
            break;
        }
    }
    assert!(ready);
    assert!(!h.window.global::<KeyPanel>().invoke_pretty_is_ready("text".into(), 3), "each format has its own");
}

#[test]
fn each_tab_keeps_the_format_its_rows_show() {
    let (mut h, app, rt, _turn) = formats_ready();
    app.key_detail.seed_hash("a", "other", &[("f", json!("x: 1"))]);
    assert_eq!(h.window.global::<ExportPrefs>().get_format(), 0, "a new tab shows JSON");
    h.window.global::<ExportPrefs>().set_format(2);

    show_seeded(&mut h, &app, &rt, "other");
    assert_eq!(h.window.global::<ExportPrefs>().get_format(), 0, "another tab has its own");
    h.window.global::<ExportPrefs>().set_format(4);

    crate::tabs::select_key(&app, "mixed".into());
    assert!(loaded(&mut h, &rt));
    assert_eq!(h.window.global::<ExportPrefs>().get_format(), 2);
    crate::tabs::select_key(&app, "other".into());
    assert!(loaded(&mut h, &rt));
    assert_eq!(h.window.global::<ExportPrefs>().get_format(), 4);
}

#[test]
fn the_rows_carry_a_one_line_preview_for_every_format() {
    let (h, _app, _rt, _turn) = formats_ready();
    let rows: Vec<_> = h.window.global::<KeyPanel>().get_all_values_rows().iter().collect();
    assert_eq!(rows.iter().map(|row| row.field.as_str()).collect::<Vec<_>>(), ["json", "yaml", "xml", "text"]);
    let yaml = &rows[1];
    assert!(yaml.value_compact_yaml != yaml.value_compact_text, "YAML is colored, raw text is not");
    assert!(yaml.value_compact != yaml.value_compact_yaml, "and JSON shows it as a string");
    let xml = &rows[2];
    assert!(xml.value_compact_xml != xml.value_compact_text);
    let text = &rows[3];
    assert!(text.value_compact_xml == text.value_compact_text, "text with no XML in it has nothing to color");
}
