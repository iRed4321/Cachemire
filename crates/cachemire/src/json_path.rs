//! A small JSONPath-style query for the key panel's filter bar: keys, `*`, `[0]`,
//! `["quoted key"]`, `..key` at any depth, an optional leading `$`. `value` names a
//! field's own value in the result table unless a real key is called `value`.

use rust_i18n::t;

use crate::backend::error::AppError;
use serde_json::Value;

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

fn describe(chars: &[char], at: usize) -> String {
    match chars.get(at) {
        Some(c) => t!("'%{c}' at position %{position}", c = c, position = at + 1).into_owned(),
        None => t!("the end of the filter").into_owned(),
    }
}

fn parse_name(chars: &[char], i: &mut usize) -> Result<Segment, AppError> {
    let start = *i;
    while *i < chars.len() && !matches!(chars[*i], '.' | '[' | ']') {
        *i += 1;
    }
    let name: String = chars[start..*i].iter().collect();
    let name = name.trim();
    if name.is_empty() {
        return Err(AppError::parse(t!("expected a key, found %{found}", found = describe(chars, start))));
    }
    Ok(if name == "*" { Segment::Wildcard } else { Segment::Key(name.to_string()) })
}

/// `[*]`, `[3]`, `['key']` / `["key"]`, with `*i` at the opening bracket.
fn parse_bracket(chars: &[char], i: &mut usize) -> Result<Segment, AppError> {
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
                return Err(AppError::parse(t!("a quoted key is missing its closing quote")));
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
            Segment::Index(digits.parse().map_err(|_| AppError::parse(t!("'%{digits}' is too big an index", digits = digits)))?)
        }
        _ => return Err(AppError::parse(t!("expected *, an index or a quoted key after '[', found %{found}", found = describe(chars, *i)))),
    };
    skip_spaces(i);
    if chars.get(*i) != Some(&']') {
        return Err(AppError::parse(t!("expected ']', found %{found}", found = describe(chars, *i))));
    }
    *i += 1;
    Ok(segment)
}

/// Parses a filter. The empty string is not a filter: callers check for it
/// first (`$` is the explicit "the whole value").
pub fn parse(query: &str) -> Result<JsonPath, AppError> {
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
            _ => return Err(AppError::parse(t!("expected '.' or '[', found %{found}", found = describe(&chars, i)))),
        }
        let segment = if chars.get(i) == Some(&'[') { parse_bracket(&chars, &mut i)? } else { parse_name(&chars, &mut i)? };
        segments.push(if descend { Segment::Descend(Box::new(segment)) } else { segment });
        needs_separator = true;
    }
    Ok(JsonPath { segments })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn select(query: &str, value: &Value) -> Vec<Value> {
        parse(query).unwrap().select(value).into_iter().cloned().collect()
    }

    fn sample() -> Value {
        json!({
            "bar": [{"foo": 1, "x": "a"}, {"foo": 2, "x": "b"}, {"other": true}],
            "name": "n",
            "nested": {"deep": {"foo": 9}},
        })
    }

    #[test]
    fn keys_wildcards_and_indexes() {
        let v = sample();
        assert_eq!(select("name", &v), vec![json!("n")]);
        assert_eq!(select("bar.*.foo", &v), vec![json!(1), json!(2)], "items without the key are skipped");
        assert_eq!(select("bar[*].x", &v), vec![json!("a"), json!("b")]);
        assert_eq!(select("bar[1].foo", &v), vec![json!(2)]);
        assert_eq!(select("bar.1.foo", &v), vec![json!(2)], "a numeric key indexes an array");
        assert_eq!(select("bar", &v).len(), 1);
        assert_eq!(select("nested.*", &v), vec![json!({"foo": 9})]);
        assert!(select("bar.7.foo", &v).is_empty());
        assert!(select("missing.foo", &v).is_empty());
        assert!(select("name.foo", &v).is_empty(), "a string has no keys");
    }

    #[test]
    fn root_marker_quoted_keys_and_descent() {
        let v = sample();
        assert_eq!(select("$", &v), vec![v.clone()]);
        assert_eq!(select("$.name", &v), vec![json!("n")]);
        assert_eq!(select("$['name']", &v), vec![json!("n")]);
        assert_eq!(select(".name", &v), vec![json!("n")]);
        assert_eq!(select("..foo", &v), vec![json!(1), json!(2), json!(9)], "at any depth, in document order");
        assert_eq!(select("nested..foo", &v), vec![json!(9)]);
        assert_eq!(select("bar..*", &v).len(), 3 + 2 + 2 + 1, "every value below bar, and bar's own items");
        assert!(parse("$").unwrap().is_identity());
        assert!(!parse("a").unwrap().is_identity());
    }

    #[test]
    fn value_alias_and_trailing_wildcard_helpers() {
        assert!(parse("value").unwrap().is_value_alias_only());
        assert!(!parse("value.a").unwrap().is_value_alias_only());
        assert!(parse("value.a").unwrap().without_value_alias().unwrap() == parse("a").unwrap());
        assert!(parse("value").unwrap().without_value_alias().unwrap().is_identity());
        assert!(parse("a.value").unwrap().without_value_alias().is_none());
        assert!(parse("a.*").unwrap().without_trailing_wildcard().unwrap() == parse("a").unwrap());
        assert!(parse("*").unwrap().without_trailing_wildcard().unwrap().is_identity());
        assert!(parse("a.*.b").unwrap().without_trailing_wildcard().is_none());
        assert!(parse("a..*").unwrap().without_trailing_wildcard().is_none(), "a descent isn't a plain trailing *");
    }

    #[test]
    fn spaces_are_forgiven_and_mistakes_are_explained() {
        let v = sample();
        assert_eq!(select("  bar.*.foo  ", &v), vec![json!(1), json!(2)]);
        assert_eq!(select("bar[ 0 ].foo", &v), vec![json!(1)]);
        for bad in ["bar.", "bar..", ".", "bar[", "bar[x]", "bar[1", "bar['x]", "$name", "bar]", "bar.*.[0"] {
            let error = parse(bad).unwrap_err();
            assert!(!error.message().is_empty(), "{bad}");
        }
        assert!(parse("bar.").unwrap_err().message().contains("expected a key"));
        assert!(parse("bar[x]").unwrap_err().message().contains("after '['"));
    }
}
