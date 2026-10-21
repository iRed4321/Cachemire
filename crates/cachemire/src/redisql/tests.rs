use super::*;

#[test]
fn a_query_tab_id_is_never_a_key_or_a_search_tab() {
    let mut tabs = QueryTabs::default();
    let id = tabs.create(String::new());
    assert!(is_query_id(&id));
    assert!(!is_query_id("search:1"));
    assert!(!is_query_id("\u{1}search:1"));
    assert_ne!(id, tabs.create(String::new()));
    tabs.forget(&id);
}

const SCRIPT: &str = "var k = 'orders';\nFROM KEY k AS o SELECT *;\nvar n = 2;\nFROM KEY 'other' AS x SELECT *";

/// The part of the script taken from the box is where the box has it, so an error in it
/// can be marked in the box.
fn assert_maps_back(run: &ScriptToRun, text: &str) {
    let taken = &run.script[run.in_script..];
    assert_eq!(&text[run.in_box..run.in_box + taken.len()], taken);
}

#[test]
fn run_at_the_caret_takes_its_statement_with_the_declarations_before_it() {
    let caret = SCRIPT.find("'other'").unwrap();
    let run = script_to_run(SCRIPT, caret, caret, false).unwrap();
    assert!(run.script.starts_with("var k = 'orders';\nvar n = 2;\n"), "{}", run.script);
    assert!(run.script.trim_end().ends_with("FROM KEY 'other' AS x SELECT *") && !run.script.contains("AS o"), "{}", run.script);
    assert_maps_back(&run, SCRIPT);

    let first = SCRIPT.find("AS o").unwrap();
    let run = script_to_run(SCRIPT, first, first, false).unwrap();
    assert!(run.script.contains("var k") && !run.script.contains("var n"), "a later declaration isn't in effect yet: {}", run.script);
    assert_maps_back(&run, SCRIPT);
}

#[test]
fn run_takes_the_selection_as_it_is_and_the_whole_box_when_asked() {
    let start = SCRIPT.find("FROM KEY 'other'").unwrap();
    let (end, anchor) = (SCRIPT.len(), start);
    let run = script_to_run(SCRIPT, end, anchor, false).unwrap();
    assert_eq!(&run.script[run.in_script..], "FROM KEY 'other' AS x SELECT *", "selected backwards as well");
    assert_eq!(run.in_box, start);
    assert_maps_back(&run, SCRIPT);

    let whole = script_to_run(SCRIPT, 3, 3, true).unwrap();
    assert_eq!((whole.script.as_str(), whole.in_script, whole.in_box), (SCRIPT, 0, 0));
}

#[test]
fn a_caret_past_the_last_statement_runs_that_statement() {
    let text = "FROM KEY 'a' AS a SELECT *;\n\n";
    let run = script_to_run(text, text.len(), text.len(), false).unwrap();
    assert_eq!(run.script.trim(), "FROM KEY 'a' AS a SELECT *");
    assert!(script_to_run("", 0, 0, false).is_none(), "an empty box runs nothing");
}

#[test]
fn a_lone_result_has_no_tab_and_each_of_several_says_its_number_and_rows() {
    let output = |rows: usize| Ok(QueryOutput { rows: vec![Map::new(); rows], truncated: false, sources_truncated: false });
    assert_eq!(result_of(0, 1, output(3)).label, "", "nothing to pick between");
    let second = result_of(1, 3, output(2));
    assert_eq!((second.label.as_str(), second.rows.len(), second.status_is_error), ("2 · 2", 2, false));

    let failed = result_of(2, 3, Err(cachemire_core::error::NotFound::Connection.into()));
    assert_eq!((failed.label.as_str(), failed.status_is_error), ("3 · !", true));
    assert!(failed.rows.is_empty() && !failed.status.is_empty(), "the error is said in its status");
}

#[test]
fn a_status_says_when_rows_or_source_values_were_left_out() {
    let plain = status_text(5, false, false);
    assert_ne!(status_text(5, true, false), plain, "LIMIT cut the matches");
    assert!(status_text(5, false, true).starts_with(&plain) && status_text(5, false, true).len() > plain.len());
}
