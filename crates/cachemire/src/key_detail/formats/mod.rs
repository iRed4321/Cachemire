//! The formats a value is shown in and edited as, beyond JSON: YAML, XML and raw text.
//! Their text for a field, its colors a line at a time, whether it is valid, and the
//! JSON-like value a YAML or XML text stands for (what the table reads).

use std::borrow::Cow;
use std::ops::Range;

use crate::t;
use serde_json::{Map, Value};

use super::color::color_for_kind;
use super::field::Field;
use super::pretty_text::{ColorSpans, ColoredParagraph, colored_paragraphs_for, escaped_paragraphs, pretty_paragraphs_for};

/// What a row shows its value as, in the order of the "Show as" menus (Slint's `Formats.names`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub(super) enum ShowFormat {
    #[default]
    Json,
    Escaped,
    Yaml,
    Xml,
    Text,
}

impl ShowFormat {
    pub(super) const COUNT: usize = 5;

    /// The format at `index` of the menus, JSON for one out of them.
    pub(super) fn from_index(index: i32) -> ShowFormat {
        match index {
            1 => ShowFormat::Escaped,
            2 => ShowFormat::Yaml,
            3 => ShowFormat::Xml,
            4 => ShowFormat::Text,
            _ => ShowFormat::Json,
        }
    }

    pub(super) fn index(self) -> usize {
        self as usize
    }

    /// The extension of a file holding a value in this format.
    pub(super) fn extension(self) -> &'static str {
        match self {
            ShowFormat::Json => "json",
            ShowFormat::Escaped | ShowFormat::Text => "txt",
            ShowFormat::Yaml => "yaml",
            ShowFormat::Xml => "xml",
        }
    }
}

/// What the editor takes its text as, in the order of its menu (Slint's `ValueEditor.kinds`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum EditKind {
    #[default]
    Json,
    Yaml,
    Xml,
    Text,
}

impl EditKind {
    pub(super) fn from_index(index: i32) -> EditKind {
        match index {
            1 => EditKind::Yaml,
            2 => EditKind::Xml,
            3 => EditKind::Text,
            _ => EditKind::Json,
        }
    }

    pub(super) fn index(self) -> i32 {
        self as i32
    }
}

/// Whether `field`'s value is a JSON object or array.
fn is_json_document(field: &Field) -> bool {
    matches!(field.json, Some(Value::Object(_) | Value::Array(_)))
}

/// The kind the editor opens `field` as from a row showing `format`: the row's, except
/// that a JSON object or array shown as YAML is still edited as the JSON it is.
pub(super) fn edit_kind_for(field: &Field, format: ShowFormat) -> EditKind {
    match format {
        ShowFormat::Json | ShowFormat::Escaped => EditKind::Json,
        ShowFormat::Yaml if is_json_document(field) => EditKind::Json,
        ShowFormat::Yaml => EditKind::Yaml,
        ShowFormat::Xml => EditKind::Xml,
        ShowFormat::Text => EditKind::Text,
    }
}

/// `value` written as YAML, without the line break that ends it.
pub(super) fn to_yaml(value: &Value) -> Option<String> {
    let mut yaml = serde_norway::to_string(value).ok()?;
    yaml.truncate(yaml.trim_end().len());
    Some(yaml)
}

/// `field` as YAML: a JSON object or array converted, any other value as its own text.
fn yaml_text(field: &Field) -> Cow<'_, str> {
    match &field.json {
        Some(json @ (Value::Object(_) | Value::Array(_))) => to_yaml(json).map_or(Cow::Borrowed(field.raw.as_str()), Cow::Owned),
        _ => Cow::Borrowed(&field.raw),
    }
}

/// `raw` as XML: indented when it is XML written on a single line, as it is otherwise.
fn xml_text(raw: &str) -> Cow<'_, str> {
    let trimmed = raw.trim();
    if trimmed.starts_with('<')
        && !trimmed.contains('\n')
        && let Some(indented) = indent_xml(trimmed)
    {
        return Cow::Owned(indented);
    }
    Cow::Borrowed(raw)
}

