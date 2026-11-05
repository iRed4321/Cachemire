//! The keyboard shortcuts of the app's own actions: their default keys, the ones picked
//! on the settings' Key bindings page instead, and the `Shortcuts` global the UI reads.
//! Standard keys (copy, paste, undo, Esc...) aren't among them and never change.

use std::borrow::Cow;
use std::cell::RefCell;
use std::sync::Arc;

use cachemire_core::state::AppState;
use rust_i18n::t;
use slint::{ComponentHandle, Keys, ModelRc, SharedString, VecModel};

use crate::error::Localize;
use crate::{MainWindow, Preferences, ShortcutAction, ShortcutRow, Shortcuts};

/// Where a shortcut works: two can't share keys in one place, nor with a window-wide
/// one, which the focused box would hide.
#[derive(Clone, Copy, PartialEq)]
enum Scope {
    Window,
    Query,
    ValueEditor,
}

impl Scope {
    fn title(self) -> Cow<'static, str> {
        match self {
            Scope::Window => t!("Window"),
            Scope::Query => t!("Query box"),
            Scope::ValueEditor => t!("Value editor"),
        }
    }
}

fn title(action: ShortcutAction) -> Cow<'static, str> {
    match action {
        ShortcutAction::ZoomIn => t!("Zoom in"),
        ShortcutAction::ZoomOut => t!("Zoom out"),
        ShortcutAction::ZoomReset => t!("Reset the zoom"),
        ShortcutAction::Search => t!("Search the open key, or open a search tab"),
        ShortcutAction::CloseTab => t!("Close the tab"),
        ShortcutAction::RunStatement => t!("Run the statement at the caret"),
        ShortcutAction::RunScript => t!("Run the whole script"),
        ShortcutAction::Suggest => t!("Show the suggestions"),
        ShortcutAction::HistoryPrevious => t!("Previous query of the history"),
        ShortcutAction::HistoryNext => t!("Next query of the history"),
        ShortcutAction::SaveValue => t!("Save the value"),
    }
}

thread_local! {
    /// What the settings' search box holds, and the names it also looks in (see `filter-changed`).
    static FILTER: RefCell<Filter> = RefCell::default();
}

#[derive(Default, Clone)]
struct Filter {
    query: String,
    page: String,
    standard: String,
}

struct Spec {
    action: ShortcutAction,
    /// its name in the settings file
    code: &'static str,
    scope: Scope,
    /// its keys until others are picked, as `Keys::from_parts` takes them
    defaults: &'static [&'static [&'static str]],
}

const SPECS: [Spec; 11] = [
    Spec {
        action: ShortcutAction::ZoomIn,
        code: "zoomIn",
        scope: Scope::Window,
        defaults: &[&["Control", "Plus"], &["Control", "Equals"]],
    },
    Spec { action: ShortcutAction::ZoomOut, code: "zoomOut", scope: Scope::Window, defaults: &[&["Control", "HyphenMinus"]] },
    Spec { action: ShortcutAction::ZoomReset, code: "zoomReset", scope: Scope::Window, defaults: &[&["Control", "Digit0"]] },
    Spec { action: ShortcutAction::Search, code: "search", scope: Scope::Window, defaults: &[&["Control", "F"]] },
    Spec { action: ShortcutAction::CloseTab, code: "closeTab", scope: Scope::Window, defaults: &[&["Control", "W"]] },
    Spec { action: ShortcutAction::RunStatement, code: "runStatement", scope: Scope::Query, defaults: &[&["Control", "Return"]] },
    Spec { action: ShortcutAction::RunScript, code: "runScript", scope: Scope::Query, defaults: &[&["Control", "Shift", "Return"]] },
    Spec { action: ShortcutAction::Suggest, code: "suggest", scope: Scope::Query, defaults: &[&["Control", "Space"]] },
    Spec { action: ShortcutAction::HistoryPrevious, code: "historyPrevious", scope: Scope::Query, defaults: &[&["Control", "UpArrow"]] },
    Spec { action: ShortcutAction::HistoryNext, code: "historyNext", scope: Scope::Query, defaults: &[&["Control", "DownArrow"]] },
    Spec {
        action: ShortcutAction::SaveValue,
        code: "saveValue",
        scope: Scope::ValueEditor,
        defaults: &[&["Control", "Return"], &["Control", "S"]],
    },
];

/// The keys that keep their usual use: copy, cut, paste, select all, undo and redo.
const STANDARD: [&[&str]; 7] = [
    &["Control", "C"],
    &["Control", "X"],
    &["Control", "V"],
    &["Control", "A"],
    &["Control", "Z"],
    &["Control", "Shift", "Z"],
    &["Control", "Y"],
];

/// The standard keys as the page lists them: what they do, and their keys (`Keys` parts, or a label).
fn standard_rows() -> Vec<(Cow<'static, str>, Vec<String>)> {
    let keys = |parts: &[&str]| Chord::of(&parse(parts)).label();
    let ctrl = |rest: &str| if cfg!(target_os = "macos") { format!("⌘{rest}") } else { format!("Ctrl+{rest}") };
    vec![
        (t!("Copy"), vec![keys(&["Control", "C"])]),
        (t!("Cut"), vec![keys(&["Control", "X"])]),
        (t!("Paste"), vec![keys(&["Control", "V"])]),
        (t!("Select all"), vec![keys(&["Control", "A"])]),
        (t!("Undo"), vec![keys(&["Control", "Z"])]),
        (t!("Redo"), vec![keys(&["Control", "Y"]), keys(&["Control", "Shift", "Z"])]),
        (t!("Close a dialog or a menu, fold the value under the pointer"), vec![keys(&["Escape"])]),
        (t!("Move between a table's cells"), vec!["↑ ↓ ← →".into()]),
        (t!("Pick a suggestion"), vec!["↑ ↓".into(), "Tab".into(), t!("Enter").into_owned()]),
        (t!("Zoom"), vec![ctrl(&t!("Mouse wheel"))]),
    ]
}

fn parse(parts: &[&str]) -> Keys {
    Keys::from_parts(parts.iter().copied()).expect("a built-in shortcut is valid")
}

fn spec(action: ShortcutAction) -> &'static Spec {
    SPECS.iter().find(|spec| spec.action == action).expect("every action has a spec")
}

