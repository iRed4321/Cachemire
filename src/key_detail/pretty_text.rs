//! Builds colored, paragraph-split text straight from a `Value` by hooking
//! serde_json's `Formatter` trait, so escaping and number formatting are
//! exactly what serde_json would write, in the single pass it already makes.

use std::cell::RefCell;
use std::io;
use std::ops::Range;
use std::rc::Rc;

use serde::Serialize;
use serde_json::ser::{CharEscape, Formatter};
use slint::Color;

use super::color::color_for_kind;

/// Caps the work spent on a collapsed row's preview. Far more than the single
/// elided line can show, so the visible cutoff is still the `overflow: elide`;
/// this only bounds the worst case.
pub(super) const COMPACT_PREVIEW_CHAR_BUDGET: usize = 350;

/// Color spans (byte ranges into their paragraph's text) in the order
/// `ColorTrackingFormatter` wrote them.
pub(super) type ColorSpans = Vec<(Range<usize>, Color)>;
/// One line's plain text plus its color spans — the shape
/// `StyledText::from_colored_paragraphs` expects.
pub(super) type ColoredParagraph = (String, ColorSpans);

/// Growing colored-text state for one `StyledText`: `paragraphs` holds
/// completed lines, `text`/`spans` the in-progress one. `budget` charges per
/// character; at zero `push_plain` errors out, truncating — fine for a preview.
#[derive(Default)]
struct ColorAccumulator {
    paragraphs: Vec<ColoredParagraph>,
    text: String,
    spans: ColorSpans,
    budget: usize,
}

impl ColorAccumulator {
    /// Appends `s`, truncated to at most `budget` remaining chars, so one
    /// oversized fragment (a giant string value arrives as a single write)
    /// can't blow past the budget.
    fn push_plain(&mut self, s: &str) -> io::Result<()> {
        if self.budget == 0 {
            return Err(io::Error::other("budget exceeded"));
        }
        let char_count = s.chars().count();
        if char_count <= self.budget {
            self.text.push_str(s);
            self.budget -= char_count;
            Ok(())
        } else {
            self.text.extend(s.chars().take(self.budget));
            self.budget = 0;
            Err(io::Error::other("budget exceeded"))
        }
    }

    fn push_colored(&mut self, s: &str, kind: &str) -> io::Result<()> {
        let start = self.text.len();
        let result = self.push_plain(s);
        if let Some(color) = color_for_kind(kind)
            && self.text.len() > start
        {
            self.spans.push((start..self.text.len(), color));
        }
        result
    }

    fn finish_paragraph(&mut self) {
        self.paragraphs.push((std::mem::take(&mut self.text), std::mem::take(&mut self.spans)));
    }
}

struct ColorTrackingFormatter {
    pretty: bool,
    current_indent: usize,
    has_value: bool,
    in_object_key: bool,
    string_start: usize,
    acc: Rc<RefCell<ColorAccumulator>>,
}

impl ColorTrackingFormatter {
    fn new(pretty: bool, acc: Rc<RefCell<ColorAccumulator>>) -> Self {
        Self { pretty, current_indent: 0, has_value: false, in_object_key: false, string_start: 0, acc }
    }

    /// Finishes the current line and starts a new one, indented to the
    /// current depth — pretty mode only; compact mode has one paragraph.
    fn newline_and_indent(&mut self) -> io::Result<()> {
        let mut acc = self.acc.borrow_mut();
        acc.finish_paragraph();
        let prefix = "  ".repeat(self.current_indent);
        if prefix.is_empty() { Ok(()) } else { acc.push_colored(&prefix, "punct") }
    }

    /// Shared by array elements and object keys: a leading comma for every
    /// item but the first, then (pretty mode only) a new indented line.
    fn begin_item(&mut self, first: bool) -> io::Result<()> {
        if !first {
            self.acc.borrow_mut().push_colored(",", "punct")?;
        }
        if self.pretty { self.newline_and_indent() } else { Ok(()) }
    }
}

impl Formatter for ColorTrackingFormatter {
    fn write_null<W: ?Sized + io::Write>(&mut self, _w: &mut W) -> io::Result<()> {
        self.acc.borrow_mut().push_colored("null", "null")
    }

    fn write_bool<W: ?Sized + io::Write>(&mut self, _w: &mut W, value: bool) -> io::Result<()> {
        self.acc.borrow_mut().push_colored(if value { "true" } else { "false" }, "bool")
    }

