//! The app's colors: the themes (four colors each, from which every other interface color
//! is derived) and the syntax themes picked in the settings, set on the Slint `Theme`
//! global and kept as a [`Paint`] for the text Rust colors itself.

use std::sync::LazyLock;

use parking_lot::RwLock;
use slint::{Color, ComponentHandle, ModelRc, SharedString, VecModel};

use crate::{MainWindow, Preferences, Theme, ThemeChoice};

/// A theme: its name in the settings file, the one shown, and its four colors
/// (background, text, accent, highlight) as `0xRRGGBB`.
pub struct Combo {
    pub id: &'static str,
    pub name: &'static str,
    pub colors: [u32; 4],
}

/// The themes, in the order the settings list them; the first is the default.
pub const COMBOS: &[Combo] = &[
    Combo { id: "cachemire-dark", name: "Cachemire Dark", colors: [0x151517, 0xe7e7ea, 0x5b9dff, 0xffbd2e] },
    Combo { id: "nord", name: "Nord", colors: [0x2e3440, 0xe5e9f0, 0x88c0d0, 0xebcb8b] },
    Combo { id: "dracula", name: "Dracula", colors: [0x282a36, 0xf8f8f2, 0xbd93f9, 0xf1fa8c] },
    Combo { id: "tokyo-night", name: "Tokyo Night", colors: [0x1a1b26, 0xc0caf5, 0x7aa2f7, 0xe0af68] },
    Combo { id: "catppuccin-mocha", name: "Catppuccin Mocha", colors: [0x1e1e2e, 0xcdd6f4, 0x89b4fa, 0xf9e2af] },
    Combo { id: "gruvbox-dark", name: "Gruvbox Dark", colors: [0x282828, 0xebdbb2, 0x83a598, 0xfabd2f] },
    Combo { id: "solarized-dark", name: "Solarized Dark", colors: [0x002b36, 0xeee8d5, 0x268bd2, 0xb58900] },
    Combo { id: "cachemire-light", name: "Cachemire Light", colors: [0xf4f4f6, 0x1c1c1e, 0x2f6fde, 0xb97a00] },
    Combo { id: "github-light", name: "GitHub Light", colors: [0xffffff, 0x1f2328, 0x0969da, 0x9a6700] },
    Combo { id: "solarized-light", name: "Solarized Light", colors: [0xfdf6e3, 0x073642, 0x268bd2, 0xb58900] },
];

/// A syntax theme: its name in the settings file, the one shown, and its colors as `0xRRGGBB`.
pub struct Syntax {
    pub id: &'static str,
    pub name: &'static str,
    key: u32,
    string: u32,
    number: u32,
    bool: u32,
    punct: u32,
    variable: u32,
}

/// The syntax themes, in the order the settings list them; the first is the default.
pub const SYNTAXES: &[Syntax] = &[
    Syntax {
        id: "cachemire",
        name: "Cachemire",
        key: 0x7ec9ff,
        string: 0x9ee6a8,
        number: 0xc9a0ff,
        bool: 0xffb454,
        punct: 0x8e8e94,
        variable: 0xff8fb8,
    },
    Syntax {
        id: "monokai",
        name: "Monokai",
        key: 0xf92672,
        string: 0xe6db74,
        number: 0xae81ff,
        bool: 0x66d9ef,
        punct: 0x8f908a,
        variable: 0xfd971f,
    },
    Syntax {
        id: "dracula",
        name: "Dracula",
        key: 0x8be9fd,
        string: 0xf1fa8c,
        number: 0xbd93f9,
        bool: 0xff79c6,
        punct: 0x7b88b8,
        variable: 0xffb86c,
    },
    Syntax {
        id: "one-dark",
        name: "One Dark",
        key: 0xe06c75,
        string: 0x98c379,
        number: 0xd19a66,
        bool: 0x56b6c2,
        punct: 0x7f848e,
        variable: 0xc678dd,
    },
    Syntax {
        id: "nord",
        name: "Nord",
        key: 0x88c0d0,
        string: 0xa3be8c,
        number: 0xb48ead,
        bool: 0x81a1c1,
        punct: 0x7b88a1,
        variable: 0xebcb8b,
    },
    Syntax {
        id: "tokyo-night",
        name: "Tokyo Night",
        key: 0x7aa2f7,
        string: 0x9ece6a,
        number: 0xff9e64,
        bool: 0xbb9af7,
        punct: 0x737aa2,
        variable: 0xf7768e,
    },
    Syntax {
        id: "gruvbox",
        name: "Gruvbox",
        key: 0x83a598,
        string: 0xb8bb26,
        number: 0xd3869b,
        bool: 0xfe8019,
        punct: 0x928374,
        variable: 0xfabd2f,
    },
    Syntax {
        id: "github-dark",
        name: "GitHub Dark",
        key: 0x7ee787,
        string: 0xa5d6ff,
        number: 0x79c0ff,
        bool: 0xff7b72,
        punct: 0x8b949e,
        variable: 0xffa657,
    },
    Syntax {
        id: "github-light",
        name: "GitHub Light",
        key: 0x116329,
        string: 0x0a3069,
        number: 0x0550ae,
        bool: 0xcf222e,
        punct: 0x6e7781,
        variable: 0x953800,
    },
    Syntax {
        id: "solarized",
        name: "Solarized",
        key: 0x268bd2,
        string: 0x2aa198,
        number: 0xd33682,
        bool: 0xcb4b16,
        punct: 0x839496,
        variable: 0x6c71c4,
    },
];

