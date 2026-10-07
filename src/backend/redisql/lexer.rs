//! Cuts a statement into tokens, each with the byte range it covers, skipping
//! whitespace and `--` comments.

use std::ops::Range;

use rust_i18n::t;

use super::Located;

#[derive(Debug, Clone, PartialEq)]
pub enum Tok {
    /// a name or a keyword (matched whatever its case), or a bare value like `true`
    Word(String),
    /// a bare number, or a value starting like one (`2024-01-01`)
    Number(String),
    /// a `'...'` or `"..."` string, without its quotes
    Str(String),
    Dot,
    Comma,
    Eq,
    Star,
    Open(char),
    Close(char),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    pub tok: Tok,
    pub span: Range<usize>,
}

impl Token {
    /// Whether this is the keyword `word`, whatever its case.
    pub fn is_keyword(&self, word: &str) -> bool {
        matches!(&self.tok, Tok::Word(w) if w.eq_ignore_ascii_case(word))
    }
}

fn is_word_start(c: char) -> bool {
    c.is_alphabetic() || c == '_'
}

/// What a bare word or number may hold past its first character: a name's
/// characters, plus `-` and `:` for values like `user:42` or `2024-01-01`.
fn is_bare_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '_' | '-' | ':' | '.')
}

/// The end of the bare token starting at `start`; a word stops at a `.` (an
/// `alias.field`), a number goes on through it (`1.5`), and both stop at a `--`.
fn bare_end(text: &str, start: usize, number: bool) -> usize {
    let mut end = start;
    for (i, c) in text[start..].char_indices() {
        let at = start + i;
        let stop = !is_bare_char(c) || (c == '.' && !number) || (i > 0 && text[at..].starts_with("--"));
        if i > 0 && stop {
            break;
        }
        end = at + c.len_utf8();
    }
    end
}

pub fn tokenize(text: &str) -> Result<Vec<Token>, Located> {
    let mut tokens = Vec::new();
    let mut i = 0;
    while let Some(c) = text[i..].chars().next() {
        let token = |tok: Tok, len: usize| Token { tok, span: i..i + len };
        if c.is_whitespace() {
            i += c.len_utf8();
        } else if text[i..].starts_with("--") {
            i = text[i..].find('\n').map_or(text.len(), |n| i + n);
        } else if c == '\'' || c == '"' {
            let Some(len) = text[i + 1..].find(c) else {
                let line_end = text[i..].find('\n').map_or(text.len(), |n| i + n);
                let message = t!("Unterminated quote: %{text}", text = &text[i..line_end]);
                return Err(Located::parse(message, Some(i..line_end)));
            };
            tokens.push(token(Tok::Str(text[i + 1..i + 1 + len].to_string()), len + 2));
            i += len + 2;
        } else if c.is_ascii_digit() || (c == '-' && text[i + 1..].starts_with(|n: char| n.is_ascii_digit())) {
            let end = bare_end(text, i, true);
            tokens.push(token(Tok::Number(text[i..end].to_string()), end - i));
            i = end;
        } else if is_word_start(c) {
            let end = bare_end(text, i, false);
            tokens.push(token(Tok::Word(text[i..end].to_string()), end - i));
            i = end;
        } else {
            let tok = match c {
                '.' => Tok::Dot,
                ',' => Tok::Comma,
                '=' => Tok::Eq,
                '*' => Tok::Star,
                '[' | '(' => Tok::Open(c),
                ']' | ')' => Tok::Close(c),
                _ => {
                    let span = i..i + c.len_utf8();
                    return Err(Located::parse(t!("Unexpected text: %{text}", text = &text[span.clone()]), Some(span)));
                }
            };
            tokens.push(token(tok, c.len_utf8()));
            i += c.len_utf8();
        }
    }
    Ok(tokens)
}
