use rustc_hash::FxHashSet;

use super::*;

fn channels(color: Color) -> [i32; 3] {
    [i32::from(color.red()), i32::from(color.green()), i32::from(color.blue())]
}

/// How far apart two colors are: the widest gap between their channels.
fn gap(a: Color, b: Color) -> i32 {
    channels(a).into_iter().zip(channels(b)).map(|(x, y)| (x - y).abs()).max().unwrap_or(0)
}

#[test]
fn every_theme_and_syntax_theme_has_its_own_name() {
    let ids: FxHashSet<&str> = COMBOS.iter().map(|c| c.id).collect();
    assert_eq!(ids.len(), COMBOS.len());
    let names: FxHashSet<&str> = COMBOS.iter().map(|c| c.name).collect();
    assert_eq!(names.len(), COMBOS.len());
    let ids: FxHashSet<&str> = SYNTAXES.iter().map(|s| s.id).collect();
    assert_eq!(ids.len(), SYNTAXES.len());
    assert!(COMBOS.iter().map(|c| c.id).chain(SYNTAXES.iter().map(|s| s.id)).all(|id| !id.is_empty() && !id.contains(' ')));
}

#[test]
fn a_name_that_is_no_themes_gives_the_default_one() {
    assert_eq!(combo("nord").name, "Nord");
    assert_eq!(combo("").id, COMBOS[0].id, "nothing picked yet");
    assert_eq!(combo("from-a-newer-version").id, "cachemire-dark");
    assert_eq!(syntax("monokai").name, "Monokai");
    assert_eq!(syntax("").id, SYNTAXES[0].id);
    assert_eq!(syntax("unknown").id, "cachemire");
}

#[test]
fn colors_mix_by_shares_and_contrast_goes_from_one_to_twenty_one() {
    let (black, white) = (rgb(0x000000), rgb(0xffffff));
    assert_eq!(rgb(0x12abef), Color::from_rgb_u8(0x12, 0xab, 0xef));
    assert_eq!(mix(black, white, 0.0), black);
    assert_eq!(mix(black, white, 1.0), white);
    assert_eq!(mix(black, white, 0.5), Color::from_rgb_u8(128, 128, 128));
    assert_eq!(mix(rgb(0x102030), rgb(0x302010), 0.5), rgb(0x202020));
    assert!((luminance(black), luminance(white)) == (0.0, 1.0));
    assert!((contrast(black, white) - 21.0).abs() < 0.01);
    assert!((contrast(white, black) - 21.0).abs() < 0.01, "whichever comes first");
    assert!((contrast(rgb(0x777777), rgb(0x777777)) - 1.0).abs() < 0.001);
}

#[test]
fn a_color_that_reads_is_kept_and_one_that_does_not_goes_toward_the_text() {
    let (dark, text) = (rgb(0x151517), rgb(0xe7e7ea));
    assert_eq!(readable(rgb(0x7ec9ff), dark, text), rgb(0x7ec9ff));
    let lifted = readable(rgb(0x202024), dark, text);
    assert!(contrast(lifted, dark) >= 3.0);
    assert_ne!(lifted, rgb(0x202024));

    // a pale color on a light background is darkened toward its text
    let (light, ink) = (rgb(0xffffff), rgb(0x1f2328));
    let darkened = readable(rgb(0xf1fa8c), light, ink);
    assert!(contrast(darkened, light) >= 3.0);
    assert!(luminance(darkened) < luminance(rgb(0xf1fa8c)));
}

#[test]
fn the_default_theme_is_the_colors_the_app_always_had() {
    let palette = derive(&COMBOS[0]);
    assert_eq!(palette.bg_app_base, rgb(0x151517));
    assert_eq!(palette.text_primary, rgb(0xe7e7ea));
    assert_eq!(palette.accent_blue, rgb(0x5b9dff));
    assert_eq!(palette.accent_yellow, rgb(0xffbd2e));
    // the status colors read on it as they are
    assert_eq!(palette.accent_red, rgb(0xff5f56));
    assert_eq!(palette.accent_green, rgb(0x29c46f));
    assert_eq!(palette.accent_orange, rgb(0xff9f45));
    assert_eq!(palette.accent_purple, rgb(0xb389f9));
    assert_eq!(palette.accent_cyan, rgb(0x6fd1e0));
    assert_eq!(palette.accent_text, rgb(0xffffff));
    // what is worked out stays within a few units of what was written by hand
    let was = [
        (palette.bg_panel, 0x19191c),
        (palette.bg_sunken, 0x111113),
        (palette.bg_elevated, 0x222226),
        (palette.bg_hover, 0x232327),
        (palette.bg_hover_elevated, 0x34343b),
        (palette.bg_selected, 0x2a4a76),
        (palette.bg_selected_soft, 0x22344d),
        (palette.border, 0x2d2d30),
        (palette.border_soft, 0x333336),
        (palette.text_secondary, 0xb4b4b9),
        (palette.text_tertiary, 0x909096),
        (palette.text_disabled, 0x6e6e73),
        (palette.accent_blue_dim, 0x3d6fc4),
    ];
    for (now, before) in was {
        assert!(gap(now, rgb(before)) <= 11, "{:?} is far from #{before:06x}", channels(now));
    }
}

