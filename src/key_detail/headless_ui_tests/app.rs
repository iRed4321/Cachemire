//! Around the key panel: translations, the title bar, the sidebar, search and
//! query tabs, saved queries, profile tints and text boxes' context menu.

use super::*;

/// The translations are bundled inside the executable: French can be
/// selected, a language it doesn't have can't.
#[test]
#[ignore = "slow: renders the real window; run with --ignored"]
fn bundled_translations_select_and_reject_languages() {
    let mut h = Harness::new();
    assert!(slint::select_bundled_translation("fr").is_ok());
    assert!(slint::select_bundled_translation("xx").is_err());
    slint::select_bundled_translation("en").unwrap();
    h.frame();
}

/// A click in the title bar does nothing; dragging from there moves the
/// window (asking the compositor to move it on the press itself would end
/// the move before the release ever reaches the app).
#[test]
#[ignore = "slow: renders the real window; run with --ignored"]
fn status_pill_click_does_nothing_but_drag_moves_the_window() {
    let mut h = Harness::new();
    table_ready(&mut h, &hash_fields(60, "user"));

    let moves = Rc::new(std::cell::Cell::new(0));
    h.window.on_request_drag_window({
        let moves = moves.clone();
        move || moves.set(moves.get() + 1)
    });
    let pill = |x: f32| LogicalPosition::new(x, 18.0);
    h.window.window().dispatch_event(WindowEvent::PointerMoved { position: pill(400.0) });
    h.window.window().dispatch_event(WindowEvent::PointerPressed { position: pill(400.0), button: PointerEventButton::Left });
    h.window.window().dispatch_event(WindowEvent::PointerReleased { position: pill(400.0), button: PointerEventButton::Left });
    assert_eq!(moves.get(), 0, "a click on the status pill doesn't move the window");
    // a press right after a click would be a double click, which never starts a drag
    wait_out_double_click(&mut h);
    h.window.window().dispatch_event(WindowEvent::PointerPressed { position: pill(400.0), button: PointerEventButton::Left });
    h.window.window().dispatch_event(WindowEvent::PointerMoved { position: pill(402.0) });
    assert_eq!(moves.get(), 0, "a couple of pixels isn't a drag");
    h.window.window().dispatch_event(WindowEvent::PointerMoved { position: pill(412.0) });
    assert_eq!(moves.get(), 1, "dragging the pill moves the window");
    h.window.window().dispatch_event(WindowEvent::PointerMoved { position: pill(430.0) });
    assert_eq!(moves.get(), 1, "once per drag");
    h.window.window().dispatch_event(WindowEvent::PointerReleased { position: pill(430.0), button: PointerEventButton::Left });
    // the empty part of the bar too
    let bar = |x: f32| LogicalPosition::new(x, 6.0);
    h.window.window().dispatch_event(WindowEvent::PointerMoved { position: bar(900.0) });
    h.window.window().dispatch_event(WindowEvent::PointerPressed { position: bar(900.0), button: PointerEventButton::Left });
    h.window.window().dispatch_event(WindowEvent::PointerReleased { position: bar(900.0), button: PointerEventButton::Left });
    assert_eq!(moves.get(), 1, "a click on the bar doesn't move it either");
    wait_out_double_click(&mut h);
    h.window.window().dispatch_event(WindowEvent::PointerPressed { position: bar(900.0), button: PointerEventButton::Left });
    h.window.window().dispatch_event(WindowEvent::PointerMoved { position: bar(920.0) });
    assert_eq!(moves.get(), 2);
    h.window.window().dispatch_event(WindowEvent::PointerReleased { position: bar(920.0), button: PointerEventButton::Left });
}

