use super::super::test_support::field;
use super::*;

/// A hash of `total` names `f000`..., with the page `start..start + loaded` read, and
/// its name scan ongoing when `more`.
fn hash(total: usize, start: usize, loaded: usize, more: bool) -> FetchedKey {
    let names: Vec<String> = (0..total).map(|i| format!("f{i:03}")).collect();
    FetchedKey {
        badge: "hash".into(),
        meta: String::new(),
        fields: names[start..start + loaded].iter().map(|n| field(n, None)).collect(),
        all_names: Some(names),
        start,
        field_count: total,
        ttl: -1,
        names_cursor: more.then_some(7),
        descending: false,
        editable: true,
    }
}

fn page(fetch: &FetchedKey, dir: PageDir) -> Option<(String, String, usize)> {
    hash_page(fetch, dir).map(|names| (names[0].clone(), names[names.len() - 1].clone(), names.len()))
}

#[test]
fn the_next_and_previous_pages_follow_the_loaded_fields() {
    let middle = hash(1000, 200, HASH_PAGE_SIZE, false);
    assert_eq!(page(&middle, PageDir::Next), Some(("f400".into(), "f599".into(), HASH_PAGE_SIZE)));
    assert_eq!(page(&middle, PageDir::Previous), Some(("f000".into(), "f199".into(), HASH_PAGE_SIZE)));
}

#[test]
fn the_last_page_is_short_and_there_is_none_past_the_ends() {
    let near_end = hash(450, 200, HASH_PAGE_SIZE, false);
    assert_eq!(page(&near_end, PageDir::Next), Some(("f400".into(), "f449".into(), 50)));
    let last = hash(450, 400, 50, false);
    assert_eq!(page(&last, PageDir::Next), None);
    assert!(!has_next(&last));
    assert_eq!(page(&hash(450, 0, HASH_PAGE_SIZE, false), PageDir::Previous), None);
}

#[test]
fn a_page_that_starts_off_the_grid_goes_back_to_the_start() {
    // opened on a field that put the first page at 50
    let shifted = hash(1000, 50, HASH_PAGE_SIZE, false);
    assert_eq!(page(&shifted, PageDir::Previous), Some(("f000".into(), "f049".into(), 50)));
}

#[test]
fn names_still_being_read_mean_there_is_more() {
    let all_read_so_far = hash(200, 0, HASH_PAGE_SIZE, true);
    assert_eq!(page(&all_read_so_far, PageDir::Next), None);
    assert!(has_next(&all_read_so_far));
}

#[test]
fn a_field_is_held_only_by_the_page_that_shows_it() {
    let fetch = hash(1000, 200, HASH_PAGE_SIZE, true);
    assert!(holds_field(&fetch, "f250"));
    assert!(!holds_field(&fetch, "f100"));
    assert!(!holds_field(&fetch, "f400"));
    assert!(!holds_field(&fetch, "unread"), "a name not read yet may be further on");
    assert!(holds_field(&hash(10, 0, 10, false), "unknown"), "every name read: it isn't a field of this hash");
}

#[test]
fn the_header_says_how_many_names_are_read_while_some_are_not() {
    assert_eq!(hash(1000, 0, HASH_PAGE_SIZE, true).hash_meta(), "1000 fields · 1000 loaded, sorted among themselves · no ttl");
    assert_eq!(hash(1000, 0, HASH_PAGE_SIZE, false).hash_meta(), "1000 fields · no ttl");
}
