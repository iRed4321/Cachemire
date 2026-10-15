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