/// `field`'s value as text in `format`: what an open row shows, and what is opened, saved or copied.
pub(super) fn shown_text(field: &Field, format: ShowFormat) -> String {
    match format {
        ShowFormat::Json => super::export::pretty_json(field),
        ShowFormat::Escaped => super::export::escaped_json(field),
        ShowFormat::Yaml => yaml_text(field).into_owned(),
        ShowFormat::Xml => xml_text(&field.raw).into_owned(),
        ShowFormat::Text => field.raw.clone(),
    }
}

/// `field`'s paragraphs as a row shows them open in `format`, colored for it.
pub(super) fn paragraphs_for(field: &Field, format: ShowFormat) -> Vec<ColoredParagraph> {
    match format {
        ShowFormat::Json => pretty_paragraphs_for(&field.value()),
        ShowFormat::Escaped => escaped_paragraphs(colored_paragraphs_for(&field.value(), false, usize::MAX)),
        ShowFormat::Yaml => colored_lines(&yaml_text(field), EditKind::Yaml),
        ShowFormat::Xml => colored_lines(&xml_text(&field.raw), EditKind::Xml),
        ShowFormat::Text => colored_lines(&field.raw, EditKind::Text),
    }
}

/// `text` a paragraph per line, colored as `kind` (which need not be valid yet).
pub(super) fn colored_lines(text: &str, kind: EditKind) -> Vec<ColoredParagraph> {
    let text = if text.contains('\r') { Cow::Owned(text.replace('\r', "")) } else { Cow::Borrowed(text) };
    let tokens = match kind {
        EditKind::Json => json_tokens(&text),
        EditKind::Yaml => yaml_tokens(&text),
        EditKind::Xml => xml_tokens(&text),
        EditKind::Text => Vec::new(),
    };
    paragraphs_from_tokens(&text, tokens)
}

/// The lines of `text` colored as `kind` and put end to end, a space between them, each
/// without the spaces around it: a value's own text as a collapsed row shows it.
pub(super) fn colored_single_line(text: &str, kind: EditKind) -> ColoredParagraph {
    let mut line = String::new();
    let mut spans = ColorSpans::new();
    for (text, colors) in colored_lines(text, kind) {
        let (from, to) = (text.len() - text.trim_start().len(), text.trim_end().len());
        if from >= to {
            continue;
        }
        if !line.is_empty() {
            line.push(' ');
        }
        let at = line.len();
        line.push_str(&text[from..to]);
        for (range, color) in colors {
            let (start, end) = (range.start.max(from), range.end.min(to));
            if start < end {
                spans.push((at + start - from..at + end - from, color));
            }
        }
    }
    (line, spans)
}