/// What starts the action now, and whether it was picked rather than the default.
fn keys_of(state: &AppState, spec: &Spec) -> (Vec<Keys>, bool) {
    let picked = state
        .settings
        .lock()
        .shortcut(spec.code)
        .and_then(|parts| Keys::from_parts(parts.iter().map(String::as_str)).ok())
        .filter(|keys| *keys != Keys::default());
    match picked {
        Some(keys) => (vec![keys], true),
        None => (spec.defaults.iter().map(|parts| parse(parts)).collect(), false),
    }
}

/// A key combination taken apart, to match a key press against, compare and show.
struct Chord {
    key: String,
    control: bool,
    meta: bool,
    /// `None` when the modifier may be up or down
    shift: Option<bool>,
    alt: Option<bool>,
}

impl Chord {
    fn of(keys: &Keys) -> Self {
        let mut chord = Chord { key: String::new(), control: false, meta: false, shift: Some(false), alt: Some(false) };
        for part in keys.to_parts() {
            match part {
                "Control" => chord.control = true,
                "Meta" => chord.meta = true,
                "Shift" => chord.shift = Some(true),
                "Shift?" => chord.shift = None,
                "Alt" => chord.alt = Some(true),
                "Alt?" => chord.alt = None,
                key => chord.key = key.to_string(),
            }
        }
        chord
    }

    /// The same test as a `KeyBinding`'s.
    fn matches(&self, text: &str, control: bool, shift: bool, alt: bool, meta: bool) -> bool {
        !self.key.is_empty()
            && text.to_lowercase() == self.key
            && control == self.control
            && meta == self.meta
            && self.shift.is_none_or(|s| s == shift)
            && self.alt.is_none_or(|a| a == alt)
    }

    /// Whether a key press can match both.
    fn overlaps(&self, other: &Chord) -> bool {
        let both = |a: Option<bool>, b: Option<bool>| a.is_none() || b.is_none() || a == b;
        self.key == other.key
            && self.control == other.control
            && self.meta == other.meta
            && both(self.shift, other.shift)
            && both(self.alt, other.alt)
    }

    fn function_key(&self) -> bool {
        let mut chars = self.key.chars();
        matches!((chars.next(), chars.next()), (Some('\u{F704}'..='\u{F71B}'), None))
    }

    /// As the Key bindings page shows it, in the interface language; on macOS, Slint's
    /// `control` is Command and `meta` Control, shown as symbols in Apple's order.
    fn label(&self) -> String {
        if cfg!(target_os = "macos") {
            let symbols = [(self.meta, "⌃"), (self.alt == Some(true), "⌥"), (self.shift == Some(true), "⇧"), (self.control, "⌘")];
            let modifiers: String = symbols.iter().filter(|(on, _)| *on).map(|(_, symbol)| *symbol).collect();
            return modifiers + &key_name(&self.key);
        }
        let mut parts: Vec<Cow<str>> = Vec::new();
        if self.meta {
            parts.push(if cfg!(windows) { "Win" } else { "Super" }.into());
        }
        if self.control {
            parts.push("Ctrl".into());
        }
        if self.alt == Some(true) {
            parts.push("Alt".into());
        }
        if self.shift == Some(true) {
            parts.push(t!("Shift"));
        }
        parts.push(key_name(&self.key).into());
        parts.join("+")
    }
}

