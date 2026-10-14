//! A script's statements: split at the `;`, `var` declarations, and the `&name`
//! variables declarations set inside the strings of the queries after them.

use rustc_hash::FxHashMap;
use std::ops::Range;

use rust_i18n::t;

use super::Located;
use super::lexer::{Tok, Token, tokenize};

fn is_ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Cuts `text[from..]` before a `--` that isn't inside quotes, per line.
fn strip_comments(query: &str) -> String {
    query
        .lines()
        .map(|line| {
            let bytes = line.as_bytes();
            let mut quote: Option<u8> = None;
            for i in 0..bytes.len() {
                match bytes[i] {
                    b'\'' | b'"' if quote.is_none() => quote = Some(bytes[i]),
                    c if Some(c) == quote => quote = None,
                    b'-' if quote.is_none() && bytes.get(i + 1) == Some(&b'-') => return line[..i].trim_end(),
                    _ => {}
                }
            }
            line
        })
        .filter(|line| !line.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Byte ranges of the `&name` variables in `text`, a quoted string's
/// contents: a variable is a whole `:`-separated segment, nothing beside it.
pub fn variable_ranges(text: &str) -> Vec<Range<usize>> {
    let mut start = 0;
    text.split(':')
        .filter_map(|segment| {
            let range = start..start + segment.len();
            start = range.end + 1;
            let name = segment.strip_prefix('&')?;
            (!name.is_empty() && name.chars().all(is_ident_char)).then_some(range)
        })
        .collect()
}

/// Whether a statement is a `var` declaration rather than a query.
pub fn is_declaration(statement: &str) -> bool {
    tokenize(statement).is_ok_and(|tokens| tokens.first().is_some_and(|t| t.is_keyword("var")))
}

/// One statement of a script: where it sits in the text (from after the previous `;` to its
/// own, comments and blank space included) and its text with the comments dropped.
pub struct Statement {
    pub range: Range<usize>,
    pub text: String,
}

/// Splits `text` at the `;` outside quotes and comments, leaving out the statements with nothing in them.
pub fn split_statements(text: &str) -> Vec<Statement> {
    let mut statements = Vec::new();
    let mut quote: Option<char> = None;
    let mut comment = false;
    let mut start = 0;
    let mut chars = text.char_indices().peekable();
    let push = |range: Range<usize>, statements: &mut Vec<Statement>| {
        let cleaned = strip_comments(&text[range.clone()]);
        if !cleaned.trim().is_empty() {
            statements.push(Statement { range, text: cleaned.trim().to_string() });
        }
    };
    while let Some((i, c)) = chars.next() {
        match (quote, comment, c) {
            (_, true, '\n') => comment = false,
            (_, true, _) => {}
            (Some(q), _, _) if c == q => quote = None,
            (Some(_), _, _) => {}
            (None, _, '\'' | '"') => quote = Some(c),
            (None, _, '-') if chars.peek().is_some_and(|(_, next)| *next == '-') => comment = true,
            (None, _, ';') => {
                push(start..i, &mut statements);
                start = i + 1;
            }
            _ => {}
        }
    }
    push(start..text.len(), &mut statements);
    statements
}

/// Replaces every `&name` variable inside the strings of `tokens` by its value;
/// an unknown one is pointed at where it is written.
pub fn substitute_variables(tokens: &mut [Token], vars: &FxHashMap<String, String>) -> Result<(), Located> {
    for token in tokens {
        let Tok::Str(text) = &token.tok else { continue };
        let ranges = variable_ranges(text);
        if ranges.is_empty() {
            continue;
        }
        let mut out = String::with_capacity(text.len());
        let mut pos = 0;
        for range in ranges {
            out.push_str(&text[pos..range.start]);
            let name = &text[range.start + 1..range.end];
            let Some(value) = vars.get(name) else {
                let at = token.span.start + 1;
                return Err(Located::parse(t!("Unknown variable: &%{name}", name = name), Some(at + range.start..at + range.end)));
            };
            out.push_str(value);
            pos = range.end;
        }
        out.push_str(&text[pos..]);
        token.tok = Tok::Str(out);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_comments_but_keeps_quoted_dashes() {
        let cleaned = strip_comments("FROM KEY 'a--b' AS x -- a trailing comment\nSELECT *");
        assert!(cleaned.contains("'a--b'"));
        assert!(!cleaned.contains("trailing comment"));
    }
}