/// The left panel's tabs: Query shows the saved queries, whose box
/// filters them as you type, and Keyspace brings the tree back.
#[test]
#[ignore = "slow: renders the real window; run with --ignored"]
fn the_left_panel_switches_between_the_keyspace_and_the_saved_queries() {
    use crate::backend::connection_cache::ConnectionCache;
    use crate::backend::connection_store::ConnectionStore;
    use crate::backend::settings_store::SettingsStore;
    use crate::backend::state::AppState;
    use crate::SidebarView;
    use std::sync::atomic::AtomicU64;
    use std::sync::{Arc, Mutex};

    let dir = std::env::temp_dir().join(format!("cachemire-queries-panel-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let state = Arc::new(AppState {
        connections: Mutex::new(ConnectionStore::new(dir.join("databases.json"))),
        settings: Mutex::new(SettingsStore::new(dir.join("settings.json"))),
        history: Mutex::new(crate::backend::history_store::HistoryStore::new(dir.join("query_history.json"))),
        redis_connections: ConnectionCache::new(dir.join("known_hosts")),
        connect_attempt: AtomicU64::new(0),
    });
    {
        let mut settings = state.settings.lock().unwrap();
        settings.create_saved_query("Stale locks", "SELECT * FROM KEY 'lock:*'", "").unwrap();
        settings.create_saved_query("Today's orders", "SELECT * FROM KEY 'orders:*'", "").unwrap();
        settings.create_saved_query("Users by country", "SELECT * FROM KEY 'user:*'", "").unwrap();
        settings.create_saved_query("Queue backlog", "SELECT * FROM KEY 'queue:*'", "").unwrap();
    }

    let mut h = Harness::new();
    crate::queries::init(&h.window, state);
    h.frame();
    assert_eq!(h.window.get_sidebar_view(), SidebarView::Keyspace, "the keyspace is the default");
    let all = h.window.global::<SavedQueries>().get_global_queries().row_count();
    assert!(all > 3);

    // the tabs are at the panel's top: the second one is the Search/Query tab
    let sweep = |h: &mut Harness, want: SidebarView, xs: std::ops::Range<usize>| {
        for y in (30..150).step_by(4) {
            for x in xs.clone().step_by(6) {
                h.click(x as f32, y as f32);
                if h.window.get_sidebar_view() == want {
                    return Some((x as f32, y as f32));
                }
            }
        }
        None
    };
    let (tab_x, tab_y) = sweep(&mut h, SidebarView::Queries, 20..270).expect("the Search/Query tab");
    let (x, y) = sweep(&mut h, SidebarView::Keyspace, 20..110).expect("the Keyspace tab brings the tree back");
    assert!(x < 110.0);
    h.click(tab_x, tab_y);
    assert_eq!(h.window.get_sidebar_view(), SidebarView::Queries);

    // the filter box is a few rows below the tabs
    let _ = y;
    for dy in (40..160).step_by(6) {
        h.click(100.0, tab_y + dy as f32);
        for c in "lock".chars() {
            let text: slint::SharedString = c.to_string().into();
            h.window.window().dispatch_event(WindowEvent::KeyPressed { text: text.clone() });
            h.window.window().dispatch_event(WindowEvent::KeyReleased { text });
        }
        if h.window.global::<SavedQueries>().get_global_queries().row_count() < all {
            break;
        }
    }
    assert_eq!(h.window.global::<SavedQueries>().get_filter(), "lock");
    assert_eq!(h.window.global::<SavedQueries>().get_global_queries().row_count(), 1, "only the queries matching the filter stay");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A search tab shows its box and hits in the main area: the box reports what is
/// typed on Enter, the hits show their matches on a filled background, and a
/// running search draws its loading bar.
#[test]
#[ignore = "slow: renders the real window; run with --ignored"]
fn a_search_tab_shows_its_box_and_its_hits() {
    let mut h = Harness::new();
    h.window.global::<TabStrip>().set_tabs(ModelRc::new(VecModel::from(vec![crate::TabData {
        key: "\u{1}search:1".into(),
        active: true,
        label: "Search".into(),
        search: true,
        query: false,
    }])));
    h.window.global::<TabStrip>().set_active_is_search(true);
    h.frame();

    let submitted = Rc::new(RefCell::new(Vec::<String>::new()));
    h.window.global::<SearchTab>().on_submitted({
        let submitted = submitted.clone();
        move |query| submitted.borrow_mut().push(query.to_string())
    });
    let type_key = |h: &Harness, text: slint::SharedString| {
        h.window.window().dispatch_event(WindowEvent::KeyPressed { text: text.clone() });
        h.window.window().dispatch_event(WindowEvent::KeyReleased { text });
    };
    // the box is in the header, across the main area
    'find: for y in (60..170).step_by(6) {
        for x in (500..900).step_by(40) {
            h.click(x as f32, y as f32);
            for c in "needle".chars() {
                type_key(&h, c.to_string().into());
            }
            type_key(&h, slint::platform::Key::Return.into());
            if !submitted.borrow().is_empty() {
                break 'find;
            }
        }
    }
    assert_eq!(submitted.borrow().first().map(String::as_str), Some("needle"));

    h.window.global::<SearchTab>().set_searched(true);
    h.window.global::<SearchTab>().set_loading(true);
    h.window.global::<SearchTab>().set_status("searching... 500 keys scanned, 2 matches".into());
    let rows: Vec<crate::SearchRowData> = (0..30)
        .map(|i| crate::SearchRowData {
            key: search_plain_text(&format!("user:{i}"), "needle"),
            field: search_plain_text("bio", "needle"),
            value: search_value_text(&format!("{{\"note\": \"a needle in row {i}\"}}"), "needle"),
            key_name: format!("user:{i}").into(),
            target_field: "bio".into(),
        })
        .collect();
    h.window.global::<SearchTab>().set_rows(ModelRc::new(VecModel::from(rows)));
    h.settle();
    // a click on a hit opens its key, at the field of the hit
    let opened = Rc::new(RefCell::new(Vec::<(String, String)>::new()));
    h.window.global::<SearchTab>().on_hit_opened({
        let opened = opened.clone();
        move |key, field| opened.borrow_mut().push((key.to_string(), field.to_string()))
    });
    'hit: for y in (200..500).step_by(7) {
        h.click(900.0, y as f32);
        h.frame();
        if !opened.borrow().is_empty() {
            break 'hit;
        }
    }
    let first = opened.borrow().first().cloned().expect("a click opens a hit");
    assert!(first.0.starts_with("user:") && first.1 == "bio", "{first:?}");

    // the find highlight (#ffe066) fills a box behind each match
    // the whole row is washed blue under the pointer, over its text too (bg-selected-soft #22344d)
    let wash = { let (r, g, b) = (0x22_u16, 0x34_u16, 0x4d_u16); ((r >> 3) << 11) | ((g >> 2) << 5) | (b >> 3) };
    let count = |h: &Harness| h.buffer.iter().filter(|p| p.0 == wash).count();
    // pointer away from the rows: the baseline has no wash
    h.window.window().dispatch_event(WindowEvent::PointerMoved { position: LogicalPosition::new(5.0, 5.0) });
    h.frame();
    let before = count(&h);
    // a row's value cell, where the text is: sweep the rows' band for one that washes
    let mut washed = false;
    'hover: for y in (200..500).step_by(7) {
        h.window.window().dispatch_event(WindowEvent::PointerMoved { position: LogicalPosition::new(900.0, y as f32) });
        h.frame();
        if count(&h) > before + 1000 {
            washed = true;
            break 'hover;
        }
    }
    assert!(washed, "hovering a row's text washes the whole row blue (wash pixels: {before} -> {})", count(&h));
    let yellow = h.buffer.iter().filter(|p| (p.0 >> 11) >= 30 && (54..=58).contains(&((p.0 >> 5) & 63)) && (p.0 & 31) <= 14).count();
    assert!(yellow > 300, "the matches are marked on a background: {yellow} pixels");
}

