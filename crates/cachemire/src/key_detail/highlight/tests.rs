use super::*;

fn c(hex: &str) -> Color {
    let channel = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).unwrap();
    Color::from_rgb_u8(channel(1), channel(3), channel(5))
}

#[test]
fn find_matches_is_case_insensitive_and_non_overlapping() {
    let paragraphs = vec![("Hello hello".to_string(), vec![]), ("aaa".to_string(), vec![])];
    assert_eq!(find_matches(&paragraphs, "hello", false), vec![(0, 0..5), (0, 6..11)]);
    assert_eq!(find_matches(&paragraphs, "aa", false), vec![(1, 0..2)]);
    assert!(find_matches(&paragraphs, "", false).is_empty());
}

#[test]
fn overlay_splits_base_spans_around_highlights() {
    let paragraphs = vec![("abcdefgh".to_string(), vec![(0..4, c("#111111")), (4..8, c("#222222"))])];
    let out = overlay_highlights(&paragraphs, &[(0, 2..6, c("#333333"))]);
    let spans: Vec<Range<usize>> = out[0].1.iter().map(|(r, _)| r.clone()).collect();
    assert_eq!(spans, vec![0..2, 2..6, 6..8]);
    // the marked text is drawn in the dark color, on the highlight's background
    assert_eq!(out[0].1[1].1, HIGHLIGHT_TEXT);
    assert_eq!(out[0].2, vec![(2..6, c("#333333"))]);
}

#[test]
fn overlay_walks_highlights_paragraph_by_paragraph() {
    let paragraphs = vec![
        ("aaaa".to_string(), vec![(0..4, c("#111111"))]),
        ("bbbb".to_string(), vec![(0..4, c("#111111"))]),
        ("cccc".to_string(), vec![(0..4, c("#111111"))]),
    ];
    let out = overlay_highlights(&paragraphs, &[(0, 0..1, c("#222222")), (2, 1..3, c("#333333")), (2, 3..4, c("#444444"))]);
    let spans = |i: usize| out[i].1.iter().map(|(r, _)| r.clone()).collect::<Vec<_>>();
    assert_eq!(spans(0), vec![0..1, 1..4]);
    assert_eq!(spans(1), vec![0..4]);
    assert_eq!(spans(2), vec![0..1, 1..3, 3..4]);
}

fn line(text: &str) -> ColoredParagraph {
    (text.to_string(), Vec::new())
}

/// The text each match of `needle` (any case) covers.
fn found<'a>(paragraphs: &'a [ColoredParagraph], needle: &str) -> Vec<&'a str> {
    find_matches(paragraphs, needle, false).into_iter().map(|(p, r)| &paragraphs[p].0[r]).collect()
}

#[test]
fn any_case_still_matches_on_a_line_whose_lowercase_is_longer_or_shorter() {
    // `İ` lowercases to two characters, `ẞ` to a shorter one
    let lines = [line("İstanbul HELLO"), line("STRAẞE Hello"), line("ẞİ hello İ")];
    assert_eq!(found(&lines, "hello"), ["HELLO", "Hello", "hello"]);
    assert_eq!(found(&lines, "straße"), ["STRAẞE"]);
}

#[test]
fn a_match_never_covers_part_of_a_character() {
    // `İ` lowercases to `i` and a combining dot: `i` alone is half of it
    let lines = [line("İ and i")];
    assert_eq!(found(&lines, "i"), ["i"]);
    assert_eq!(found(&lines, "i̇"), ["İ"]);
}

#[test]
fn case_sensitive_finds_only_the_exact_text() {
    let lines = [line("Hello hello HELLO")];
    let exact: Vec<&str> = find_matches(&lines, "hello", true).into_iter().map(|(p, r)| &lines[p].0[r]).collect();
    assert_eq!(exact, ["hello"]);
    assert!(find_matches(&lines, "", false).is_empty());
}

#[test]
fn a_hit_far_into_its_text_is_shown_in_a_window_around_it() {
    let long = line(&format!("{}needle at the end", "x".repeat(300)));
    let (shown, _) = hit_window(&long, "needle");
    assert!(shown.starts_with('…'), "{shown}");
    assert!(shown.ends_with("needle at the end"));
    assert_eq!(hit_window(&line("short needle"), "needle").0, "short needle", "near the start: no window");
    assert_eq!(hit_window(&line("no match here"), "needle").0, "no match here");
}

#[test]
fn a_window_keeps_the_colors_of_what_it_shows() {
    let paragraph = (format!("{}tail", "a".repeat(100)), vec![(0..100, c("#ff0000")), (100..104, c("#00ff00"))]);
    let (shown, spans) = hit_window(&paragraph, "tail");
    let lead = "…".len();
    assert_eq!(&shown[spans[1].0.clone()], "tail");
    assert_eq!(spans[0].0.start, lead, "the colors start after the …");
}
