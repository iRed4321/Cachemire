//! The fonts of the interface and of values and queries: the ones picked go to the `Theme`
//! global, and the Display settings' two menus list the fonts installed on the computer
//! (read once, off the UI thread), monospaced ones only for values and queries.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use skrifa::MetadataProvider;
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::{MainWindow, Preferences, Theme};

/// The fonts the app brings, used while none is picked.
pub const DEFAULT_UI: &str = "Roboto";
pub const DEFAULT_MONO: &str = "JetBrains Mono";

/// The installed font families with Latin letters, by name, and those whose letters all
/// take the same width.
struct Installed {
    all: Vec<String>,
    mono: Vec<String>,
}

static INSTALLED: OnceLock<Installed> = OnceLock::new();
// set once a thread was started reading them ahead of a menu opening (see `prepare`)
static PREPARING: AtomicBool = AtomicBool::new(false);

/// Reads the installed families, as Slint finds them (both go through fontique).
fn read_installed() -> Installed {
    let mut collection = fontique::Collection::new(fontique::CollectionOptions { shared: false, system_fonts: true });
    let mut names: Vec<String> = collection.family_names().filter(|name| !name.starts_with('.')).map(str::to_string).collect();
    names.sort_by_key(|name| name.to_lowercase());
    names.dedup();
    let (mut all, mut mono) = (Vec::new(), Vec::new());
    for name in names {
        if name == DEFAULT_UI || name == DEFAULT_MONO {
            continue;
        }
        let family = collection.family_id(&name).and_then(|id| collection.family(id));
        let Some(fixed) = family.as_ref().and_then(|family| family.default_font()).and_then(fixed_width) else { continue };
        if fixed {
            mono.push(name.clone());
        }
        all.push(name);
    }
    tracing::debug!(fonts = all.len(), monospaced = mono.len(), "read the installed fonts");
    Installed { all, mono }
}

/// Whether `font`'s Latin letters all take the same width; `None` when it lacks some.
fn fixed_width(font: &fontique::FontInfo) -> Option<bool> {
    let blob = font.load(None)?;
    let font = skrifa::FontRef::from_index(blob.data(), font.index()).ok()?;
    let charmap = font.charmap();
    let metrics = font.glyph_metrics(skrifa::instance::Size::unscaled(), skrifa::instance::LocationRef::default());
    let widths: Option<Vec<f32>> =
        ['i', 'M', 'W', 'a'].into_iter().map(|c| charmap.map(c).and_then(|glyph| metrics.advance_width(glyph))).collect();
    let widths = widths?;
    Some(widths.iter().all(|width| *width == widths[0]))
}

/// What a menu lists: "" (the app's own font) then the installed families holding `filter`.
fn options(installed: &Installed, mono: bool, filter: &str) -> Vec<SharedString> {
    let words = filter.trim().to_lowercase();
    let default = if mono { DEFAULT_MONO } else { DEFAULT_UI };
    let families = if mono { &installed.mono } else { &installed.all };
    let default = default.to_lowercase().contains(&words).then(SharedString::new);
    default.into_iter().chain(families.iter().filter(|name| name.to_lowercase().contains(&words)).map(SharedString::from)).collect()
}

/// Starts reading the installed fonts on a thread of their own, once, so a menu opened later
/// finds them read (or waits for that same reading: `OnceLock` runs it once).
pub fn prepare() {
    if INSTALLED.get().is_none() && !PREPARING.swap(true, Ordering::Relaxed) {
        std::thread::spawn(|| {
            INSTALLED.get_or_init(read_installed);
        });
    }
}

/// Fills the open menu's list (`Preferences.font-options`), reading the installed fonts
/// first on a thread of their own if that's not done yet.
pub fn request(window: &MainWindow, mono: bool, filter: String) {
    let preferences = window.global::<Preferences>();
    if let Some(installed) = INSTALLED.get() {
        preferences.set_font_options(ModelRc::new(VecModel::from(options(installed, mono, &filter))));
        return;
    }
    preferences.set_fonts_loading(true);
    let weak = window.as_weak();
    std::thread::spawn(move || {
        let installed = INSTALLED.get_or_init(read_installed);
        let _ = slint::invoke_from_event_loop(move || {
            let Some(window) = weak.upgrade() else { return };
            let preferences = window.global::<Preferences>();
            preferences.set_fonts_loading(false);
            preferences.set_font_options(ModelRc::new(VecModel::from(options(installed, mono, &filter))));
        });
    });
}

/// Gives the interface and the values the fonts picked ("" for the app's own).
pub fn apply(window: &MainWindow, ui: &str, mono: &str) {
    let theme = window.global::<Theme>();
    theme.set_font_ui(if ui.is_empty() { DEFAULT_UI } else { ui }.into());
    theme.set_font_mono(if mono.is_empty() { DEFAULT_MONO } else { mono }.into());
    let preferences = window.global::<Preferences>();
    preferences.set_ui_font(ui.into());
    preferences.set_mono_font(mono.into());
}
