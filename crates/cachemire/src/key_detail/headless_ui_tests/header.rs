//! The key's header: its name, the view switcher, the search bar and the
//! JSON filter's completion.

use super::*;

/// The key name in the header can be selected, and copied from the
/// right-click menu (StyledText leaves right-clicks to the TouchArea behind
/// it — see AllValuesRow's `row-area` for the same pattern).
#[test]
#[ignore = "slow: renders the real window; run with --ignored"]
fn header_key_name_can_be_selected_and_copied_via_right_click() {
    let mut h = Harness::new();
    table_ready(&mut h, &hash_fields(60, "user"));

    let copied = Rc::new(RefCell::new(Vec::<String>::new()));
    h.window.global::<KeyPanel>().on_copy_selection({
        let copied = copied.clone();
        move |text| copied.borrow_mut().push(text.to_string())
    });
    let y = 102.0f32;
    let press = |h: &Harness, x: f32, button: PointerEventButton| {
        h.window.window().dispatch_event(WindowEvent::PointerMoved { position: LogicalPosition::new(x, y) });
        h.window.window().dispatch_event(WindowEvent::PointerPressed { position: LogicalPosition::new(x, y), button });
    };
    let release = |h: &Harness, x: f32, button: PointerEventButton| {
        h.window.window().dispatch_event(WindowEvent::PointerReleased { position: LogicalPosition::new(x, y), button });
    };
    // the name starts after the header's reveal button (22px and a 4px gap)
    let shift = 26.0;
    press(&h, 343.0 + shift, PointerEventButton::Left);
    h.window.window().dispatch_event(WindowEvent::PointerMoved { position: LogicalPosition::new(372.0 + shift, y) });
    release(&h, 372.0 + shift, PointerEventButton::Left);
    h.frame();
    press(&h, 355.0 + shift, PointerEventButton::Right);
    release(&h, 355.0 + shift, PointerEventButton::Right);
    h.frame();
    // the menu opened at the click, its "Copy selection" row just under it
    let item = LogicalPosition::new(355.0 + shift + 30.0, y + 16.0);
    h.window.window().dispatch_event(WindowEvent::PointerMoved { position: item });
    h.window.window().dispatch_event(WindowEvent::PointerPressed { position: item, button: PointerEventButton::Left });
    h.window.window().dispatch_event(WindowEvent::PointerReleased { position: item, button: PointerEventButton::Left });
    h.frame();
    assert_eq!(copied.borrow().len(), 1, "one Copy selection from the header");
    assert!(!copied.borrow()[0].is_empty() && "users".contains(copied.borrow()[0].as_str()), "the key's own text: {:?}", copied.borrow());
}

/// The switcher at the right of the key's header: its segments pick the view
/// and tell Rust (`key-view-changed`).
#[test]
#[ignore = "slow: renders the real window; run with --ignored"]
fn the_view_switcher_selects_the_view() {
    let mut h = Harness::new();
    h.frame();
    let changes = Rc::new(std::cell::Cell::new(0));
    h.window.global::<KeyPanel>().on_view_changed({
        let changes = changes.clone();
        move || changes.set(changes.get() + 1)
    });
    assert_eq!(h.window.global::<KeyPanel>().get_view(), KeyView::Fields, "the field/value view is the default");

    // the switcher's exact place isn't pinned down here: sweep its stretch of the
    // key's header, the first segment that changes the view is the one
    let sweep = |h: &mut Harness, want: KeyView| {
        for y in HEADER_ROW_Y.step_by(4) {
            for x in HEADER_SWITCHER_X.step_by(4) {
                h.click(x as f32, y as f32);
                if h.window.global::<KeyPanel>().get_view() == want {
                    return true;
                }
            }
        }
        false
    };
    assert!(sweep(&mut h, KeyView::Table), "a segment selects the table");
    assert_eq!(changes.get(), 1);
    assert!(sweep(&mut h, KeyView::Fields), "and one selects the field/value view again");
    assert_eq!(changes.get(), 2);
}

