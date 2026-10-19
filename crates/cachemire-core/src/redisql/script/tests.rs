use super::*;

#[test]
fn strips_comments_but_keeps_quoted_dashes() {
    let cleaned = strip_comments("FROM KEY 'a--b' AS x -- a trailing comment\nSELECT *");
    assert!(cleaned.contains("'a--b'"));
    assert!(!cleaned.contains("trailing comment"));
}

fn texts(script: &str) -> Vec<String> {
    split_statements(script).into_iter().map(|s| s.text).collect()
}

#[test]
fn statements_split_at_semicolons_outside_quotes_and_comments() {
    let script = "var a = 'x;y';\nFROM KEY 'k;1' AS k -- not; here\nSELECT *;\n;  ;\n-- only a comment;\nFROM KEY \"q\" AS q";
    assert_eq!(texts(script), ["var a = 'x;y'", "FROM KEY 'k;1' AS k\nSELECT *", "FROM KEY \"q\" AS q"]);
}

#[test]
fn a_statements_range_points_into_the_script() {
    let script = "var a = 1;\n  FROM KEY 'k' AS k";
    let statements = split_statements(script);
    assert_eq!(&script[statements[1].range.clone()], "\n  FROM KEY 'k' AS k");
}

#[test]
fn an_unclosed_quote_keeps_the_rest_of_the_script_in_one_statement() {
    assert_eq!(texts("FROM KEY 'k;still inside"), ["FROM KEY 'k;still inside"]);
}

#[test]
fn a_variable_is_a_whole_colon_separated_segment() {
    let text = "user:&id:orders:&id2x:a&b:&:&_ok";
    let found: Vec<&str> = variable_ranges(text).into_iter().map(|r| &text[r]).collect();
    assert_eq!(found, ["&id", "&id2x", "&_ok"]);
    assert!(variable_ranges("&a-b").is_empty());
}

#[test]
fn variables_are_replaced_inside_strings_only() {
    let vars: FxHashMap<String, String> = [("id".to_string(), "42".to_string())].into_iter().collect();
    let mut tokens = tokenize("FROM KEY 'user:&id' AS u WHERE u.id = '&id'").unwrap();
    substitute_variables(&mut tokens, &vars).unwrap();
    let strings: Vec<&Tok> = tokens.iter().filter(|t| matches!(t.tok, Tok::Str(_))).map(|t| &t.tok).collect();
    assert_eq!(strings, [&Tok::Str("user:42".into()), &Tok::Str("42".into())]);
}

#[test]
fn an_unknown_variable_is_pointed_at_where_it_is_written() {
    let text = "FROM KEY 'user:&nope' AS u";
    let mut tokens = tokenize(text).unwrap();
    let error = substitute_variables(&mut tokens, &FxHashMap::default()).unwrap_err();
    assert_eq!(&text[error.span.unwrap()], "&nope");
}

#[test]
fn only_a_statement_starting_with_var_is_a_declaration() {
    assert!(is_declaration("VAR a = 1"));
    assert!(is_declaration("  var a = 'x'"));
    assert!(!is_declaration("FROM KEY 'var' AS v"));
    assert!(!is_declaration("variable = 1"));
    assert!(!is_declaration("var 'unclosed"));
}