/// A colored run of a text: its bytes and the kind [`color_for_kind`] knows.
type Token = (Range<usize>, &'static str);

/// The lines of `text`, each with the part of every one of `tokens` (in order) on it.
fn paragraphs_from_tokens(text: &str, tokens: Vec<Token>) -> Vec<ColoredParagraph> {
    let mut paragraphs = Vec::new();
    let mut next = 0;
    let mut start = 0;
    for line in text.split('\n') {
        let end = start + line.len();
        let mut spans = ColorSpans::new();
        while let Some((range, kind)) = tokens.get(next) {
            if range.start >= end {
                break;
            }
            let (from, to) = (range.start.max(start), range.end.min(end));
            if let Some(color) = color_for_kind(kind)
                && to > from
            {
                spans.push((from - start..to - start, color));
            }
            // a run going on past this line is met again on the next one
            if range.end > end {
                break;
            }
            next += 1;
        }
        paragraphs.push((line.to_string(), spans));
        start = end + 1;
    }
    paragraphs
}

/// The end of the number starting at `at`.
fn number_end(bytes: &[u8], at: usize) -> usize {
    at + bytes[at..].iter().take_while(|b| b.is_ascii_digit() || matches!(b, b'-' | b'+' | b'.' | b'e' | b'E')).count()
}

/// The end of the string whose opening quote is at `at`: past its closing quote, or
/// at the end of its line when it isn't closed there.
fn string_end(bytes: &[u8], at: usize) -> usize {
    let mut i = at + 1;
    while i < bytes.len() && bytes[i] != b'"' && bytes[i] != b'\n' {
        i += if bytes[i] == b'\\' && bytes.get(i + 1).is_some_and(|b| *b != b'\n') { 2 } else { 1 };
    }
    if bytes.get(i) == Some(&b'"') { i + 1 } else { i.min(bytes.len()) }
}

/// The colored runs of `text`, JSON that may not be valid yet: strings (a key when a
/// `:` follows), numbers, `true`/`false`, and punctuation; anything else is left plain.
fn json_tokens(text: &str) -> Vec<Token> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let (end, kind) = match bytes[i] {
            b'"' => {
                let end = string_end(bytes, i);
                let key = text[end..].trim_start().starts_with(':');
                (end, if key { "key" } else { "string" })
            }
            b'-' | b'0'..=b'9' => (number_end(bytes, i), "number"),
            b'{' | b'}' | b'[' | b']' | b',' | b':' => (i + 1, "punct"),
            _ if bytes[i..].starts_with(b"true") => (i + 4, "bool"),
            _ if bytes[i..].starts_with(b"false") => (i + 5, "bool"),
            _ => {
                i += 1;
                continue;
            }
        };
        out.push((i..end, kind));
        i = end;
    }
    out
}

/// Where the `:` ending a YAML key is in `rest` (a line past its indent and dashes), if it
/// starts with a key: a `:` before a space or the line's end, outside quotes.
fn yaml_key_colon(rest: &str) -> Option<usize> {
    let bytes = rest.as_bytes();
    let mut i = 0;
    if let Some(quote @ (b'"' | b'\'')) = bytes.first().copied() {
        i = 1 + bytes[1..].iter().position(|b| *b == quote)? + 1;
    } else if matches!(bytes.first(), Some(b'{' | b'[' | b'#' | b'|' | b'>')) {
        return None;
    }
    while i < bytes.len() {
        match bytes[i] {
            b':' if matches!(bytes.get(i + 1), None | Some(b' ')) => return Some(i),
            b'#' if i > 0 && bytes[i - 1] == b' ' => return None,
            _ => i += 1,
        }
    }
    None
}

/// Where a ` #` comment starts in the YAML scalar `value`, outside quotes.
fn yaml_comment_start(value: &str) -> Option<usize> {
    let bytes = value.as_bytes();
    let mut quote = None;
    for (i, b) in bytes.iter().copied().enumerate() {
        match (quote, b) {
            (None, b'"' | b'\'') if i == 0 => quote = Some(b),
            (Some(q), b) if b == q => quote = None,
            (None, b'#') if i == 0 || bytes[i - 1] == b' ' => return Some(i),
            _ => {}
        }
    }
    None
}

/// The kind a YAML scalar is colored as.
fn yaml_scalar_kind(scalar: &str) -> &'static str {
    match scalar {
        "null" | "Null" | "NULL" | "~" => "null",
        "true" | "True" | "TRUE" | "false" | "False" | "FALSE" => "bool",
        _ if scalar.starts_with(['&', '*', '!']) => "punct",
        _ if scalar.parse::<f64>().is_ok() && scalar.starts_with(|c: char| c.is_ascii_digit() || matches!(c, '-' | '+' | '.')) => "number",
        _ => "string",
    }
}