/// The search bar above the filter bar: the scope switch and case toggle are plain
/// properties, Enter turns the typed text into badges via Rust, and a badge's ×
/// asks Rust to drop it.
#[test]
#[ignore = "slow: renders the real window; run with --ignored"]
fn the_search_bar_wires_its_scope_case_toggle_and_badges() {
    let mut h = Harness::new();
    h.frame();
    // the search bar is in the "More" section
    h.open_more();
    assert_eq!(h.window.global::<KeyPanel>().get_filter_scope(), SearchScope::Everywhere, "everywhere is the default scope");

    // sweep the search bar's row for the segment that picks each scope —
    // its exact place isn't pinned down here, just that clicking it works
    let sweep_scope = |h: &mut Harness, want: SearchScope| {
        for y in MORE_ROW_Y.step_by(4) {
            for x in (291..560).step_by(4) {
                h.click(x as f32, y as f32);
                if h.window.global::<KeyPanel>().get_filter_scope() == want {
                    return true;
                }
            }
        }
        false
    };
    assert!(sweep_scope(&mut h, SearchScope::Fields), "a segment picks Fields");
    assert!(sweep_scope(&mut h, SearchScope::Value), "a segment picks Value");
    assert!(sweep_scope(&mut h, SearchScope::Everywhere), "and one picks Everywhere again");

    // the case-sensitivity toggle: a plain property, plus a callback so Rust
    // can re-filter under it
    let case_changes = Rc::new(std::cell::Cell::new(0));
    h.window.global::<KeyPanel>().on_filter_case_sensitivity_changed({
        let case_changes = case_changes.clone();
        move || case_changes.set(case_changes.get() + 1)
    });
    assert!(!h.window.global::<KeyPanel>().get_filter_case_sensitive());
    let mut toggle_row_y = None;
    'toggle: for y in MORE_ROW_Y.step_by(4) {
        for x in (291..900).step_by(4) {
            h.click(x as f32, y as f32);
            if h.window.global::<KeyPanel>().get_filter_case_sensitive() {
                toggle_row_y = Some(y as f32);
                break 'toggle;
            }
        }
    }
    let row_y = toggle_row_y.expect("a click flips case-sensitivity");
    assert_eq!(case_changes.get(), 1, "and tells Rust once");

    // Enter in the box: Rust reads it from here (the callback carries nothing)
    let submits = Rc::new(std::cell::Cell::new(0));
    h.window.global::<KeyPanel>().on_filter_query_submitted({
        let submits = submits.clone();
        move || submits.set(submits.get() + 1)
    });
    // click into the box first (same row as the toggle, to its right so it
    // lands past the switch and the toggle), so the Enter below reaches it
    h.click(SEARCH_BOX_X, row_y);
    h.window.global::<KeyPanel>().set_filter_query("needle".into());
    h.window.window().dispatch_event(WindowEvent::KeyPressed { text: slint::platform::Key::Return.into() });
    h.window.window().dispatch_event(WindowEvent::KeyReleased { text: slint::platform::Key::Return.into() });
    assert_eq!(submits.get(), 1, "the box tells Rust rather than adding the badge itself");

    // a badge Rust put in filter-badges (as if that submit had gone through)
    // renders in the header, tinted by its scope, and its × asks Rust to
    // drop exactly that one
    let removed = Rc::new(RefCell::new(Vec::<(String, SearchScope)>::new()));
    h.window.global::<KeyPanel>().on_filter_badge_removed({
        let removed = removed.clone();
        move |text, scope| removed.borrow_mut().push((text.to_string(), scope))
    });
    h.window.global::<KeyPanel>().set_filter_badges(ModelRc::new(VecModel::from(vec![
        crate::BadgeData { text: "needle".into(), scope: SearchScope::Value },
        crate::BadgeData { text: "prefix".into(), scope: SearchScope::Fields },
    ])));
    h.settle();
    // the Value badge's text and × are drawn solid accent-purple; only the key's
    // header is checked, since the scope switch's chosen "Value" segment below is
    // purple too
    let purple = { let (r, g, b) = (0xb3_u16, 0x89_u16, 0xf9_u16); ((r >> 3) << 11) | ((g >> 2) << 5) | (b >> 3) };
    let mut min_y = usize::MAX;
    let mut max_y = 0;
    let mut max_x = 0;
    for (i, p) in h.buffer.iter().enumerate() {
        if p.0 == purple && i / WIDTH < MORE_ROW_Y.start as usize {
            let (x, y) = (i % WIDTH, i / WIDTH);
            min_y = min_y.min(y);
            max_y = max_y.max(y);
            max_x = max_x.max(x);
        }
    }
    assert!(max_y > 0, "the Value badge's tint shows in the header");

    // its × sits at the pill's own right edge
    h.click(max_x as f32 - 3.0, ((min_y + max_y) / 2) as f32);
    assert_eq!(removed.borrow().as_slice(), [("needle".to_string(), SearchScope::Value)], "the badge's own × asked to drop exactly it");
}