#[test]
fn every_theme_keeps_its_four_colors_and_stays_readable() {
    for combo in COMBOS {
        let palette = derive(combo);
        let [background, text, accent, highlight] = combo.colors.map(rgb);
        assert_eq!(
            (palette.bg_app_base, palette.text_primary, palette.accent_blue, palette.accent_yellow),
            (background, text, accent, highlight),
            "{}",
            combo.name
        );
        for surface in [palette.bg_app_base, palette.bg_panel, palette.bg_sunken, palette.bg_elevated, palette.bg_hover_elevated] {
            assert!(contrast(palette.text_primary, surface) >= 6.5, "{}: text on a surface", combo.name);
            assert!(contrast(palette.text_secondary, surface) >= 4.0, "{}: secondary text", combo.name);
            assert!(contrast(palette.text_tertiary, surface) >= 2.7, "{}: tertiary text", combo.name);
        }
        for status in [palette.accent_red, palette.accent_green, palette.accent_orange, palette.accent_purple, palette.accent_cyan] {
            assert!(contrast(status, background) >= 3.0, "{}: a status color", combo.name);
        }
        assert!(contrast(palette.accent_text, accent) >= 2.4, "{}: the text of a primary button", combo.name);
        // the levels of text go from the text toward the background, the surfaces the other way
        let toward_background = [palette.text_primary, palette.text_secondary, palette.text_tertiary, palette.text_disabled];
        assert!(toward_background.windows(2).all(|pair| contrast(pair[0], background) > contrast(pair[1], background)), "{}", combo.name);
        let raised = [palette.bg_panel, palette.bg_elevated, palette.bg_hover, palette.bg_hover_elevated];
        assert!(raised.windows(2).all(|pair| contrast(pair[0], text) > contrast(pair[1], text)), "{}", combo.name);
        assert!(contrast(palette.border_soft, background) > contrast(palette.border, background), "{}", combo.name);
        assert!(contrast(palette.bg_selected, background) > contrast(palette.bg_selected_soft, background), "{}", combo.name);
    }
}

#[test]
fn light_and_dark_themes_sink_their_boxes_their_own_way() {
    let dark = derive(combo("cachemire-dark"));
    assert!(luminance(dark.bg_sunken) < luminance(dark.bg_app_base), "darker than a dark background");
    let light = derive(combo("github-light"));
    assert!(luminance(light.bg_sunken) < luminance(light.bg_app_base), "a shade under a light one");
    assert!(contrast(light.bg_sunken, light.bg_app_base) < 1.2, "but only just");
    // a light accent takes dark text, a dark one white
    assert_eq!(derive(combo("nord")).accent_text, rgb(0x151517));
    assert_eq!(derive(combo("github-light")).accent_text, rgb(0xffffff));
}

#[test]
fn every_syntax_theme_reads_on_every_theme() {
    for combo in COMBOS {
        let background = derive(combo).bg_sunken;
        for syntax in SYNTAXES {
            let paint = paint_for(combo, syntax);
            let tokens = [paint.key, paint.string, paint.number, paint.bool, paint.punct, paint.variable, paint.escape, paint.too_long];
            for token in tokens {
                assert!(contrast(token, background) >= 3.0, "{} on {}: {:?}", syntax.name, combo.name, channels(token));
            }
            assert!(contrast(paint.mark_text, paint.mark_find) >= 4.5, "{}: the text on a find mark", combo.name);
            assert!(contrast(paint.mark_text, paint.mark_filter) >= 3.0, "{}: the text on a filter mark", combo.name);
        }
    }
}

#[test]
fn the_default_paint_is_the_syntax_colors_the_app_always_had() {
    let paint = paint_for(&COMBOS[0], &SYNTAXES[0]);
    assert_eq!(
        [paint.key, paint.string, paint.number, paint.bool, paint.punct, paint.variable],
        [0x7ec9ff, 0x9ee6a8, 0xc9a0ff, 0xffb454, 0x8e8e94, 0xff8fb8].map(rgb)
    );
    assert_eq!(paint.escape, rgb(0xe7e7ea));
    assert_eq!((paint.keyword, paint.comment, paint.error), (rgb(0x5b9dff), derive(&COMBOS[0]).text_tertiary, rgb(0xff5f56)));
    assert_eq!((paint.mark_filter, paint.mark_current), (rgb(0xffbd2e), rgb(0xff9f45)));
    assert!(gap(paint.mark_find, rgb(0xffe066)) <= 4, "the pale yellow of a find match");
    assert_eq!(paint.mark_text, rgb(0x151517), "dark text on the marks");
}

#[test]
fn a_syntax_theme_changes_the_tokens_and_a_theme_what_surrounds_them() {
    let default = paint_for(&COMBOS[0], &SYNTAXES[0]);
    let monokai = paint_for(&COMBOS[0], syntax("monokai"));
    assert_eq!(monokai.key, rgb(0xf92672));
    assert_ne!(monokai.string, default.string);
    assert_eq!((monokai.keyword, monokai.mark_filter, monokai.comment), (default.keyword, default.mark_filter, default.comment));

    let nord = paint_for(combo("nord"), &SYNTAXES[0]);
    assert_eq!((nord.keyword, nord.mark_filter, nord.escape), (rgb(0x88c0d0), rgb(0xebcb8b), rgb(0xe5e9f0)));
    assert_eq!(nord.key, default.key, "a token that reads there keeps its color");

    // a syntax theme made for dark backgrounds is darkened on a light one
    let on_light = paint_for(combo("github-light"), syntax("dracula"));
    assert_ne!(on_light.string, rgb(0xf1fa8c));
}

#[test]
fn the_paint_in_use_is_the_default_until_one_is_set() {
    let default = paint_for(&COMBOS[0], &SYNTAXES[0]);
    assert_eq!(paint().key, default.key);
    set_paint(paint_for(combo("dracula"), syntax("dracula")));
    assert_eq!(paint().key, rgb(0x8be9fd));
    assert_eq!(paint().keyword, rgb(0xbd93f9));
}
