use super::*;
use crate::test_support::{block_on, temp_dir};

// every script here fails before a query would reach the server
fn run(script: &str) -> Vec<StatementResult> {
    let state = AppState::for_tests(&temp_dir("redisql"));
    block_on(run_script(&state, None, script))
}

#[test]
fn a_script_of_declarations_only_gives_no_result() {
    assert!(run("var a = 1; var b = 'x'; -- nothing to run").is_empty());
}

#[test]
fn an_error_points_into_the_whole_script_and_stops_it() {
    let script = "var id = 7;\nFROM KEY 'user:&id' AS u WHERE u.x = 1 ORDER BY x;\nFROM KEY 'never:run'";
    let results = run(script);
    assert_eq!(results.len(), 1, "nothing runs past the first error");
    assert!(results[0].output.is_err());
    assert_eq!(&script[results[0].error_span.clone().unwrap()], "ORDER");
}

#[test]
fn a_variable_is_known_only_after_its_declaration() {
    let script = "FROM KEY 'user:&id';\nvar id = 7";
    let results = run(script);
    assert_eq!(&script[results[0].error_span.clone().unwrap()], "&id");
}

#[test]
fn a_bad_declaration_is_reported_where_it_is() {
    let script = "var ok = 1;\n  var broken 2";
    let results = run(script);
    assert_eq!(results.len(), 1);
    let span = results[0].error_span.clone().unwrap();
    assert!(span.start >= script.find("var broken").unwrap(), "{span:?} is before the declaration");
}

#[test]
fn an_unterminated_quote_is_reported() {
    let script = "FROM KEY 'oops";
    let results = run(script);
    assert_eq!(&script[results[0].error_span.clone().unwrap()], "'oops");
}
