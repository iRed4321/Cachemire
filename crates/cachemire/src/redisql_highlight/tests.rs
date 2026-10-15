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