/// A connection profile tints the app's own background a little, whatever its
/// color, and nothing else: the panels keep theirs, and clearing it restores it.
#[test]
#[ignore = "slow: renders the real window; run with --ignored"]
fn a_connection_profile_tints_the_apps_background_and_its_chrome_border() {
    let h = Harness::new();
    let theme = h.window.global::<crate::Theme>();
    let (base, panel, border) = (theme.get_bg_app(), theme.get_bg_panel(), theme.get_border_soft());
    assert_eq!(base, theme.get_bg_app_base(), "no profile in use: the plain background");
    assert_eq!(theme.get_chrome_border(), border, "no profile in use: the plain border");

    let channels = |c: slint::Color| [c.red() as i32, c.green() as i32, c.blue() as i32];
    for tint in [
        crate::ProfileTint::Red,
        crate::ProfileTint::Orange,
        crate::ProfileTint::Yellow,
        crate::ProfileTint::Green,
        crate::ProfileTint::Cyan,
        crate::ProfileTint::Blue,
        crate::ProfileTint::Purple,
        crate::ProfileTint::Pink,
    ] {
        theme.set_env_tint(tint);
        theme.set_env_tinted(true);
        let tinted = theme.get_bg_app();
        assert_ne!(tinted, base, "{tint:?} changes the background");
        assert_eq!(tinted.alpha(), 255, "{tint:?} stays opaque");
        let shift = channels(tinted).iter().zip(channels(base)).map(|(t, b)| (t - b).abs()).max().unwrap();
        assert!((25..=70).contains(&shift), "{tint:?} is a tint, not a fill: it moves a channel by {shift}");
        assert_eq!(theme.get_bg_panel(), panel, "the panels are not tinted");

        // the chrome border (the keyspace/key panels, the title bar's pills) tints
        // too, more than the background, since it's a thin line rather than a fill
        let tinted_border = theme.get_chrome_border();
        assert_ne!(tinted_border, border, "{tint:?} changes the chrome border");
        let border_shift = channels(tinted_border).iter().zip(channels(border)).map(|(t, b)| (t - b).abs()).max().unwrap();
        assert!(border_shift > shift, "{tint:?}'s border ({border_shift}) should tint more than the background ({shift})");
    }

    theme.set_env_tint(crate::ProfileTint::Red);
    theme.set_env_tinted(true);
    let [dr, dg, db] = [0, 1, 2].map(|i| channels(theme.get_bg_app())[i] - channels(base)[i]);
    assert!(dr > dg && dr > db, "red is the channel that grows most: {dr} {dg} {db}");

    theme.set_env_tinted(false);
    assert_eq!(theme.get_bg_app(), base);
    assert_eq!(theme.get_chrome_border(), border);
}