/// A key's name on its key cap; the special keys are Slint's codes (`slint::platform::Key`).
fn key_name(key: &str) -> String {
    let mut chars = key.chars();
    let (Some(c), None) = (chars.next(), chars.next()) else { return key.to_uppercase() };
    let name: Cow<str> = match c {
        '\n' => t!("Enter"),
        ' ' => t!("Space"),
        '\t' => "Tab".into(),
        '\u{8}' => t!("Backspace"),
        '\u{7f}' => t!("Delete"),
        '\u{1b}' => "Esc".into(),
        '\u{F700}' => "↑".into(),
        '\u{F701}' => "↓".into(),
        '\u{F702}' => "←".into(),
        '\u{F703}' => "→".into(),
        '\u{F704}'..='\u{F71B}' => format!("F{}", c as u32 - 0xF703).into(),
        '\u{F727}' => t!("Insert"),
        '\u{F729}' => t!("Home"),
        '\u{F72B}' => t!("End"),
        '\u{F72C}' => t!("Page Up"),
        '\u{F72D}' => t!("Page Down"),
        _ => return c.to_uppercase().collect(),
    };
    name.into_owned()
}

/// The keys a key press makes, or none for a lone modifier (Shift, Ctrl...). A digit or a sign,
/// which Shift makes on some keyboard layouts, then matches with or without Shift.
fn pressed_keys(text: &str, control: bool, shift: bool, alt: bool, meta: bool) -> Option<Keys> {
    let key = text.to_lowercase();
    let mut chars = key.chars();
    let first = chars.next()?;
    let single = chars.next().is_none();
    if single && ('\u{10}'..='\u{18}').contains(&first) {
        return None;
    }
    let any_shift =
        single && !first.is_alphabetic() && !first.is_control() && !first.is_whitespace() && !('\u{F700}'..='\u{F7FF}').contains(&first);
    let parts = [
        meta.then_some("Meta"),
        control.then_some("Control"),
        alt.then_some("Alt"),
        if any_shift { Some("Shift?") } else { shift.then_some("Shift") },
        Some(key.as_str()),
    ];
    Keys::from_parts(parts.into_iter().flatten()).ok()
}

/// Why keys can't be bound to an action.
enum Refused {
    NoControl,
    Standard,
    Taken,
    NotSaved(String),
}

impl Refused {
    fn message(&self) -> String {
        match self {
            Refused::NoControl => t!("A shortcut needs Ctrl, unless it is a function key (F1 to F24).").into_owned(),
            Refused::Standard => t!("These keys keep their usual use (copy, paste, undo, Esc, Tab...).").into_owned(),
            Refused::Taken => t!("Another shortcut already uses these keys.").into_owned(),
            Refused::NotSaved(error) => error.clone(),
        }
    }
}

/// Saves `keys` as the ones of `spec`'s action, unless they're refused.
fn bind(state: &AppState, spec: &Spec, keys: Keys) -> Result<(), Refused> {
    let chord = Chord::of(&keys);
    if !chord.control && !chord.function_key() {
        return Err(Refused::NoControl);
    }
    if chord.key == "\u{1b}" || chord.key == "\t" || STANDARD.iter().any(|parts| Chord::of(&parse(parts)).overlaps(&chord)) {
        return Err(Refused::Standard);
    }
    let shares_place = |other: Scope| other == spec.scope || other == Scope::Window || spec.scope == Scope::Window;
    let taken = SPECS
        .iter()
        .filter(|other| other.action != spec.action && shares_place(other.scope))
        .any(|other| keys_of(state, other).0.iter().any(|keys| Chord::of(keys).overlaps(&chord)));
    if taken {
        return Err(Refused::Taken);
    }
    let default = spec.defaults.len() == 1 && parse(spec.defaults[0]) == keys;
    let stored = (!default).then(|| keys.to_parts().map(String::from).collect());
    state.settings.lock().set_shortcut(spec.code, stored).map_err(|e| Refused::NotSaved(e.localized()))
}

