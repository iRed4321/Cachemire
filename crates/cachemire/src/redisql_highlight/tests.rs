use super::*;

fn kinds(line: &str) -> Vec<(&str, &str)> {
    highlight_line(line).into_iter().map(|(range, color)| (&line[range], color_name(color))).collect()
}

fn color_name(color: Color) -> &'static str {
    match color {
        c if c == KEYWORD_COLOR => "keyword",
        c if c == STRING_COLOR => "string",
        c if c == NUMBER_COLOR => "number",
        c if c == BOOL_COLOR => "bool",
        c if c == PUNCT_COLOR => "punct",
        c if c == COMMENT_COLOR => "comment",
        c if c == VAR_COLOR => "variable",
        _ => "?",
    }
}

#[test]
fn clause_keywords_are_recognized_whatever_the_case() {
    assert_eq!(kinds("FROM KEY 'k' AS x"), vec![("FROM", "keyword"), ("KEY", "keyword"), ("'k'", "string"), ("AS", "keyword")]);
    assert_eq!(kinds("from key 'k' as x"), vec![("from", "keyword"), ("key", "keyword"), ("'k'", "string"), ("as", "keyword")]);
}

#[test]
fn a_word_that_only_starts_with_a_keyword_is_not_one() {
    assert_eq!(kinds("SELECT selected"), vec![("SELECT", "keyword")]);
}

#[test]
fn strings_numbers_and_booleans_are_colored() {
    assert_eq!(kinds("WHERE x.qty = 3 AND x.ok = true"), vec![
        ("WHERE", "keyword"),
        (".", "punct"),
        ("=", "punct"),
        ("3", "number"),
        ("AND", "keyword"),
        (".", "punct"),
        ("=", "punct"),
        ("true", "bool"),
    ]);
    assert_eq!(kinds("x.label = 'a,b'"), vec![(".", "punct"), ("=", "punct"), ("'a,b'", "string")]);
}

#[test]
fn a_double_dash_comment_runs_to_end_of_line_but_not_inside_a_string() {
    assert_eq!(kinds("SELECT * -- everything"), vec![("SELECT", "keyword"), ("*", "punct"), ("-- everything", "comment")]);
    assert_eq!(kinds("FROM KEY 'a--b' AS x"), vec![("FROM", "keyword"), ("KEY", "keyword"), ("'a--b'", "string"), ("AS", "keyword")]);
}

#[test]
fn an_in_list_colors_its_brackets_and_values() {
    assert_eq!(kinds("WHERE x.id IN [1, 2, 'c']"), vec![
        ("WHERE", "keyword"),
        (".", "punct"),
        ("IN", "keyword"),
        ("[", "punct"),
        ("1", "number"),
        (",", "punct"),
        ("2", "number"),
        (",", "punct"),
        ("'c'", "string"),
        ("]", "punct"),
    ]);
}

#[test]
fn an_unterminated_string_still_colors_to_the_end_of_the_line() {
    assert_eq!(kinds("FROM KEY 'unterminated"), vec![("FROM", "keyword"), ("KEY", "keyword"), ("'unterminated", "string")]);
}

#[test]
fn plain_identifiers_and_whitespace_get_no_span_but_the_dot_between_them_does() {
    assert_eq!(kinds("i.productId p.label"), vec![(".", "punct"), (".", "punct")]);
}

#[test]
fn a_trailing_carriage_return_is_not_colored_as_part_of_the_line() {
    // a pasted Windows query still has `\r\n` for one frame, before
    // `normalize_line_endings` catches up (see redisql::init)
    assert_eq!(kinds("FROM KEY 'k'\r"), vec![("FROM", "keyword"), ("KEY", "keyword"), ("'k'", "string")]);
}

#[test]
fn crlf_and_lone_cr_both_collapse_to_lf() {
    assert_eq!(normalize_line_endings("FROM KEY 'k'\r\nSELECT *\r\nLIMIT 1"), "FROM KEY 'k'\nSELECT *\nLIMIT 1");
    assert_eq!(normalize_line_endings("a\rb"), "a\nb");
    assert_eq!(normalize_line_endings("no newlines here"), "no newlines here");
}

/// The text the error mark covers on each line it touches.
fn marked(text: &str, error: Range<usize>) -> Vec<String> {
    highlighted_lines(text, Some(error)).into_iter().filter_map(|(line, _, marked)| marked.map(|r| line[r].to_string())).collect()
}

#[test]
fn the_error_is_marked_on_the_line_it_is_on() {
    let text = "FROM KEY 'a'\nORDER BY x";
    let at = text.find("ORDER").unwrap();
    assert_eq!(marked(text, at..at + 5), ["ORDER"]);
}

#[test]
fn an_error_over_several_lines_is_marked_on_each() {
    let text = "FROM KEY 'a' AS a\nJOIN KEY 'b' AS b ON a.id\n= b.ref";
    let (start, end) = (text.find("a.id").unwrap(), text.len());
    assert_eq!(marked(text, start..end), ["a.id", "= b.ref"]);
}

#[test]
fn a_carriage_return_moves_no_mark() {
    let text = "FROM KEY 'a'\r\nLIMIT x";
    let at = text.find('x').unwrap();
    assert_eq!(marked(text, at..at + 1), ["x"]);
}

#[test]
fn an_empty_error_marks_nothing() {
    assert!(marked("FROM KEY 'a'", 3..3).is_empty());
    assert!(highlighted_lines("a\nb", None).iter().all(|(_, _, marked)| marked.is_none()));
}

#[test]
fn a_variable_inside_a_string_has_its_own_color() {
    assert_eq!(kinds("'user:&id:x'"), [("'user:", "string"), ("&id", "variable"), (":x'", "string")]);
    assert_eq!(kinds("'&id'"), [("'", "string"), ("&id", "variable"), ("'", "string")]);
    assert_eq!(kinds("'a&id'"), [("'a&id'", "string")], "only a whole segment is a variable");
}