/// The theme named `id`, the default one for a name that isn't one's.
pub fn combo(id: &str) -> &'static Combo {
    COMBOS.iter().find(|combo| combo.id == id).unwrap_or(&COMBOS[0])
}

/// The syntax theme named `id`, the default one for a name that isn't one's.
pub fn syntax(id: &str) -> &'static Syntax {
    SYNTAXES.iter().find(|syntax| syntax.id == id).unwrap_or(&SYNTAXES[0])
}

fn rgb(hex: u32) -> Color {
    Color::from_rgb_u8((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

/// `from` with `share` (0 to 1) of `to` mixed in.
fn mix(from: Color, to: Color, share: f32) -> Color {
    let channel = |a: u8, b: u8| (f32::from(a) + (f32::from(b) - f32::from(a)) * share).round().clamp(0.0, 255.0) as u8;
    Color::from_rgb_u8(channel(from.red(), to.red()), channel(from.green(), to.green()), channel(from.blue(), to.blue()))
}

/// How light a color looks, from 0 (black) to 1 (white): its WCAG relative luminance.
fn luminance(color: Color) -> f32 {
    let linear = |channel: u8| {
        let c = f32::from(channel) / 255.0;
        if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
    };
    0.2126 * linear(color.red()) + 0.7152 * linear(color.green()) + 0.0722 * linear(color.blue())
}

/// The WCAG contrast ratio of two colors, from 1 (the same) to 21 (black on white).
fn contrast(a: Color, b: Color) -> f32 {
    let (a, b) = (luminance(a), luminance(b));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

/// `color` as it reads on `background`: itself when it stands out enough, else with as
/// much of `text` mixed in as it takes.
fn readable(color: Color, background: Color, text: Color) -> Color {
    (0..=4).map(|step| mix(color, text, step as f32 * 0.2)).find(|c| contrast(*c, background) >= 3.0).unwrap_or(text)
}

/// How different two colors look: their distance in CIE L*a*b* (about 2 is barely seen, 100 is opposite).
fn distance(a: Color, b: Color) -> f32 {
    let lab = |color: Color| {
        let linear = |channel: u8| {
            let c = f32::from(channel) / 255.0;
            if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
        };
        let (r, g, b) = (linear(color.red()), linear(color.green()), linear(color.blue()));
        let curve = |t: f32| if t > 0.008856 { t.cbrt() } else { 7.787 * t + 16.0 / 116.0 };
        let x = curve((0.4124 * r + 0.3576 * g + 0.1805 * b) / 0.95047);
        let y = curve(0.2126 * r + 0.7152 * g + 0.0722 * b);
        let z = curve((0.0193 * r + 0.1192 * g + 0.9505 * b) / 1.08883);
        [116.0 * y - 16.0, 500.0 * (x - y), 200.0 * (y - z)]
    };
    let (a, b) = (lab(a), lab(b));
    a.iter().zip(b).map(|(a, b)| (a - b) * (a - b)).sum::<f32>().sqrt()
}

/// Two colors closer than this are taken for one another.
const TOLD_APART: f32 = 16.0;

/// The interface colors, each named as its `Theme` property.
struct Palette {
    bg_app_base: Color,
    bg_panel: Color,
    bg_sunken: Color,
    bg_elevated: Color,
    bg_hover: Color,
    bg_hover_elevated: Color,
    bg_selected: Color,
    bg_selected_soft: Color,
    border: Color,
    border_soft: Color,
    text_primary: Color,
    text_secondary: Color,
    text_tertiary: Color,
    text_disabled: Color,
    accent_blue: Color,
    accent_blue_dim: Color,
    accent_text: Color,
    accent_red: Color,
    accent_yellow: Color,
    accent_green: Color,
    accent_orange: Color,
    accent_purple: Color,
    accent_cyan: Color,
}

/// Every interface color of `combo`: surfaces and borders are its background with some of its
/// text mixed in, the dimmer texts the reverse, selections its accent over its background;
/// the status colors keep their hue and are made to read on the background.
fn derive(combo: &Combo) -> Palette {
    let [background, text, accent, highlight] = combo.colors.map(rgb);
    let dark = luminance(background) < 0.4;
    let (black, white) = (Color::from_rgb_u8(0, 0, 0), Color::from_rgb_u8(255, 255, 255));
    let raised = |share: f32| mix(background, text, share);
    // a status color: the first of `choices` that reads here and isn't taken for one of `taken`
    let signal = |choices: &[u32], taken: &[Color]| {
        let reads = |hex: &u32| readable(rgb(*hex), background, text);
        let apart = |color: &Color| taken.iter().all(|other| distance(*color, *other) >= TOLD_APART);
        choices.iter().map(reads).find(apart).unwrap_or_else(|| reads(&choices[0]))
    };
    let red = signal(&[0xff5f56], &[]);
    let green = signal(&[0x29c46f, 0xa6d94a], &[accent, highlight, red]);
    let orange = signal(&[0xff9f45, 0xff79c6], &[accent, highlight, red, green]);
    let purple = signal(&[0xb389f9, 0xff79c6, 0xf06ad8], &[accent, highlight, red, green, orange]);
    let cyan = signal(&[0x6fd1e0, 0x5b9dff, 0x3fd6a8], &[accent, highlight, red, green, orange, purple]);
    Palette {
        bg_app_base: background,
        bg_panel: raised(0.02),
        bg_sunken: if dark { mix(background, black, 0.2) } else { raised(0.045) },
        bg_elevated: raised(0.06),
        bg_hover: raised(0.07),
        bg_hover_elevated: raised(0.15),
        bg_selected: mix(background, accent, 0.4),
        bg_selected_soft: mix(background, accent, 0.22),
        border: raised(0.115),
        border_soft: raised(0.14),
        text_primary: text,
        text_secondary: mix(text, background, 0.24),
        text_tertiary: mix(text, background, 0.41),
        text_disabled: mix(text, background, 0.58),
        accent_blue: accent,
        accent_blue_dim: mix(accent, background, 0.3),
        accent_text: if contrast(white, accent) >= 2.5 { white } else { rgb(0x151517) },
        accent_red: red,
        accent_yellow: highlight,
        accent_green: green,
        accent_orange: orange,
        accent_purple: purple,
        accent_cyan: cyan,
    }
}

/// The colors of the text Rust colors itself: syntax tokens (JSON, YAML, XML, Redisql)
/// and the marks behind search and find matches.
#[derive(Clone, Copy)]
pub struct Paint {
    pub key: Color,
    pub string: Color,
    pub number: Color,
    pub bool: Color,
    pub punct: Color,
    // the backslashes an escaped value adds, a string too long to show, a Redisql `$variable`
    pub escape: Color,
    pub too_long: Color,
    pub variable: Color,
    pub keyword: Color,
    pub comment: Color,
    pub error: Color,
    // the backgrounds of the filter's matches, of the current find match and of the
    // others, and the text on them
    pub mark_filter: Color,
    pub mark_current: Color,
    pub mark_find: Color,
    pub mark_text: Color,
}

/// What `syntax` and the marks look like on `combo`: a syntax color too close to the
/// background takes some of the text's.
fn paint_for(combo: &Combo, syntax: &Syntax) -> Paint {
    let palette = derive(combo);
    let (background, text) = (palette.bg_sunken, palette.text_primary);
    let token = |hex: u32| readable(rgb(hex), background, text);
    // a paler shade of the highlight, as a marker pen's
    let mark_find = mix(palette.accent_yellow, rgb(0xffffa0), 0.5);
    let mark_text = [palette.bg_app_base, text].into_iter().max_by(|a, b| contrast(*a, mark_find).total_cmp(&contrast(*b, mark_find)));
    Paint {
        key: token(syntax.key),
        string: token(syntax.string),
        number: token(syntax.number),
        bool: token(syntax.bool),
        punct: token(syntax.punct),
        escape: text,
        too_long: mix(palette.accent_red, text, 0.25),
        variable: token(syntax.variable),
        keyword: palette.accent_blue,
        comment: palette.text_tertiary,
        error: palette.accent_red,
        mark_filter: palette.accent_yellow,
        mark_current: palette.accent_orange,
        mark_find,
        mark_text: mark_text.unwrap_or(palette.bg_app_base),
    }
}

static PAINT: LazyLock<RwLock<Paint>> = LazyLock::new(|| RwLock::new(paint_for(&COMBOS[0], &SYNTAXES[0])));

// in the tests, each one's thread has its own applied paint, so none changes another's colors
#[cfg(test)]
thread_local! {
    static THREAD_PAINT: std::cell::Cell<Option<Paint>> = const { std::cell::Cell::new(None) };
}

/// The colors text is painted in now: those of the theme and syntax theme last applied.
pub fn paint() -> Paint {
    #[cfg(test)]
    if let Some(paint) = THREAD_PAINT.get() {
        return paint;
    }
    *PAINT.read()
}

/// Makes `paint` what [`paint`] gives from now on.
fn set_paint(paint: Paint) {
    #[cfg(test)]
    THREAD_PAINT.set(Some(paint));
    #[cfg(not(test))]
    {
        *PAINT.write() = paint;
    }
}

/// Makes the window take the theme `combo` and the syntax theme `syntax` (names as in the
/// settings file): the `Theme` colors, the [`paint`], and which choice the settings tick.
pub fn apply(window: &MainWindow, combo_id: &str, syntax_id: &str) {
    let (combo, syntax) = (combo(combo_id), syntax(syntax_id));
    let palette = derive(combo);
    set_paint(paint_for(combo, syntax));
    let theme = window.global::<Theme>();
    theme.set_bg_app_base(palette.bg_app_base);
    theme.set_bg_panel(palette.bg_panel);
    theme.set_bg_sunken(palette.bg_sunken);
    theme.set_bg_elevated(palette.bg_elevated);
    theme.set_bg_hover(palette.bg_hover);
    theme.set_bg_hover_elevated(palette.bg_hover_elevated);
    theme.set_bg_selected(palette.bg_selected);
    theme.set_bg_selected_soft(palette.bg_selected_soft);
    theme.set_border(palette.border);
    theme.set_border_soft(palette.border_soft);
    theme.set_text_primary(palette.text_primary);
    theme.set_text_secondary(palette.text_secondary);
    theme.set_text_tertiary(palette.text_tertiary);
    theme.set_text_disabled(palette.text_disabled);
    theme.set_accent_blue(palette.accent_blue);
    theme.set_accent_blue_dim(palette.accent_blue_dim);
    theme.set_accent_text(palette.accent_text);
    theme.set_accent_red(palette.accent_red);
    theme.set_accent_yellow(palette.accent_yellow);
    theme.set_accent_green(palette.accent_green);
    theme.set_accent_orange(palette.accent_orange);
    theme.set_accent_purple(palette.accent_purple);
    theme.set_accent_cyan(palette.accent_cyan);
    theme.set_paint_tick(theme.get_paint_tick().wrapping_add(1));
    let preferences = window.global::<Preferences>();
    preferences.set_theme(combo.id.into());
    preferences.set_syntax_theme(syntax.id.into());
}

/// One choice of a list in the settings: its name and the colors of its swatches.
fn choice(id: &str, name: &str, colors: &[u32]) -> ThemeChoice {
    let colors: Vec<Color> = colors.iter().copied().map(rgb).collect();
    ThemeChoice { id: SharedString::from(id), name: SharedString::from(name), colors: ModelRc::new(VecModel::from(colors)) }
}

/// Fills the settings' two lists: the themes with their four colors, the syntax themes
/// with those of a key, a string, a number and a boolean.
pub fn init(window: &MainWindow) {
    let combos: Vec<ThemeChoice> = COMBOS.iter().map(|c| choice(c.id, c.name, &c.colors)).collect();
    let syntaxes: Vec<ThemeChoice> = SYNTAXES.iter().map(|s| choice(s.id, s.name, &[s.key, s.string, s.number, s.bool])).collect();
    let preferences = window.global::<Preferences>();
    preferences.set_themes(ModelRc::new(VecModel::from(combos)));
    preferences.set_syntax_themes(ModelRc::new(VecModel::from(syntaxes)));
}

#[cfg(test)]
mod tests;