fn set_keys(global: &Shortcuts<'_>, action: ShortcutAction, first: Keys, second: Keys) {
    match action {
        ShortcutAction::ZoomIn => {
            global.set_zoom_in(first);
            global.set_zoom_in_2(second);
        }
        ShortcutAction::ZoomOut => global.set_zoom_out(first),
        ShortcutAction::ZoomReset => global.set_zoom_reset(first),
        ShortcutAction::Search => global.set_search(first),
        ShortcutAction::CloseTab => global.set_close_tab(first),
        ShortcutAction::RunStatement => global.set_run_statement(first),
        ShortcutAction::RunScript => global.set_run_script(first),
        ShortcutAction::Suggest => global.set_suggest(first),
        ShortcutAction::HistoryPrevious => global.set_history_previous(first),
        ShortcutAction::HistoryNext => global.set_history_next(first),
        ShortcutAction::SaveValue => {
            global.set_save_value(first);
            global.set_save_value_2(second);
        }
    }
}

/// Gives the UI each action's keys and the rows of the Key bindings page; again after a
/// change, or a new language (the rows name the keys in it).
fn row(action: ShortcutAction, title: &str, labels: Vec<String>, custom: bool, group_title: &str) -> ShortcutRow {
    let labels: Vec<SharedString> = labels.into_iter().map(SharedString::from).collect();
    ShortcutRow { action, title: title.into(), keys: ModelRc::new(VecModel::from(labels)), custom, group_title: group_title.into() }
}

/// Gives the UI each action's keys and the rows of the Key bindings page that the settings'
/// search finds; again after a change, a search or a new language (the rows are in it).
pub fn refresh(window: &MainWindow, state: &AppState) {
    let global = window.global::<Shortcuts>();
    let filter = FILTER.with_borrow(Filter::clone);
    let found = |words: &[&str]| crate::settings::found(&filter.query, &words.join(" "));
    let mut rows = Vec::with_capacity(SPECS.len());
    let mut previous = None;
    for spec in &SPECS {
        let (keys, custom) = keys_of(state, spec);
        let labels: Vec<String> = keys.iter().map(|keys| Chord::of(keys).label()).collect();
        let (title, group) = (title(spec.action), spec.scope.title());
        if found(&[&filter.page, &group, &title, &labels.join(" ")]) {
            let group_title = if previous == Some(spec.scope) { "" } else { &*group };
            rows.push(row(spec.action, &title, labels, custom, group_title));
            previous = Some(spec.scope);
        }
        let mut keys = keys.into_iter();
        set_keys(&global, spec.action, keys.next().unwrap_or_default(), keys.next().unwrap_or_default());
    }
    global.set_rows(ModelRc::new(VecModel::from(rows)));
    let standard: Vec<ShortcutRow> = standard_rows()
        .into_iter()
        .filter(|(title, labels)| found(&[&filter.page, &filter.standard, title, &labels.join(" ")]))
        .map(|(title, labels)| row(ShortcutAction::default(), &title, labels, false, ""))
        .collect();
    global.set_standard(ModelRc::new(VecModel::from(standard)));
}

pub fn init(window: &MainWindow, state: Arc<AppState>) {
    refresh(window, &state);
    let global = window.global::<Shortcuts>();

    global.on_matches(|keys, text, control, shift, alt, meta| Chord::of(&keys).matches(&text, control, shift, alt, meta));

    global.on_filter_changed({
        let weak = window.as_weak();
        let state = state.clone();
        move |query, page, standard| {
            let Some(window) = weak.upgrade() else { return };
            let filter = Filter { query: query.into(), page: page.into(), standard: standard.into() };
            FILTER.with_borrow_mut(|current| *current = filter);
            refresh(&window, &state);
        }
    });

    // the keys pressed for an action on the Key bindings page: bound to it, or refused
    global.on_recorded({
        let weak = window.as_weak();
        let state = state.clone();
        move |action, text, control, shift, alt, meta| {
            let Some(window) = weak.upgrade() else { return };
            let Some(keys) = pressed_keys(&text, control, shift, alt, meta) else { return };
            let result = bind(&state, spec(action), keys);
            window.global::<Shortcuts>().set_recording(false);
            window.global::<Preferences>().set_error(result.err().map(|why| why.message()).unwrap_or_default().into());
            refresh(&window, &state);
        }
    });

    global.on_reset({
        let weak = window.as_weak();
        move |action| {
            let Some(window) = weak.upgrade() else { return };
            let result = state.settings.lock().set_shortcut(spec(action).code, None);
            window.global::<Preferences>().set_error(result.err().map(|e| e.localized()).unwrap_or_default().into());
            refresh(&window, &state);
        }
    });
}