/// A running search has a pause button under its box, which asks Rust to pause or
/// resume it; one that isn't running has none.
#[test]
#[ignore = "slow: renders the real window; run with --ignored"]
fn a_running_search_has_a_pause_button_and_a_finished_one_has_none() {
    let mut h = Harness::new();
    h.window.global::<TabStrip>().set_tabs(ModelRc::new(VecModel::from(vec![crate::TabData {
        key: "\u{1}search:1".into(),
        active: true,
        label: "Search".into(),
        search: true,
        query: false,
    }])));
    h.window.global::<TabStrip>().set_active_is_search(true);
    h.window.global::<SearchTab>().set_searched(true);
    h.window.global::<SearchTab>().set_status("searching... 500 of 9000 keys scanned, 2 matches".into());
    h.frame();

    let toggles = Rc::new(std::cell::Cell::new(0));
    h.window.global::<SearchTab>().on_pause_toggled({
        let toggles = toggles.clone();
        move || toggles.set(toggles.get() + 1)
    });
    // the button is at the right end of the status line under the search box
    let sweep = |h: &mut Harness| {
        for y in (138..164).step_by(4) {
            for x in (900..1290).step_by(4) {
                h.click(x as f32, y as f32);
                if toggles.get() > 0 {
                    return true;
                }
            }
        }
        false
    };

    h.window.global::<SearchTab>().set_loading(false);
    h.frame();
    assert!(!sweep(&mut h), "no button once the search is over");

    h.window.global::<SearchTab>().set_loading(true);
    h.frame();
    assert!(sweep(&mut h), "a running search can be paused");
    assert_eq!(toggles.get(), 1);

    // paused, the same button resumes it
    h.window.global::<SearchTab>().set_paused(true);
    h.frame();
    toggles.set(0);
    assert!(sweep(&mut h), "and a paused one can be resumed");
}