/// The colored runs of `text` read as YAML a line at a time: comments, the dashes of
/// lists, keys, and what follows them (scalars by type, a flow value as JSON, block text).
fn yaml_tokens(text: &str) -> Vec<Token> {
    let mut out = Vec::new();
    // the indent of the line a block scalar (`|` or `>`) started on: deeper lines are its text
    let mut block: Option<usize> = None;
    let mut start = 0;
    for line in text.split('\n') {
        let line_start = start;
        start += line.len() + 1;
        let indent = line.len() - line.trim_start_matches(' ').len();
        let content = line.trim_end();
        if let Some(parent) = block {
            if content.is_empty() || indent > parent {
                out.push((line_start + indent.min(content.len())..line_start + content.len(), "string"));
                continue;
            }
            block = None;
        }
        let mut i = indent.min(content.len());
        if content[i..].starts_with('#') || matches!(&content[i..], "---" | "...") {
            out.push((line_start + i..line_start + content.len(), "punct"));
            continue;
        }
        while content[i..].starts_with("- ") || &content[i..] == "-" {
            out.push((line_start + i..line_start + i + 1, "punct"));
            i += 1;
            i += content[i..].len() - content[i..].trim_start_matches(' ').len();
        }
        if let Some(colon) = yaml_key_colon(&content[i..]) {
            out.push((line_start + i..line_start + i + colon, "key"));
            out.push((line_start + i + colon..line_start + i + colon + 1, "punct"));
            i += colon + 1;
            i += content[i..].len() - content[i..].trim_start_matches(' ').len();
        }
        let value = &content[i..];
        let (value, comment) = match yaml_comment_start(value) {
            Some(at) => (value[..at].trim_end(), Some(at)),
            None => (value, None),
        };
        let at = line_start + i;
        if value.starts_with(['|', '>']) {
            out.push((at..at + value.len(), "punct"));
            block = Some(indent);
        } else if value.starts_with(['{', '[']) {
            out.extend(json_tokens(value).into_iter().map(|(range, kind)| (at + range.start..at + range.end, kind)));
        } else if !value.is_empty() {
            out.push((at..at + value.len(), yaml_scalar_kind(value)));
        }
        if let Some(comment) = comment {
            out.push((at + comment..line_start + content.len(), "punct"));
        }
    }
    out
}

/// Where `needle` is in `text` from `from` on, past its end; the text's end when it isn't there.
fn end_after(text: &str, from: usize, needle: &str) -> usize {
    text[from..].find(needle).map_or(text.len(), |at| from + at + needle.len())
}

/// The end of the XML tag starting at `at` (past its `>`), a `>` inside a quoted value not counting.
fn xml_tag_end(text: &str, at: usize) -> usize {
    let mut quote = None;
    for (i, b) in text.bytes().enumerate().skip(at) {
        match (quote, b) {
            (None, b'"' | b'\'') => quote = Some(b),
            (Some(q), b) if b == q => quote = None,
            (None, b'>') => return i + 1,
            _ => {}
        }
    }
    text.len()
}

/// The end of the piece of markup starting at the `<` at `at`: a comment, a CDATA section,
/// a declaration or a tag.
fn xml_markup_end(text: &str, at: usize) -> usize {
    let rest = &text[at..];
    if rest.starts_with("<!--") {
        end_after(text, at + 4, "-->")
    } else if rest.starts_with("<![CDATA[") {
        end_after(text, at + 9, "]]>")
    } else if rest.starts_with("<?") {
        end_after(text, at + 2, "?>")
    } else {
        xml_tag_end(text, at)
    }
}

/// The colored runs of the tag `text[at..end]`: its brackets, its name, and its
/// attributes' names and quoted values.
fn xml_tag_tokens(text: &str, at: usize, end: usize, out: &mut Vec<Token>) {
    let bytes = text.as_bytes();
    let name_char = |b: u8| !b.is_ascii_whitespace() && !matches!(b, b'<' | b'>' | b'/' | b'=' | b'"' | b'\'');
    let mut i = at;
    let mut named = false;
    while i < end {
        let b = bytes[i];
        if matches!(b, b'<' | b'>' | b'/' | b'=' | b'?' | b'!') {
            out.push((i..i + 1, "punct"));
            i += 1;
        } else if matches!(b, b'"' | b'\'') {
            let close = bytes[i + 1..end].iter().position(|c| *c == b).map_or(end, |p| i + 1 + p + 1);
            out.push((i..close, "string"));
            i = close;
        } else if name_char(b) {
            let stop = i + bytes[i..end].iter().take_while(|c| name_char(**c)).count();
            out.push((i..stop, if named { "bool" } else { "key" }));
            named = true;
            i = stop;
        } else {
            i += 1;
        }
    }
}