    fn write_i64<W: ?Sized + io::Write>(&mut self, _w: &mut W, value: i64) -> io::Result<()> {
        self.acc.borrow_mut().push_colored(&value.to_string(), "number")
    }

    fn write_u64<W: ?Sized + io::Write>(&mut self, _w: &mut W, value: u64) -> io::Result<()> {
        self.acc.borrow_mut().push_colored(&value.to_string(), "number")
    }

    fn write_f64<W: ?Sized + io::Write>(&mut self, _w: &mut W, value: f64) -> io::Result<()> {
        self.acc.borrow_mut().push_colored(&value.to_string(), "number")
    }

    // Strings arrive as `begin_string`, fragment/escape calls, `end_string`.
    // Fragments write plain (no per-fragment span) so the whole string —
    // quotes included — is one key/string-colored span, matching one token.
    fn begin_string<W: ?Sized + io::Write>(&mut self, _w: &mut W) -> io::Result<()> {
        let mut acc = self.acc.borrow_mut();
        self.string_start = acc.text.len();
        acc.push_plain("\"")
    }

    fn end_string<W: ?Sized + io::Write>(&mut self, _w: &mut W) -> io::Result<()> {
        let mut acc = self.acc.borrow_mut();
        acc.push_plain("\"")?;
        let end = acc.text.len();
        let kind = if self.in_object_key { "key" } else { "string" };
        if let Some(color) = color_for_kind(kind) {
            acc.spans.push((self.string_start..end, color));
        }
        Ok(())
    }

    fn write_string_fragment<W: ?Sized + io::Write>(&mut self, _w: &mut W, fragment: &str) -> io::Result<()> {
        self.acc.borrow_mut().push_plain(fragment)
    }

    fn write_char_escape<W: ?Sized + io::Write>(&mut self, _w: &mut W, char_escape: CharEscape) -> io::Result<()> {
        use CharEscape::*;
        let escaped = match char_escape {
            AsciiControl(byte) => {
                const HEX: [u8; 16] = *b"0123456789abcdef";
                let bytes = [b'\\', b'u', b'0', b'0', HEX[(byte >> 4) as usize], HEX[(byte & 0xf) as usize]];
                return self.acc.borrow_mut().push_plain(std::str::from_utf8(&bytes).unwrap_or_default());
            }
            Quote => "\\\"",
            ReverseSolidus => "\\\\",
            Solidus => "\\/",
            Backspace => "\\b",
            FormFeed => "\\f",
            LineFeed => "\\n",
            CarriageReturn => "\\r",
            Tab => "\\t",
        };
        self.acc.borrow_mut().push_plain(escaped)
    }

    fn begin_array<W: ?Sized + io::Write>(&mut self, _w: &mut W) -> io::Result<()> {
        self.current_indent += 1;
        self.has_value = false;
        self.acc.borrow_mut().push_colored("[", "punct")
    }

    fn end_array<W: ?Sized + io::Write>(&mut self, _w: &mut W) -> io::Result<()> {
        self.current_indent -= 1;
        if self.pretty && self.has_value {
            self.newline_and_indent()?;
        }
        self.acc.borrow_mut().push_colored("]", "punct")
    }

    fn begin_array_value<W: ?Sized + io::Write>(&mut self, _w: &mut W, first: bool) -> io::Result<()> {
        self.begin_item(first)
    }

    fn end_array_value<W: ?Sized + io::Write>(&mut self, _w: &mut W) -> io::Result<()> {
        self.has_value = true;
        Ok(())
    }

    fn begin_object<W: ?Sized + io::Write>(&mut self, _w: &mut W) -> io::Result<()> {
        self.current_indent += 1;
        self.has_value = false;
        self.acc.borrow_mut().push_colored("{", "punct")
    }

    fn end_object<W: ?Sized + io::Write>(&mut self, _w: &mut W) -> io::Result<()> {
        self.current_indent -= 1;
        if self.pretty && self.has_value {
            self.newline_and_indent()?;
        }
        self.acc.borrow_mut().push_colored("}", "punct")
    }

    fn begin_object_key<W: ?Sized + io::Write>(&mut self, _w: &mut W, first: bool) -> io::Result<()> {
        self.begin_item(first)?;
        self.in_object_key = true;
        Ok(())
    }

    fn end_object_key<W: ?Sized + io::Write>(&mut self, _w: &mut W) -> io::Result<()> {
        self.in_object_key = false;
        Ok(())
    }

