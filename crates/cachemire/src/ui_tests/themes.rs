//! The themes and syntax themes of the Display settings: what picking one does to the
//! window's colors, to the text Rust colors and to the settings file.

use super::formats::show_seeded;
use super::links::{links_ready, loaded, show_connection};
use super::*;
use crate::theme::{COMBOS, SYNTAXES, paint};
use crate::{Preferences, Theme};
use serde_json::json;
use slint::Color;

fn rgb(hex: u32) -> Color {
    Color::from_rgb_u8((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

#[test]
fn the_settings_list_every_theme_with_its_colors() {
    let state = test_state();
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let h = Harness::new();
    crate::settings::init(&h.window, state.clone(), runtime.handle().clone());
    let preferences = h.window.global::<Preferences>();

    let themes: Vec<_> = preferences.get_themes().iter().collect();
    assert_eq!(themes.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(), COMBOS.iter().map(|c| c.id).collect::<Vec<_>>());
    assert_eq!(themes[1].name, "Nord");
    assert_eq!(themes[1].colors.iter().collect::<Vec<_>>(), [0x2e3440, 0xe5e9f0, 0x88c0d0, 0xebcb8b].map(rgb), "its four colors");
    let syntaxes: Vec<_> = preferences.get_syntax_themes().iter().collect();
    assert_eq!(syntaxes.len(), SYNTAXES.len());
    assert!(syntaxes.iter().all(|s| s.colors.row_count() == 4), "four sample colors each");

    // nothing saved yet: the default ones are ticked and in use
    assert_eq!((preferences.get_theme().as_str(), preferences.get_syntax_theme().as_str()), ("cachemire-dark", "cachemire"));
    assert_eq!(h.window.global::<Theme>().get_bg_app_base(), rgb(0x151517));
}

#[test]
fn the_window_starts_on_the_themes_saved() {
    let state = test_state();
    state.settings.lock().set_theme("github-light").unwrap();
    state.settings.lock().set_syntax_theme("github-light").unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let h = Harness::new();
    crate::settings::init(&h.window, state.clone(), runtime.handle().clone());
    let theme = h.window.global::<Theme>();
    assert_eq!((theme.get_bg_app_base(), theme.get_text_primary(), theme.get_accent_blue()), (rgb(0xffffff), rgb(0x1f2328), rgb(0x0969da)));
    assert_eq!(h.window.global::<Preferences>().get_theme(), "github-light");
    assert_eq!(paint().key, rgb(0x116329));
}

#[test]
fn a_saved_theme_this_version_does_not_know_gives_the_default_one() {
    let state = test_state();
    state.settings.lock().set_theme("from-the-future").unwrap();
    state.settings.lock().set_syntax_theme("neither").unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let h = Harness::new();
    crate::settings::init(&h.window, state.clone(), runtime.handle().clone());
    assert_eq!(h.window.global::<Theme>().get_bg_app_base(), rgb(0x151517));
    let preferences = h.window.global::<Preferences>();
    assert_eq!((preferences.get_theme().as_str(), preferences.get_syntax_theme().as_str()), ("cachemire-dark", "cachemire"));
    assert_eq!(state.settings.lock().theme(), "from-the-future", "what was saved is left as it is");
}

#[test]
fn picking_a_theme_recolors_the_window_and_saves_it() {
    let (mut h, app, rt, _turn, state) = links_ready(&[("a", "redis://cache.test:6379")]);
    crate::settings::init(&h.window, state.clone(), rt.handle().clone());
    show_connection(&h, "a");
    app.key_detail.seed_hash("a", "user:1", &[("bio", json!({"age": 1}))]);
    show_seeded(&mut h, &app, &rt, "user:1");
    let theme = h.window.global::<Theme>();
    let (tick, base_before, panel_before) = (theme.get_paint_tick(), theme.get_bg_app_base(), theme.get_bg_panel());
    // emptied, to see the key's rows made again
    h.window.global::<KeyPanel>().set_all_values_rows(Default::default());

    h.window.global::<Preferences>().invoke_theme_selected("dracula".into());
    assert_eq!(state.settings.lock().theme(), "dracula", "saved");
    assert_eq!(h.window.global::<Preferences>().get_theme(), "dracula", "ticked in the list");
    assert_eq!(h.window.global::<Preferences>().get_error(), "");
    let theme = h.window.global::<Theme>();
    assert_eq!(
        (theme.get_bg_app_base(), theme.get_text_primary(), theme.get_accent_blue(), theme.get_accent_yellow()),
        (rgb(0x282a36), rgb(0xf8f8f2), rgb(0xbd93f9), rgb(0xf1fa8c))
    );
    assert_ne!((theme.get_bg_app_base(), theme.get_bg_panel()), (base_before, panel_before));
    assert_eq!(theme.get_bg_app(), rgb(0x282a36), "the app's background follows");
    assert_eq!(theme.get_paint_tick(), tick + 1, "what Rust colored is asked for again");
    assert_eq!((paint().keyword, paint().mark_filter), (rgb(0xbd93f9), rgb(0xf1fa8c)));

    // the key shown is made again from what was fetched, to take the new colors
    assert!(loaded(&mut h, &rt));
    assert_eq!(h.window.global::<KeyPanel>().get_all_values_rows().row_count(), 1);

    // the profile colors are no theme's: the same on every one
    let theme = h.window.global::<Theme>();
    assert_eq!(theme.invoke_profile_color(crate::ProfileTint::Blue), rgb(0x5b9dff));
    assert_eq!(theme.invoke_profile_color(crate::ProfileTint::Red), rgb(0xff5f56));
}

#[test]
fn picking_a_syntax_theme_recolors_the_values_shown() {
    let (mut h, app, rt, _turn, state) = links_ready(&[("a", "redis://cache.test:6379")]);
    crate::settings::init(&h.window, state.clone(), rt.handle().clone());
    show_connection(&h, "a");
    app.key_detail.seed_hash("a", "user:1", &[("bio", json!({"age": 1, "name": "Ada"}))]);
    show_seeded(&mut h, &app, &rt, "user:1");
    let query = |h: &Harness| h.window.global::<QueryTab>().invoke_highlight("FROM KEY 'k'".into(), 0, 0, "".into(), 0);
    crate::redisql::init(&app, &h.window);
    let query_before = query(&h);
    let base = h.window.global::<Theme>().get_bg_app_base();
    // emptied, to see the key's rows made again (off this thread, where a test's paint isn't)
    h.window.global::<KeyPanel>().set_all_values_rows(Default::default());

    h.window.global::<Preferences>().invoke_syntax_theme_selected("monokai".into());
    assert_eq!(state.settings.lock().syntax_theme(), "monokai");
    assert_eq!(h.window.global::<Preferences>().get_syntax_theme(), "monokai");
    assert_eq!((paint().key, paint().string), (rgb(0xf92672), rgb(0xe6db74)));
    assert_eq!(h.window.global::<Theme>().get_bg_app_base(), base, "the theme stays");
    assert!(loaded(&mut h, &rt));
    assert_eq!(h.window.global::<KeyPanel>().get_all_values_rows().row_count(), 1, "the rows are made again");
    assert!(query(&h) != query_before, "and a query's text is colored again");
}