/// The colored runs of `text` read as XML: comments and declarations, tags (names,
/// attributes, values), CDATA sections and entities; the text between tags is left plain.
fn xml_tokens(text: &str) -> Vec<Token> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'<' => {
                let end = xml_markup_end(text, i);
                let rest = &text[i..];
                if rest.starts_with("<!--") || rest.starts_with("<?") || (rest.starts_with("<!") && !rest.starts_with("<![CDATA[")) {
                    out.push((i..end, "punct"));
                } else if rest.starts_with("<![CDATA[") {
                    out.push((i..end, "string"));
                } else {
                    xml_tag_tokens(text, i, end, &mut out);
                }
                i = end;
            }
            b'&' => {
                let end = bytes[i..].iter().take(12).position(|b| *b == b';').map(|p| i + p + 1);
                if let Some(end) = end {
                    out.push((i..end, "escape"));
                }
                i = end.unwrap_or(i + 1);
            }
            _ => i += 1,
        }
    }
    out
}

/// `xml` with each tag on its own line, two spaces a level, whatever its lines were; an element
/// holding only text stays as written. `None` when text sits next to elements, or tags don't pair.
fn indent_xml(xml: &str) -> Option<String> {
    #[derive(PartialEq)]
    enum Last {
        Start,
        Open,
        Text,
        Tag,
    }
    let mut out = String::with_capacity(xml.len() + xml.len() / 4);
    let mut depth = 0usize;
    let mut last = Last::Start;
    let mut i = 0;
    let line = |out: &mut String, depth: usize| {
        out.push('\n');
        out.extend(std::iter::repeat_n("  ", depth));
    };
    while i < xml.len() {
        let lt = xml[i..].find('<').map_or(xml.len(), |at| i + at);
        let text = &xml[i..lt];
        if !text.trim().is_empty() {
            if last != Last::Open {
                return None;
            }
            out.push_str(text);
            last = Last::Text;
        }
        if lt == xml.len() {
            break;
        }
        let end = xml_markup_end(xml, lt);
        let markup = &xml[lt..end];
        if !markup.ends_with('>') {
            return None;
        }
        if markup.starts_with("<![CDATA[") {
            if !matches!(last, Last::Open | Last::Text) {
                return None;
            }
            out.push_str(markup);
            last = Last::Text;
        } else if markup.starts_with("</") {
            depth = depth.checked_sub(1)?;
            if last == Last::Tag {
                line(&mut out, depth);
            }
            out.push_str(markup);
            last = Last::Tag;
        } else {
            if last == Last::Text {
                return None;
            }
            if last != Last::Start {
                line(&mut out, depth);
            }
            out.push_str(markup);
            let opens = !markup.starts_with("<!") && !markup.starts_with("<?") && !markup.ends_with("/>");
            if opens {
                depth += 1;
            }
            last = if opens { Last::Open } else { Last::Tag };
        }
        i = end;
    }
    (depth == 0).then_some(out)
}

/// `xml` with each tag on its own line, two spaces a level (see [`indent_xml`]).
pub(super) fn format_xml(xml: &str) -> Option<String> {
    indent_xml(xml.trim())
}