/// A text box's right-click "Paste" is offered only when there's something to
/// paste (checked once, at the click — not kept live while the menu is open):
/// nothing new is drawn if the clipboard is empty, but a popup shows if it isn't.
#[test]
#[ignore = "slow: renders the real window; run with --ignored"]
fn right_click_offers_paste_only_when_the_clipboard_holds_text() {
    let mut h = Harness::new();
    h.frame();
    h.open_more();
    let pos = LogicalPosition::new(SEARCH_BOX_X, (MORE_ROW_Y.start + MORE_ROW_Y.end) as f32 / 2.0);
    let right_click = |h: &mut Harness| {
        h.window.window().dispatch_event(WindowEvent::PointerMoved { position: pos });
        h.window.window().dispatch_event(WindowEvent::PointerPressed { position: pos, button: PointerEventButton::Right });
        h.window.window().dispatch_event(WindowEvent::PointerReleased { position: pos, button: PointerEventButton::Right });
        h.settle();
    };

    h.window.on_clipboard_has_text(|| false);
    let before = h.buffer.clone();
    right_click(&mut h);
    assert_eq!(h.buffer, before, "nothing is drawn: there is nothing to paste");

    h.window.on_clipboard_has_text(|| true);
    right_click(&mut h);
    assert_ne!(h.buffer, before, "a popup is drawn: there is something to paste");
}

/// A query tab shows its box and, in the key panel under it, its result: a Run button of the
/// panel's header reports exactly what was typed, and the rows fill the panel's table.
#[test]
#[ignore = "slow: renders the real window; run with --ignored"]
fn a_query_tab_shows_its_box_and_its_result() {
    let mut h = Harness::new();
    h.window.global::<TabStrip>().set_tabs(ModelRc::new(VecModel::from(vec![crate::TabData {
        key: "\u{2}query:1".into(),
        active: true,
        label: "Query".into(),
        search: false,
        query: true,
    }])));
    h.window.global::<TabStrip>().set_active_is_query(true);
    let query = "FROM KEY 'orders' AS o\nJOIN KEY 'products' AS p ON o.productId = p.id\nSELECT o.id, p.label\nLIMIT 100";
    h.window.global::<QueryTab>().set_text(query.into());
    h.frame();

    let run_calls = Rc::new(RefCell::new(Vec::<String>::new()));
    h.window.global::<QueryTab>().on_run_requested({
        let run_calls = run_calls.clone();
        move |q, _, _, _| run_calls.borrow_mut().push(q.to_string())
    });
    // the Run buttons are in the results' header, under the query box
    'find: for y in (150..340).step_by(6) {
        for x in (330..900).step_by(10) {
            h.click(x as f32, y as f32);
            if !run_calls.borrow().is_empty() {
                break 'find;
            }
        }
    }
    assert_eq!(run_calls.borrow().first().map(String::as_str), Some(query), "Run reports exactly what was typed");

    // a result lands in the key panel like a key's fields do
    h.window.global::<QueryTab>().set_ran(true);
    h.window.global::<KeyPanel>().set_loaded(true);
    let rows = hash_fields(3, "row");
    h.window.global::<KeyPanel>().set_view(KeyView::Table);
    apply_table(&h.window, build_table(&rows, &[], false));
    h.settle();

    // the result is drawn as the key panel's table, one row per result row
    assert!(h.window.global::<KeyPanel>().get_table_columns().row_count() > 1 && h.window.global::<KeyPanel>().get_table_rows().row_count() == 3);
}