    fn begin_object_value<W: ?Sized + io::Write>(&mut self, _w: &mut W) -> io::Result<()> {
        self.acc.borrow_mut().push_colored(if self.pretty { ": " } else { ":" }, "punct")
    }

    fn end_object_value<W: ?Sized + io::Write>(&mut self, _w: &mut W) -> io::Result<()> {
        self.has_value = true;
        Ok(())
    }
}

pub(super) fn colored_paragraphs_for(value: &impl Serialize, pretty: bool, budget: usize) -> Vec<ColoredParagraph> {
    let acc = Rc::new(RefCell::new(ColorAccumulator { budget, ..Default::default() }));
    {
        let formatter = ColorTrackingFormatter::new(pretty, acc.clone());
        let mut serializer = serde_json::Serializer::with_formatter(io::sink(), formatter);
        // ignore the result: a budget cutoff aborts serialization on purpose, and
        // writing to io::sink() can't fail for any other reason
        let _ = value.serialize(&mut serializer);
    }
    let mut acc = acc.borrow_mut();
    acc.finish_paragraph();
    std::mem::take(&mut acc.paragraphs)
}

/// Adds `text` to `out`, a `\` before each `"` and `\` in the color of kind "escape", the rest
/// (and the escaped characters themselves) in `color`; `spans` gets one span per colored run.
fn push_escaped(out: &mut String, spans: &mut ColorSpans, text: &str, color: Option<Color>) {
    let slash = color_for_kind("escape");
    let close = |out: &String, spans: &mut ColorSpans, from: usize| {
        if let Some(color) = color
            && out.len() > from
        {
            spans.push((from..out.len(), color));
        }
    };
    let mut run = out.len();
    for c in text.chars() {
        if c == '"' || c == '\\' {
            close(out, spans, run);
            let start = out.len();
            out.push('\\');
            if let Some(slash) = slash {
                spans.push((start..out.len(), slash));
            }
            run = out.len();
        }
        out.push(c);
    }
    close(out, spans, run);
}

/// The text of `paragraphs` as the JSON string holding it: quoted, each `"` and `\` backslashed,
/// every token keeping its color and the added backslashes getting their own.
pub(super) fn escaped_paragraphs(paragraphs: Vec<ColoredParagraph>) -> Vec<ColoredParagraph> {
    let mut out = String::new();
    let mut spans = ColorSpans::new();
    let punct = color_for_kind("punct");
    let quote = |out: &mut String, spans: &mut ColorSpans| {
        let start = out.len();
        out.push('"');
        if let Some(color) = punct {
            spans.push((start..out.len(), color));
        }
    };
    quote(&mut out, &mut spans);
    for (text, colors) in &paragraphs {
        let mut at = 0;
        for (range, color) in colors {
            let start = range.start.max(at);
            if range.end <= start {
                continue;
            }
            push_escaped(&mut out, &mut spans, &text[at..start], None);
            push_escaped(&mut out, &mut spans, &text[start..range.end], Some(*color));
            at = range.end;
        }
        push_escaped(&mut out, &mut spans, &text[at..], None);
    }
    quote(&mut out, &mut spans);
    vec![(out, spans)]
}

/// A value's paragraphs as a row shows them open: indented, or (`escaped`) as the one
/// line of its compact JSON escaped into a string.
pub(super) fn paragraphs_for(value: &impl Serialize, escaped: bool) -> Vec<ColoredParagraph> {
    if escaped { escaped_paragraphs(colored_paragraphs_for(value, false, usize::MAX)) } else { pretty_paragraphs_for(value) }
}

/// Pretty (indented) paragraphs for a value — the unit the pretty cache
/// stores, from which the actual `StyledText` (with whatever highlights
/// apply at the time) is derived on demand.
pub(super) fn pretty_paragraphs_for(value: &impl Serialize) -> Vec<ColoredParagraph> {
    colored_paragraphs_for(value, true, usize::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn text_of(paragraphs: &[ColoredParagraph]) -> String {
        paragraphs.iter().map(|(t, _)| t.as_str()).collect::<Vec<_>>().join("\n")
    }

    #[test]
    fn pretty_paragraphs_indent_one_line_per_item() {
        let value = json!({"a": {"b": [1, 2]}, "e": ""});
        let text = text_of(&pretty_paragraphs_for(&value));
        assert!(text.contains("  \"a\": {\n    \"b\": [\n      1,\n      2\n    ]\n  },\n  \"e\": \"\""), "{text}");
    }
}
