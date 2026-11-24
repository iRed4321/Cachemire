//! The Key bindings page of the Settings: the app's shortcuts listed by place, picking other
//! keys for one (or being told why they can't be used), going back to the defaults.

use super::*;
use crate::{Preferences, ShortcutAction, ShortcutRow, Shortcuts};

/// The window with its shortcuts wired over `state`, which the test can read.
fn bindings(state: &std::sync::Arc<cachemire_core::state::AppState>) -> Harness {
    let h = Harness::new();
    crate::shortcuts::init(&h.window, state.clone());
    h
}

fn rows(h: &Harness) -> Vec<ShortcutRow> {
    h.window.global::<Shortcuts>().get_rows().iter().collect()
}

fn row(h: &Harness, action: ShortcutAction) -> ShortcutRow {
    rows(h).into_iter().find(|row| row.action == action).expect("the action is listed")
}

fn keys_shown(row: &ShortcutRow) -> Vec<String> {
    row.keys.iter().map(|keys| keys.to_string()).collect()
}

/// Presses `text` with Ctrl (and Shift when `shift`) while the page records keys for `action`.
fn press(h: &Harness, action: ShortcutAction, text: &str, control: bool, shift: bool) {
    let shortcuts = h.window.global::<Shortcuts>();
    shortcuts.set_recording_action(action);
    shortcuts.set_recording(true);
    shortcuts.invoke_recorded(action, text.into(), control, shift, false, false);
}

fn error(h: &Harness) -> String {
    h.window.global::<Preferences>().get_error().to_string()
}

#[test]
fn the_page_lists_every_shortcut_under_the_place_it_works_in() {
    let h = bindings(&test_state());
    let rows = rows(&h);
    assert_eq!(rows.len(), 12, "every action of the app");
    let titles: Vec<String> = rows.iter().filter(|row| !row.group_title.is_empty()).map(|row| row.group_title.to_string()).collect();
    assert_eq!(titles, ["Window", "Query box", "Value editor"], "each place named once, on its first row");
    assert_eq!(keys_shown(&row(&h, ShortcutAction::CloseTab)), ["Ctrl+W"]);
    assert_eq!(keys_shown(&row(&h, ShortcutAction::SaveValue)), ["Ctrl+Enter", "Ctrl+S"], "an action can have two");
    assert!(rows.iter().all(|row| !row.custom), "nothing picked yet");

    let standard: Vec<String> = h.window.global::<Shortcuts>().get_standard().iter().map(|row| row.title.to_string()).collect();
    assert!(standard.contains(&"Copy".to_string()) && standard.contains(&"Undo".to_string()), "{standard:?}");
}

#[test]
fn keys_pressed_for_a_shortcut_replace_its_default_and_are_kept() {
    let state = test_state();
    let h = bindings(&state);
    press(&h, ShortcutAction::CloseTab, "q", true, false);
    assert_eq!(error(&h), "");
    assert!(!h.window.global::<Shortcuts>().get_recording(), "the page stops listening once keys are taken");
    let close = row(&h, ShortcutAction::CloseTab);
    assert_eq!(keys_shown(&close), ["Ctrl+Q"]);
    assert!(close.custom, "marked as changed, with its reset button");
    assert!(state.settings.lock().shortcut("closeTab").is_some(), "saved in the settings");

    let shortcuts = h.window.global::<Shortcuts>();
    assert!(shortcuts.invoke_matches(shortcuts.get_close_tab(), "q".into(), true, false, false, false), "Ctrl+Q now closes the tab");
    assert!(!shortcuts.invoke_matches(shortcuts.get_close_tab(), "w".into(), true, false, false, false), "Ctrl+W no longer does");
}

#[test]
fn the_app_starts_on_the_keys_picked_before_and_ignores_unreadable_ones() {
    let state = test_state();
    {
        let mut settings = state.settings.lock();
        settings.set_shortcut("closeTab", Some(vec!["Control".into(), "q".into()])).unwrap();
        settings.set_shortcut("search", Some(vec!["NoSuchKeyName".into(), "Control".into()])).unwrap();
    }
    let h = bindings(&state);
    let close = row(&h, ShortcutAction::CloseTab);
    assert_eq!((keys_shown(&close), close.custom), (vec!["Ctrl+Q".to_string()], true));
    let search = row(&h, ShortcutAction::Search);
    assert_eq!((keys_shown(&search), search.custom), (vec!["Ctrl+F".to_string()], false), "back on its default");
}