/// Saving a query from its tab's header puts it in the sidebar's Search/Query
/// list right away, and the row's own × removes it — the system this replaces
/// a placeholder list with (see queries.rs).
#[test]
#[ignore = "slow: renders the real window; run with --ignored"]
fn a_saved_query_appears_in_the_sidebar_and_its_own_x_removes_it() {
    use crate::backend::connection_cache::ConnectionCache;
    use crate::backend::connection_store::ConnectionStore;
    use crate::backend::settings_store::SettingsStore;
    use crate::backend::state::AppState;
    use std::sync::atomic::AtomicU64;
    use std::sync::{Arc, Mutex};

    let dir = std::env::temp_dir().join(format!("cachemire-saved-query-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let state = Arc::new(AppState {
        connections: Mutex::new(ConnectionStore::new(dir.join("databases.json"))),
        settings: Mutex::new(SettingsStore::new(dir.join("settings.json"))),
        history: Mutex::new(crate::backend::history_store::HistoryStore::new(dir.join("query_history.json"))),
        redis_connections: ConnectionCache::new(dir.join("known_hosts")),
        connect_attempt: AtomicU64::new(0),
    });
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();

    let mut h = Harness::new();
    h.window.global::<TabStrip>().set_tabs(ModelRc::new(VecModel::from(vec![crate::TabData {
        key: "\u{2}query:1".into(),
        active: true,
        label: "Query".into(),
        search: false,
        query: true,
    }])));
    h.window.global::<TabStrip>().set_active_is_query(true);
    h.window.global::<QueryTab>().set_text("SELECT * FROM KEY 'demo'".into());
    crate::queries::init(&h.window, state.clone());
    let rt = runtime.handle().clone();
    let app = std::rc::Rc::new(crate::app::App {
        window: h.window.as_weak(),
        key_detail: crate::key_detail::init(&h.window, state.clone(), rt.clone()),
        keyspace: crate::keyspace::Keyspace::new(&h.window, state.clone(), rt.clone()),
        state: state.clone(),
        rt,
        tabs: Default::default(),
        searches: Default::default(),
        queries: Default::default(),
        transfer: Default::default(),
        cloud: Default::default(),
    });
    crate::redisql::init(&app, &h.window);
    h.frame();

    // the Save button is in the results' header, after the undo, redo and history buttons
    let type_key = |h: &Harness, text: slint::SharedString| {
        h.window.window().dispatch_event(WindowEvent::KeyPressed { text: text.clone() });
        h.window.window().dispatch_event(WindowEvent::KeyReleased { text });
    };
    // The header's buttons are found by clicking (cheap), starting with the leftmost of Run all and
    // Run; Save is the button just before them, so only a few positions are tried with the typing.
    let runs = Rc::new(std::cell::Cell::new(0));
    h.window.global::<QueryTab>().on_run_requested({
        let runs = runs.clone();
        move |_, _, _, _| runs.set(runs.get() + 1)
    });
    let mut run_all_at = None;
    'find_run: for y in (150..340).step_by(6) {
        for x in (330..900).step_by(10) {
            h.click(x as f32, y as f32);
            if runs.get() > 0 {
                run_all_at = Some((x, y + 6));
                break 'find_run;
            }
        }
    }
    let (run_all_x, row_y) = run_all_at.expect("the Run all button was found");
    'find_save: for x in (run_all_x - 90..run_all_x).step_by(8) {
        h.click(x as f32, row_y as f32);
        h.frame();
        for c in "Demo query".chars() {
            type_key(&h, c.to_string().into());
        }
        h.window.window().dispatch_event(WindowEvent::KeyPressed { text: slint::platform::Key::Return.into() });
        h.window.window().dispatch_event(WindowEvent::KeyReleased { text: slint::platform::Key::Return.into() });
        h.frame();
        if h.window.global::<SavedQueries>().get_global_queries().row_count() > 0 {
            break 'find_save;
        }
    }
    assert_eq!(h.window.global::<SavedQueries>().get_global_queries().row_count(), 1, "the Save button was found and the query saved");
    assert_eq!(h.window.global::<SavedQueries>().get_global_queries().row_data(0).unwrap().name, "Demo query");

    // its own × removes it: sweep the sidebar's Search/Query list for it
    h.window.set_sidebar_view(crate::SidebarView::Queries);
    h.settle();
    'find_delete: for y in (150..300).step_by(4) {
        for x in (250..310).step_by(4) {
            h.click(x as f32, y as f32);
            if h.window.global::<SavedQueries>().get_global_queries().row_count() == 0 {
                break 'find_delete;
            }
        }
    }
    assert_eq!(h.window.global::<SavedQueries>().get_global_queries().row_count(), 0, "the row's own x removed it");
    let _ = std::fs::remove_dir_all(&dir);
}
