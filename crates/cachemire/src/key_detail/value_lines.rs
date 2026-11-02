//! An open row's text as the lines it is drawn in: its paragraphs wrapped to the row's
//! width in characters, so the row builds only the lines near the view however long the
//! text is; and the positions (line, byte offset in it) its selection and matches use.

use std::ops::Range;
use std::rc::Rc;
use std::sync::{Arc, Weak};

use rustc_hash::FxHashMap;
use slint::Color;
use slint::private_unstable_api::re_exports::StyledText;

use super::highlight::{Match, find_matches, overlay_highlights};
use super::pretty_text::ColoredParagraph;
use super::word::word_range;

/// One drawn line: bytes `start..end` of paragraph `para`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct VisualLine {
    pub(super) para: usize,
    pub(super) start: usize,
    pub(super) end: usize,
}

/// `text` cut into lines of at most `columns` characters, each one after its last
/// space when it has one (the space stays at the end of the line).
fn wrap(text: &str, columns: usize) -> Vec<Range<usize>> {
    let columns = columns.max(1);
    let mut lines = Vec::new();
    let (mut start, mut count, mut last_space) = (0, 0, None);
    for (i, c) in text.char_indices() {
        if count == columns {
            let end = last_space.filter(|&space| space > start).unwrap_or(i);
            lines.push(start..end);
            count = text[end..i].chars().count();
            last_space = text[end..i].rfind(' ').map(|at| end + at + 1);
            start = end;
        }
        count += 1;
        if c == ' ' {
            last_space = Some(i + 1);
        }
    }
    lines.push(start..text.len());
    lines
}

/// Every paragraph of `paragraphs` wrapped to `columns` characters, in order.
fn layout(paragraphs: &[ColoredParagraph], columns: usize) -> Vec<VisualLine> {
    paragraphs
        .iter()
        .enumerate()
        .flat_map(|(para, (text, _))| wrap(text, columns).into_iter().map(move |r| VisualLine { para, start: r.start, end: r.end }))
        .collect()
}

/// The spans of `spans` (sorted, not overlapping) within `start..end`, cut to it and
/// counted from `start`.
fn clip(spans: &[(Range<usize>, Color)], start: usize, end: usize) -> Vec<(Range<usize>, Color)> {
    let first = spans.partition_point(|(r, _)| r.end <= start);
    spans[first..]
        .iter()
        .take_while(|(r, _)| r.start < end)
        .map(|(r, color)| (r.start.max(start) - start..r.end.min(end) - start, *color))
        .filter(|(r, _)| !r.is_empty())
        .collect()
}

/// A text's paragraphs and the lines they're drawn in.
pub(super) struct Lines<'a> {
    pub(super) paragraphs: &'a [ColoredParagraph],
    pub(super) layout: &'a [VisualLine],
}

