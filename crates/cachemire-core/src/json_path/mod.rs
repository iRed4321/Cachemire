//! A small JSONPath-style query for the key panel's filter bar: keys, `*`, `[0]`,
//! `["quoted key"]`, `..key` at any depth, an optional leading `$`. `value` names a
//! field's own value in the result table unless a real key is called `value`.

use serde_json::Value;

/// Why a filter can't be understood.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ParseError {
    #[error("expected a key, found {0}")]
    ExpectedKey(Found),
    #[error("a quoted key is missing its closing quote")]
    UnclosedQuote,
    #[error("'{0}' is too big an index")]
    IndexTooBig(String),
    #[error("expected *, an index or a quoted key after '[', found {0}")]
    ExpectedInBrackets(Found),
    #[error("expected ']', found {0}")]
    ExpectedClosingBracket(Found),
    #[error("expected '.' or '[', found {0}")]
    ExpectedSeparator(Found),
}

/// What a filter has where something else was expected.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Found {
    /// a character, at its position counted from 1
    Char { c: char, position: usize },
    End,
}

impl std::fmt::Display for Found {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Found::Char { c, position } => write!(f, "'{c}' at position {position}"),
            Found::End => f.write_str("the end of the filter"),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Segment {
    /// a key of an object; on an array, a numeric key is an index
    Key(String),
    Index(usize),
    Wildcard,
    /// `..x`: `x` matched at the current value and at every value below it
    Descend(Box<Segment>),
}

impl Segment {
    fn apply<'a>(&self, value: &'a Value, out: &mut Vec<&'a Value>) {
        match self {
            Segment::Key(key) => match value {
                Value::Object(object) => out.extend(object.get(key)),
                Value::Array(items) => {
                    if let Ok(index) = key.parse::<usize>() {
                        out.extend(items.get(index));
                    }
                }
                _ => {}
            },
            Segment::Index(index) => {
                if let Value::Array(items) = value {
                    out.extend(items.get(*index));
                }
            }
            Segment::Wildcard => match value {
                Value::Object(object) => out.extend(object.values()),
                Value::Array(items) => out.extend(items.iter()),
                _ => {}
            },
            Segment::Descend(inner) => {
                inner.apply(value, out);
                match value {
                    Value::Object(object) => object.values().for_each(|child| self.apply(child, out)),
                    Value::Array(items) => items.iter().for_each(|child| self.apply(child, out)),
                    _ => {}
                }
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct JsonPath {
    segments: Vec<Segment>,
}

impl JsonPath {
    /// `$` alone: selects the value itself.
    pub fn is_identity(&self) -> bool {
        self.segments.is_empty()
    }

    /// The path is just `value`: the name the field/value layout gives a
    /// field's value.
    pub fn is_value_alias_only(&self) -> bool {
        matches!(self.segments.as_slice(), [Segment::Key(key)] if key == "value")
    }

    /// This path without a leading `value` key, when it starts with one:
    /// `value.records` read as `records`.
    pub fn without_value_alias(&self) -> Option<JsonPath> {
        match self.segments.first() {
            Some(Segment::Key(key)) if key == "value" => Some(JsonPath { segments: self.segments[1..].to_vec() }),
            _ => None,
        }
    }

    /// This path without its trailing `*`, when it ends with one: `records.*`
    /// as `records`.
    pub fn without_trailing_wildcard(&self) -> Option<JsonPath> {
        match self.segments.last() {
            Some(Segment::Wildcard) => Some(JsonPath { segments: self.segments[..self.segments.len() - 1].to_vec() }),
            _ => None,
        }
    }

    /// Everything the path matches in `root`, in document order.
    pub fn select<'a>(&self, root: &'a Value) -> Vec<&'a Value> {
        let mut current = vec![root];
        for segment in &self.segments {
            let mut next = Vec::new();
            for value in current {
                segment.apply(value, &mut next);
            }
            if next.is_empty() {
                return next;
            }
            current = next;
        }
        current
    }
}

fn found(chars: &[char], at: usize) -> Found {
    chars.get(at).map_or(Found::End, |&c| Found::Char { c, position: at + 1 })
}

fn parse_name(chars: &[char], i: &mut usize) -> Result<Segment, ParseError> {
    let start = *i;
    while *i < chars.len() && !matches!(chars[*i], '.' | '[' | ']') {
        *i += 1;
    }
    let name: String = chars[start..*i].iter().collect();
    let name = name.trim();
    if name.is_empty() {
        return Err(ParseError::ExpectedKey(found(chars, start)));
    }
    Ok(if name == "*" { Segment::Wildcard } else { Segment::Key(name.to_string()) })
}

/// `[*]`, `[3]`, `['key']` / `["key"]`, with `*i` at the opening bracket.
fn parse_bracket(chars: &[char], i: &mut usize) -> Result<Segment, ParseError> {
    *i += 1;
    let skip_spaces = |i: &mut usize| {
        while *i < chars.len() && chars[*i] == ' ' {
            *i += 1;
        }
    };
    skip_spaces(i);
    let segment = match chars.get(*i) {
        Some('*') => {
            *i += 1;
            Segment::Wildcard
        }
        Some(&quote @ ('\'' | '"')) => {
            let start = *i + 1;
            let Some(len) = chars[start..].iter().position(|c| *c == quote) else {
                return Err(ParseError::UnclosedQuote);
            };
            *i = start + len + 1;
            Segment::Key(chars[start..start + len].iter().collect())
        }
        Some(c) if c.is_ascii_digit() => {
            let start = *i;
            while *i < chars.len() && chars[*i].is_ascii_digit() {
                *i += 1;
            }
            let digits: String = chars[start..*i].iter().collect();
            Segment::Index(digits.parse().map_err(|_| ParseError::IndexTooBig(digits))?)
        }
        _ => return Err(ParseError::ExpectedInBrackets(found(chars, *i))),
    };
    skip_spaces(i);
    if chars.get(*i) != Some(&']') {
        return Err(ParseError::ExpectedClosingBracket(found(chars, *i)));
    }
    *i += 1;
    Ok(segment)
}

/// Parses a filter. The empty string is not a filter: callers check for it
/// first (`$` is the explicit "the whole value").
pub fn parse(query: &str) -> Result<JsonPath, ParseError> {
    let chars: Vec<char> = query.trim().chars().collect();
    let mut i = usize::from(chars.first() == Some(&'$'));
    // a path may begin with a key directly (`records.*`); after `$` or a
    // segment a separator is needed
    let mut needs_separator = i == 1;
    let mut segments = Vec::new();

    while i < chars.len() {
        let mut descend = false;
        match chars[i] {
            '.' => {
                i += 1;
                if chars.get(i) == Some(&'.') {
                    descend = true;
                    i += 1;
                }
            }
            '[' => {}
            _ if !needs_separator => {}
            _ => return Err(ParseError::ExpectedSeparator(found(&chars, i))),
        }
        let segment = if chars.get(i) == Some(&'[') { parse_bracket(&chars, &mut i)? } else { parse_name(&chars, &mut i)? };
        segments.push(if descend { Segment::Descend(Box::new(segment)) } else { segment });
        needs_separator = true;
    }
    Ok(JsonPath { segments })
}

#[cfg(test)]
mod tests;