/// Typing in the filter bar: a dot lists the keys under the path, typing
/// narrows the list, the arrows choose and Enter writes the choice.
#[test]
#[ignore = "slow: renders the real window; run with --ignored"]
fn the_filter_completes_paths_from_the_loaded_values() {
    let mut h = Harness::new();
    h.frame();
    let fields = hash_fields(3, "user");
    let suggester = Rc::new(suggest::Suggester::default());
    h.window.global::<KeyPanel>().on_suggest({
        let (fields, suggester, weak) = (fields.clone(), suggester.clone(), h.window.as_weak());
        move |text, cursor| {
            let completion = suggester.complete(&fields, &text, cursor as usize);
            let rows: Vec<SuggestionRow> = completion
                .iter()
                .flat_map(|c| &c.items)
                .map(|i| SuggestionRow { before: i.key[..i.matched.0].into(), matched: i.key[i.matched.0..i.matched.1].into(), after: i.key[i.matched.1..].into() })
                .collect();
            let any = !rows.is_empty();
            weak.upgrade().unwrap().global::<KeyPanel>().set_suggestions(ModelRc::new(VecModel::from(rows)));
            any
        }
    });
    h.window.global::<KeyPanel>().on_complete_suggestion({
        let (fields, suggester) = (fields.clone(), suggester.clone());
        move |text, cursor, index| match suggester.complete(&fields, &text, cursor as usize).and_then(|c| c.accept(&text, index as usize)) {
            Some(edit) => CompletionEdit { text: edit.text.into(), cursor: edit.cursor as i32 },
            None => CompletionEdit { text, cursor: -1 },
        }
    });

    // the JSON filter is in the "More" section: open it, and click into its box
    h.open_more();
    h.click(JSON_FILTER_X, (MORE_ROW_Y.start + MORE_ROW_Y.end) as f32 / 2.0);
    let press = |h: &Harness, key: slint::platform::Key| {
        let text: slint::SharedString = key.into();
        h.window.window().dispatch_event(WindowEvent::KeyPressed { text: text.clone() });
        h.window.window().dispatch_event(WindowEvent::KeyReleased { text });
    };
    let type_text = |h: &Harness, text: &str| {
        for c in text.chars() {
            let text: slint::SharedString = c.to_string().into();
            h.window.window().dispatch_event(WindowEvent::KeyPressed { text: text.clone() });
            h.window.window().dispatch_event(WindowEvent::KeyReleased { text });
        }
    };
    let shown = |h: &Harness| -> Vec<String> {
        h.window.global::<KeyPanel>().get_suggestions().iter().map(|r| format!("{}{}{}", r.before, r.matched, r.after)).collect()
    };

    type_text(&h, "address.");
    h.frame();
    assert_eq!(h.window.global::<KeyPanel>().get_json_filter(), "address.");
    assert_eq!(shown(&h), ["*", "city", "zip", "geo"], "a dot lists what is under the path");

    type_text(&h, "z");
    h.frame();
    assert_eq!(shown(&h), ["zip"], "typing narrows it");
    assert_eq!(h.window.global::<KeyPanel>().get_suggestions().row_data(0).unwrap().matched, "z");

    // Enter with nothing chosen leaves the list alone and applies the filter as typed
    let applied = Rc::new(std::cell::Cell::new(0));
    h.window.global::<KeyPanel>().on_json_filter_submitted({
        let applied = applied.clone();
        move || applied.set(applied.get() + 1)
    });
    press(&h, slint::platform::Key::Return);
    assert_eq!((applied.get(), h.window.global::<KeyPanel>().get_json_filter().as_str()), (1, "address.z"));

    // the arrows choose, Enter writes the choice
    type_text(&h, "i");
    h.frame();
    press(&h, slint::platform::Key::DownArrow);
    press(&h, slint::platform::Key::Return);
    h.frame();
    assert_eq!((applied.get(), h.window.global::<KeyPanel>().get_json_filter().as_str()), (1, "address.zip"), "the choice is written, not applied");

    // and the caret is after it: typing goes on from there
    type_text(&h, ".");
    h.frame();
    assert_eq!(h.window.global::<KeyPanel>().get_json_filter(), "address.zip.");
}