#[test]
fn reset_brings_a_shortcut_back_to_its_default_keys() {
    let state = test_state();
    let h = bindings(&state);
    press(&h, ShortcutAction::Search, "k", true, true);
    assert_eq!(keys_shown(&row(&h, ShortcutAction::Search)), ["Ctrl+Shift+K"]);

    h.window.global::<Shortcuts>().invoke_reset(ShortcutAction::Search);
    let search = row(&h, ShortcutAction::Search);
    assert_eq!(keys_shown(&search), ["Ctrl+F"]);
    assert!(!search.custom);
    assert!(state.settings.lock().shortcut("search").is_none(), "nothing left in the settings");
}

#[test]
fn keys_that_cant_be_a_shortcut_are_refused_with_the_reason() {
    let state = test_state();
    let h = bindings(&state);
    press(&h, ShortcutAction::CloseTab, "q", false, false);
    assert!(error(&h).contains("needs Ctrl"), "{}", error(&h));
    press(&h, ShortcutAction::CloseTab, "c", true, false);
    assert!(error(&h).contains("usual use"), "Ctrl+C stays copy: {}", error(&h));
    press(&h, ShortcutAction::CloseTab, "f", true, false);
    assert!(error(&h).contains("already uses"), "Ctrl+F searches: {}", error(&h));
    assert_eq!(keys_shown(&row(&h, ShortcutAction::CloseTab)), ["Ctrl+W"], "the shortcut kept its keys");
    assert!(state.settings.lock().shortcut("closeTab").is_none());

    // a function key needs no Ctrl, and the next keys taken clear the reason shown
    press(&h, ShortcutAction::CloseTab, "\u{F708}", false, false);
    assert_eq!(error(&h), "");
    assert_eq!(keys_shown(&row(&h, ShortcutAction::CloseTab)), ["F5"]);
}

#[test]
fn the_query_box_and_the_value_editor_can_use_the_same_keys() {
    let h = bindings(&test_state());
    // Ctrl+S saves in the value editor; the query box is another place
    press(&h, ShortcutAction::RunScript, "s", true, false);
    assert_eq!(error(&h), "");
    assert_eq!(keys_shown(&row(&h, ShortcutAction::RunScript)), ["Ctrl+S"]);
    // but a window-wide shortcut would be hidden by either box
    press(&h, ShortcutAction::ZoomReset, "s", true, false);
    assert!(error(&h).contains("already uses"), "{}", error(&h));
}

#[test]
fn shift_alone_while_recording_waits_for_the_key_that_goes_with_it() {
    let state = test_state();
    let h = bindings(&state);
    press(&h, ShortcutAction::Suggest, "\u{10}", true, true);
    assert!(h.window.global::<Shortcuts>().get_recording(), "still listening");
    assert_eq!(error(&h), "");
    assert!(state.settings.lock().shortcut("suggest").is_none());
}

#[test]
fn a_digit_picked_with_shift_works_with_or_without_it() {
    // on a French keyboard the digits are typed with Shift
    let h = bindings(&test_state());
    press(&h, ShortcutAction::ZoomOut, "9", true, true);
    assert_eq!(error(&h), "");
    let shortcuts = h.window.global::<Shortcuts>();
    let keys = shortcuts.get_zoom_out();
    assert!(shortcuts.invoke_matches(keys.clone(), "9".into(), true, true, false, false));
    assert!(shortcuts.invoke_matches(keys.clone(), "9".into(), true, false, false, false));
    assert!(!shortcuts.invoke_matches(keys, "9".into(), false, false, false, false), "Ctrl is still needed");
}

#[test]
fn the_settings_search_keeps_the_shortcuts_it_finds() {
    let h = bindings(&test_state());
    let shortcuts = h.window.global::<Shortcuts>();
    shortcuts.invoke_filter_changed("zoom".into(), "Key bindings".into(), "Standard keys".into());
    let found: Vec<ShortcutAction> = rows(&h).iter().map(|row| row.action).collect();
    assert_eq!(found, [ShortcutAction::ZoomIn, ShortcutAction::ZoomOut, ShortcutAction::ZoomReset]);
    assert_eq!(rows(&h)[0].group_title, "Window", "the first row found still names its place");
    let standard: Vec<String> = shortcuts.get_standard().iter().map(|row| row.title.to_string()).collect();
    assert_eq!(standard, ["Zoom"], "the mouse wheel's zoom");

    shortcuts.invoke_filter_changed("ctrl+w".into(), "Key bindings".into(), "Standard keys".into());
    let found: Vec<ShortcutAction> = rows(&h).iter().map(|row| row.action).collect();
    assert_eq!(found, [ShortcutAction::CloseTab], "found by its keys too");

    shortcuts.invoke_filter_changed("key bindings".into(), "Key bindings".into(), "Standard keys".into());
    assert_eq!(rows(&h).len(), 12, "the page's name finds all of it");
}
