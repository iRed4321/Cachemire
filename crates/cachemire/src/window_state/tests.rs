use super::*;

fn rect(x: i32, y: i32, width: u32, height: u32) -> Rect {
    Rect { x, y, width, height }
}

const LAPTOP: Rect = Rect { x: 0, y: 0, width: 1920, height: 1080 };
const LEFT_MONITOR: Rect = Rect { x: -2560, y: 0, width: 2560, height: 1440 };

#[test]
fn a_window_comes_back_where_it_was() {
    let state = WindowState { normal: Some(rect(100, 80, 1200, 800)), maximized: false, maximized_rect: None };
    assert_eq!(state.start_rect(), Some(rect(100, 80, 1200, 800)));
    assert_eq!(WindowState::default().start_rect(), None, "a first start leaves the place to the system");
}

#[test]
fn a_saved_size_too_small_to_use_is_raised() {
    let state = WindowState { normal: Some(rect(10, 10, 50, 0)), maximized: false, maximized_rect: None };
    assert_eq!(state.start_rect(), Some(rect(10, 10, MIN_SIZE.0, MIN_SIZE.1)));
}

#[test]
fn a_window_maximized_on_another_screen_starts_on_that_screen() {
    // dragged to the left monitor and maximized there, its normal rect still on the laptop
    let state = WindowState { normal: Some(rect(100, 100, 1200, 800)), maximized: true, maximized_rect: Some(LEFT_MONITOR) };
    let start = state.start_rect().unwrap();
    assert!(LEFT_MONITOR.contains(start.center().0, start.center().1), "{start:?}");
    assert_eq!((start.width, start.height), (1200, 800));

    let same_screen = WindowState { normal: Some(rect(-2000, 100, 1200, 800)), maximized: true, maximized_rect: Some(LEFT_MONITOR) };
    assert_eq!(same_screen.start_rect(), Some(rect(-2000, 100, 1200, 800)), "already there: kept as it was");
}

#[test]
fn a_window_only_ever_maximized_gets_a_normal_size_on_its_screen() {
    let state = WindowState { normal: None, maximized: true, maximized_rect: Some(LAPTOP) };
    assert_eq!(state.start_rect(), Some(rect(0, 0, 1536, 864)));
}

#[test]
fn a_window_left_on_an_unplugged_screen_is_brought_to_the_primary_one() {
    let on_gone_screen = rect(-2000, 100, 1200, 800);
    assert_eq!(place_on_screens(on_gone_screen, &[LAPTOP], Some(LAPTOP)), rect(360, 140, 1200, 800));
    let too_big = rect(-2560, 0, 2560, 1440);
    assert_eq!(place_on_screens(too_big, &[LAPTOP], Some(LAPTOP)), LAPTOP, "shrunk to fit the screen");
}

#[test]
fn a_window_mostly_off_screen_but_still_reachable_is_left_alone() {
    // its left 150 px still show on the laptop's right edge
    let hanging = rect(1770, 500, 1200, 800);
    assert_eq!(place_on_screens(hanging, &[LAPTOP], Some(LAPTOP)), hanging);
    let sliver = rect(1870, 500, 1200, 800);
    assert_ne!(place_on_screens(sliver, &[LAPTOP], Some(LAPTOP)), sliver, "50 px can't be grabbed");
    assert_eq!(place_on_screens(rect(-2000, 100, 1200, 800), &[LAPTOP, LEFT_MONITOR], Some(LAPTOP)), rect(-2000, 100, 1200, 800));
}

#[test]
fn with_no_screen_known_nothing_is_moved() {
    let anywhere = rect(-9000, -9000, 800, 600);
    assert_eq!(place_on_screens(anywhere, &[], None), anywhere);
}

#[test]
fn the_saved_state_reads_back_as_written() {
    let file = crate::test_support::temp_dir("window").join("window.json");
    let state = WindowState { normal: Some(rect(-5, 7, 900, 700)), maximized: true, maximized_rect: Some(LEFT_MONITOR) };
    persist::save(&file, &state).unwrap();
    assert!(matches!(persist::load::<WindowState>(&file), Loaded::Read(read) if read == state));
}