impl Lines<'_> {
    fn text(&self, line: usize) -> &str {
        let l = self.layout[line];
        &self.paragraphs[l.para].0[l.start..l.end]
    }

    /// The byte offset in `line` of its `column`-th character, its end past the last one.
    pub(super) fn offset_at(&self, line: usize, column: usize) -> usize {
        if line >= self.layout.len() {
            return 0;
        }
        let text = self.text(line);
        text.char_indices().nth(column).map_or(text.len(), |(at, _)| at)
    }

    /// The line holding byte `byte` of paragraph `para`, and that byte's offset in it.
    pub(super) fn position_of(&self, para: usize, byte: usize) -> (usize, usize) {
        let line = self.layout.partition_point(|l| (l.para, l.start) <= (para, byte)).saturating_sub(1);
        (line, self.layout.get(line).map_or(0, |l| byte.saturating_sub(l.start)))
    }

    /// The end of the last line.
    pub(super) fn end(&self) -> (usize, usize) {
        let last = self.layout.len().saturating_sub(1);
        (last, self.layout.get(last).map_or(0, |l| l.end - l.start))
    }

    /// The text from position `from` to `to`: a paragraph's lines joined as they were, the
    /// paragraphs by `\n`.
    pub(super) fn selected_text(&self, from: (usize, usize), to: (usize, usize)) -> String {
        let mut out = String::new();
        for line in from.0..=to.0.min(self.layout.len().saturating_sub(1)) {
            if line >= self.layout.len() {
                break;
            }
            if line > from.0 && self.layout[line].para != self.layout[line - 1].para {
                out.push('\n');
            }
            let text = self.text(line);
            let start = if line == from.0 { from.1.min(text.len()) } else { 0 };
            let end = if line == to.0 { to.1.min(text.len()) } else { text.len() };
            if start < end && text.is_char_boundary(start) && text.is_char_boundary(end) {
                out.push_str(&text[start..end]);
            }
        }
        out
    }

    /// Where the word at byte `offset` of `line` starts and ends (see [`word_range`]).
    pub(super) fn word_at(&self, line: usize, offset: usize) -> Option<((usize, usize), (usize, usize))> {
        let l = *self.layout.get(line)?;
        let (start, end) = word_range(&self.paragraphs[l.para].0, l.start + offset)?;
        Some((self.position_of(l.para, start), self.position_of(l.para, end)))
    }

    /// Lines `range` (those of it there are) as drawn, with their length in bytes:
    /// `highlights` gives the marks of a range of paragraphs, indexed from its start.
    pub(super) fn styled(
        &self,
        range: Range<usize>,
        highlights: impl FnOnce(Range<usize>) -> Vec<(usize, Range<usize>, Color)>,
    ) -> Vec<(StyledText, usize)> {
        let range = range.start.min(self.layout.len())..range.end.min(self.layout.len());
        if range.is_empty() {
            return Vec::new();
        }
        let paras = self.layout[range.start].para..self.layout[range.end - 1].para + 1;
        let marked = overlay_highlights(&self.paragraphs[paras.clone()], &highlights(paras.clone()));
        self.layout[range]
            .iter()
            .map(|l| {
                let (text, spans, backgrounds) = &marked[l.para - paras.start];
                let line = (text[l.start..l.end].to_string(), clip(spans, l.start, l.end), clip(backgrounds, l.start, l.end));
                (StyledText::from_highlighted_paragraphs(vec![line]), l.end - l.start)
            })
            .collect()
    }
}

/// What a text's lines are built from, kept while its rendering lives: its wrapping
/// for a column count, and the find matches of a query in it.
struct Memo {
    paragraphs: Weak<Vec<ColoredParagraph>>,
    layout: Option<(usize, Rc<Vec<VisualLine>>)>,
    finds: Option<(String, Rc<Vec<Match>>)>,
}

/// The [`Memo`]s of open rows' renderings, by field name and escaped or not.
#[derive(Default)]
pub(super) struct LineMemos(FxHashMap<(String, bool), Memo>);

impl LineMemos {
    /// The memo of `paragraphs`, a new one if it was of another rendering.
    fn memo(&mut self, name: &str, escaped: bool, paragraphs: &Arc<Vec<ColoredParagraph>>) -> &mut Memo {
        self.0.retain(|_, memo| memo.paragraphs.strong_count() > 0);
        let weak = Arc::downgrade(paragraphs);
        let memo = self.0.entry((name.to_string(), escaped)).or_insert_with(|| Memo { paragraphs: Weak::new(), layout: None, finds: None });
        if !memo.paragraphs.ptr_eq(&weak) {
            *memo = Memo { paragraphs: weak, layout: None, finds: None };
        }
        memo
    }

    /// `paragraphs` wrapped to `columns` characters.
    pub(super) fn layout(
        &mut self,
        name: &str,
        escaped: bool,
        paragraphs: &Arc<Vec<ColoredParagraph>>,
        columns: usize,
    ) -> Rc<Vec<VisualLine>> {
        let memo = self.memo(name, escaped, paragraphs);
        match &memo.layout {
            Some((built_for, layout)) if *built_for == columns => layout.clone(),
            _ => {
                let built = Rc::new(layout(paragraphs, columns));
                memo.layout = Some((columns, built.clone()));
                built
            }
        }
    }

    /// The matches of `query` (already lowercased) in `paragraphs`, any case.
    pub(super) fn finds(&mut self, name: &str, escaped: bool, paragraphs: &Arc<Vec<ColoredParagraph>>, query: &str) -> Rc<Vec<Match>> {
        let memo = self.memo(name, escaped, paragraphs);
        match &memo.finds {
            Some((found_for, finds)) if found_for == query => finds.clone(),
            _ => {
                let found = Rc::new(find_matches(paragraphs, query, false));
                memo.finds = Some((query.to_string(), found.clone()));
                found
            }
        }
    }
}