/// `yaml` written again in block style, two spaces a level. Comments, anchors and tags
/// wouldn't survive that: with any, only the spaces ending its lines are removed.
pub(super) fn format_yaml(yaml: &str) -> Option<String> {
    let kept = yaml_tokens(yaml).into_iter().any(|(range, kind)| kind == "punct" && yaml[range].starts_with(['#', '&', '*', '!']));
    if kept {
        return Some(yaml.lines().map(str::trim_end).collect::<Vec<_>>().join("\n"));
    }
    let value = serde_norway::from_str::<serde_norway::Value>(yaml).ok()?;
    let mut written = serde_norway::to_string(&value).ok()?;
    written.truncate(written.trim_end().len());
    Some(written)
}

/// An XML element as a JSON-like value: its text when it has neither attributes nor child
/// elements; else an object of `@attribute`s, children by tag (a repeated one as an array)
/// and `#text`.
fn xml_element_value(node: roxmltree::Node) -> Value {
    let children: Vec<roxmltree::Node> = node.children().filter(roxmltree::Node::is_element).collect();
    let text: String = node.children().filter(roxmltree::Node::is_text).filter_map(|n| n.text()).collect::<String>().trim().to_string();
    if children.is_empty() && node.attributes().len() == 0 {
        return Value::String(text);
    }
    let mut map = Map::new();
    for attribute in node.attributes() {
        map.insert(format!("@{}", attribute.name()), Value::String(attribute.value().to_string()));
    }
    for child in children {
        let value = xml_element_value(child);
        match map.get_mut(child.tag_name().name()) {
            Some(Value::Array(items)) => items.push(value),
            Some(first) => *first = Value::Array(vec![first.take(), value]),
            None => {
                map.insert(child.tag_name().name().to_string(), value);
            }
        }
    }
    if !text.is_empty() {
        map.insert("#text".to_string(), Value::String(text));
    }
    Value::Object(map)
}

/// The value an XML document stands for: its root element's, or the list of the root's
/// children when all of them are the same repeated element.
fn xml_value(raw: &str) -> Option<Value> {
    let document = roxmltree::Document::parse(raw).ok()?;
    match xml_element_value(document.root_element()) {
        Value::Object(map) if map.len() == 1 && map.values().all(Value::is_array) => map.into_iter().next().map(|(_, items)| items),
        value @ Value::Object(_) => Some(value),
        _ => None,
    }
}

/// The object or array `raw` stands for when it is an XML document, or YAML holding a
/// mapping or a list: what the table reads of a value that isn't JSON.
pub(super) fn structured_value(raw: &str) -> Option<Value> {
    let trimmed = raw.trim_start();
    if trimmed.starts_with('<') {
        return xml_value(raw);
    }
    if !trimmed.starts_with('-') && !raw.contains(": ") && !raw.contains(":\n") {
        return None;
    }
    yaml_document(raw)
}

/// The JSON object or array `text` holds as YAML, when it isn't JSON already.
pub(super) fn yaml_document(text: &str) -> Option<Value> {
    match serde_norway::from_str::<Value>(text) {
        Ok(value @ (Value::Object(_) | Value::Array(_))) => Some(value),
        _ => None,
    }
}

/// Why `text` isn't valid as `kind`, "" when it is (raw text always is).
pub(super) fn check(text: &str, kind: EditKind) -> String {
    match kind {
        EditKind::Json => match serde_json::from_str::<Value>(text) {
            Ok(_) => String::new(),
            Err(e) => t!("Not valid JSON: line %{line}, column %{column}", line = e.line(), column = e.column()).into_owned(),
        },
        EditKind::Yaml => match serde_norway::from_str::<serde_norway::Value>(text) {
            Ok(_) => String::new(),
            Err(e) => match e.location() {
                Some(at) => t!("Not valid YAML: line %{line}, column %{column}", line = at.line(), column = at.column()).into_owned(),
                None => t!("Not valid YAML").into_owned(),
            },
        },
        EditKind::Xml => match roxmltree::Document::parse(text) {
            Ok(_) => String::new(),
            Err(e) => t!("Not valid XML: line %{line}, column %{column}", line = e.pos().row, column = e.pos().col).into_owned(),
        },
        EditKind::Text => String::new(),
    }
}

#[cfg(test)]
mod tests;
