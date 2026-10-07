//! The value list: filter highlights, open rows (their fixed header, pretty
//! text, reopening) and its scroll position across tab switches.

use super::*;

/// The text a value filter matches gets a filled background (the amber
/// highlight), not just another text color.
#[test]
#[ignore = "slow: renders the real window; run with --ignored"]
fn value_filter_matches_are_drawn_on_a_filled_background() {
    let mut h = Harness::new();
    // the value filter matches a field's raw text, which the test helper leaves empty
    let fields: Fields = hash_fields(20, "user").iter().map(|f| Arc::new(field::Field::from_value(f.name.clone(), f.json.clone().unwrap()))).collect();
    let amber = |h: &Harness| {
        h.buffer
            .iter()
            .filter(|p| (p.0 >> 11) >= 30 && (44..=50).contains(&((p.0 >> 5) & 63)) && (p.0 & 31) <= 8)
            .count()
    };
    h.frame();
    let before = amber(&h);
    let filter = SearchFilter::new(vec![SearchTerm { text: "person".into(), scope: SearchScope::Value }], false);
    let (chars, rows) = build_all_values_table(&fields, &filter, &FxHashSet::default());
    h.window.global::<KeyPanel>().set_all_values_field_chars(chars);
    h.window.global::<KeyPanel>().set_all_values_rows(ModelRc::new(VecModel::from(rows)));
    h.settle();
    let after = amber(&h);
    // "person" is 6 glyphs wide on every visible row: a filled box is hundreds of pixels
    assert!(after > before + 300, "amber pixels: {before} -> {after}");
}

/// An open row's header stays put, fixed above the scrolling area, while its
/// text scrolls under it; clicking it folds the row, whether it is the row's own
/// or the fixed one.
#[test]
#[ignore = "slow: renders the real window; run with --ignored"]
fn the_header_of_an_open_row_stays_fixed_while_its_text_scrolls() {
    let mut h = Harness::new();
    h.window.global::<KeyPanel>().set_view(KeyView::Fields);
    let big: serde_json::Value = json!({"items": (0..300).map(|i| json!({"id": i, "name": format!("item number {i}")})).collect::<Vec<_>>()});
    let fields: Fields = (0..3).map(|i| Arc::new(field::Field::from_value(format!("row{i}"), big.clone()))).collect();
    h.window.global::<KeyPanel>().on_request_pretty_value({
        let fields = fields.clone();
        move |name, _q, _c, _e| {
            let f = fields.iter().find(|f| f.name == name.as_str()).unwrap();
            highlight::pretty_styled_text_with_highlights(&pretty_text::pretty_paragraphs_for(&f.value()), &[], false, "", 0)
        }
    });
    h.window.global::<KeyPanel>().on_pretty_is_ready(|_, _| true);
    let folded = Rc::new(RefCell::new(Vec::<(String, bool)>::new()));
    h.window.global::<KeyPanel>().on_field_expanded_changed({
        let (weak, folded) = (h.window.as_weak(), folded.clone());
        move |field, expanded| {
            folded.borrow_mut().push((field.to_string(), expanded));
            if let Some(window) = weak.upgrade() {
                set_field_expanded(&window, &field, expanded);
            }
        }
    });
    show_values(&h.window, &fields, None, &FxHashSet::from_iter(["row1".to_string()]));
    h.settle();

    // the badge's box (bg-elevated) in the header's band, under the key's header: its top edge
    let elevated = { let (r, g, b) = (0x22_u16, 0x22_u16, 0x26_u16); ((r >> 3) << 11) | ((g >> 2) << 5) | (b >> 3) };
    let badge_top = |h: &Harness| -> Option<usize> {
        (LIST_TOP_Y..HEIGHT).find(|y| h.buffer[y * WIDTH + 380].0 == elevated)
    };
    let scroll_to = |h: &mut Harness, y: f32| {
        arm_all_scroll(&h.window, ScrollPos { all_y: y, ..Default::default() });
        h.settle();
    };
    let mut tops = Vec::new();
    for y in [-900.0, -1100.0, -1337.5, -1600.25, -2000.0] {
        scroll_to(&mut h, y);
        assert!((current_scroll(&h.window).all_y - y).abs() < 1.0, "scrolled to {y}: {:?}", current_scroll(&h.window));
        tops.push(badge_top(&h));
    }
    assert!(tops.iter().all(|t| t.is_some()), "the header is on screen at every position: {tops:?}");
    assert!(tops.windows(2).all(|w| w[0] == w[1]), "and never moves: {tops:?}");

    // its find box works from there: typing a query and Enter searches this row's value
    let top = tops[0].unwrap() as f32;
    let searched = Rc::new(RefCell::new(Vec::<(String, String)>::new()));
    h.window.global::<KeyPanel>().on_find_in_value({
        let searched = searched.clone();
        move |field, query, _, _| {
            searched.borrow_mut().push((field.to_string(), query.to_string()));
            crate::FindResult { count: 3, index: 0, line: 0, label: "1/3".into() }
        }
    });
    let type_key = |h: &Harness, text: slint::SharedString| {
        h.window.window().dispatch_event(WindowEvent::KeyPressed { text: text.clone() });
        h.window.window().dispatch_event(WindowEvent::KeyReleased { text });
    };
    'find: for x in (470..670).step_by(10) {
        h.click(x as f32, top + 14.0);
        type_key(&h, "z".into());
        type_key(&h, slint::platform::Key::Return.into());
        h.frame();
        if !searched.borrow().is_empty() {
            break 'find;
        }
    }
    assert_eq!(searched.borrow().first(), Some(&("row1".to_string(), "z".to_string())), "{:?}", searched.borrow());

    // a click on the fixed header folds its row (the search scrolled to its match)
    scroll_to(&mut h, -2000.0);
    h.click(380.0, top + 6.0);
    h.settle();
    assert_eq!(folded.borrow().last(), Some(&("row1".to_string(), false)), "{:?}", folded.borrow());
}

/// A row asked for (a click on a hit of a search tab) is built, opened and
/// scrolled to, however far down the list it is.
#[test]
#[ignore = "slow: renders the real window; run with --ignored"]
fn a_revealed_row_is_opened_and_scrolled_to() {
    let mut h = Harness::new();
    h.window.global::<KeyPanel>().set_view(KeyView::Fields);
    let fields = hash_fields(400, "long");
    h.window.global::<KeyPanel>().on_request_pretty_value({
        let fields = fields.clone();
        move |name, _q, _c, _e| {
            let f = fields.iter().find(|f| f.name == name.as_str()).unwrap();
            highlight::pretty_styled_text_with_highlights(&pretty_text::pretty_paragraphs_for(&f.value()), &[], false, "", 0)
        }
    });
    h.window.global::<KeyPanel>().on_pretty_is_ready(|_, _| true);
    h.window.global::<KeyPanel>().on_field_expanded_changed({
        let weak = h.window.as_weak();
        move |field, expanded| {
            if let Some(window) = weak.upgrade() {
                set_field_expanded(&window, &field, expanded);
            }
        }
    });

    // what tabs.rs and load_key do for a hit on `long:250`
    let target = "long:250";
    {
        let scroll = h.window.global::<crate::KeyScroll>();
        scroll.set_reveal_field(target.into());
        scroll.set_reveal_pending(true);
        scroll.set_reveal_index(fields.iter().position(|f| f.name == target).unwrap() as i32);
    }
    show_values(&h.window, &fields, None, &FxHashSet::from_iter([target.to_string()]));
    h.settle();
    h.settle();

    let y = current_scroll(&h.window).all_y;
    // 250 rows of about 27px sit above it
    assert!(y < -5000.0 && y > -9000.0, "scrolled down to the row: {y}");
    assert!(!h.window.global::<crate::KeyScroll>().get_reveal_pending(), "and the request is done");
    assert_eq!(current_expanded_fields(&h.window), FxHashSet::from_iter([target.to_string()]), "the row is open");
}

/// A long value list scrolled far down comes back to exactly where it was
/// on switching back, even though rows must first be created around that
/// spot — otherwise the Flickable would clamp to a momentarily shorter list.
#[test]
#[ignore = "slow: renders the real window; run with --ignored"]
fn value_list_scroll_restores_when_a_tab_is_switched_back_to() {
    let mut h = Harness::new();
    let (long, short) = (hash_fields(400, "long"), hash_fields(30, "short"));

    h.window.global::<KeyPanel>().set_view(KeyView::Fields);
    show_values(&h.window, &long, None, &FxHashSet::default());
    h.settle();
    wheel_down_to(&mut h, |w| current_scroll(w).all_y);
    let saved = current_scroll(&h.window);
    assert!(saved.all_y < -2700.0, "the wheel scrolled the list far down: {saved:?}");
    for _ in 0..2 {
        show_values(&h.window, &short, Some(ScrollPos::default()), &FxHashSet::default());
        h.settle();
        assert_eq!(current_scroll(&h.window).all_y, 0.0);
        show_values(&h.window, &long, Some(saved), &FxHashSet::default());
        h.settle();
        assert_eq!(current_scroll(&h.window).all_y, saved.all_y, "the list comes back to the same deep position, every time");
    }
}

/// An opened row is remembered too: expand one, leave the tab, show a
/// different key, then come back — the row reopens on its own (`changed
/// expanded` fires from its own `init`, not just the bookkeeping agreeing).
#[test]
#[ignore = "slow: renders the real window; run with --ignored"]
fn expanded_row_reopens_when_a_tab_is_switched_back_to() {
    let mut h = Harness::new();
    let (long, short) = (hash_fields(400, "long"), hash_fields(30, "short"));
    h.window.global::<KeyPanel>().set_view(KeyView::Fields);

    let seen = Rc::new(RefCell::new(Vec::<(String, bool)>::new()));
    h.window.global::<KeyPanel>().on_field_expanded_changed({
        let weak = h.window.as_weak();
        let seen = seen.clone();
        move |field, expanded| {
            seen.borrow_mut().push((field.to_string(), expanded));
            if let Some(window) = weak.upgrade() {
                set_field_expanded(&window, &field, expanded);
            }
        }
    });
    show_values(&h.window, &short, None, &FxHashSet::default());
    h.settle();
    assert!(current_expanded_fields(&h.window).is_empty(), "a fresh tab starts with nothing open");

    let target_field = short[0].name.clone();
    h.window.global::<KeyPanel>().invoke_field_expanded_changed(target_field.as_str().into(), true);
    assert_eq!(current_expanded_fields(&h.window), FxHashSet::from_iter([target_field.clone()]));
    let leaving_state = current_expanded_fields(&h.window);

    // a different key: nothing of it starts open
    show_values(&h.window, &long, None, &FxHashSet::default());
    h.settle();
    assert!(current_expanded_fields(&h.window).is_empty());
    seen.borrow_mut().clear();

    // checked after exactly one frame (row construction + init both happen
    // within it): the row must be *born* open via a binding, not flipped
    // imperatively, or it would paint collapsed for a frame then flash open
    show_values(&h.window, &short, None, &leaving_state);
    h.frame();
    assert!(
        seen.borrow().contains(&(target_field.clone(), true)),
        "the restored row reports itself open again on its own, in the very frame that builds it: {:?}",
        seen.borrow()
    );
    assert_eq!(current_expanded_fields(&h.window), leaving_state, "and the window agrees");
    h.settle();

    // folding it removes it from the set, the same way
    h.window.global::<KeyPanel>().invoke_field_expanded_changed(target_field.as_str().into(), false);
    assert!(current_expanded_fields(&h.window).is_empty());
}

/// Two rows opening together (as restoring a tab does) must both get their
/// pretty text: `pretty-is-ready` has each row ask about its own field, so
/// one shared "ready" pulse can wake several rows without racing.
#[test]
#[ignore = "slow: renders the real window; run with --ignored"]
fn two_rows_opening_together_both_get_their_pretty_text() {
    let mut h = Harness::new();
    h.window.global::<KeyPanel>().set_view(KeyView::Fields);

    let ready = Rc::new(RefCell::new(FxHashSet::<String>::default()));
    h.window.global::<KeyPanel>().on_pretty_is_ready({
        let ready = ready.clone();
        move |field, _| ready.borrow().contains(field.as_str())
    });
    let requested = Rc::new(RefCell::new(Vec::<String>::new()));
    h.window.global::<KeyPanel>().on_request_pretty_value_async({
        let requested = requested.clone();
        move |field, _| requested.borrow_mut().push(field.to_string())
    });
    let loaded_calls = Rc::new(RefCell::new(Vec::<String>::new()));
    h.window.global::<KeyPanel>().on_request_pretty_value({
        let loaded_calls = loaded_calls.clone();
        move |field, _query, _current, _| {
            loaded_calls.borrow_mut().push(field.to_string());
            StyledText::default()
        }
    });

    let two = vec![field("alpha", None), field("beta", None)];
    show_values(&h.window, &two, None, &FxHashSet::from_iter(["alpha".to_string(), "beta".to_string()]));
    h.settle();
    assert_eq!(requested.borrow().len(), 2, "both rows prefetch on opening: {:?}", requested.borrow());

    // both complete "at once": marked ready together, one shared pulse
    ready.borrow_mut().extend(["alpha".to_string(), "beta".to_string()]);
    h.window.global::<KeyPanel>().set_pretty_ready_tick(h.window.global::<KeyPanel>().get_pretty_ready_tick() + 1);
    h.settle();
    assert_eq!(
        {
            let mut got = loaded_calls.borrow().clone();
            got.sort();
            got
        },
        vec!["alpha".to_string(), "beta".to_string()],
        "one pulse wakes both rows, and each finds its own field ready"
    );

    // a field that isn't ready yet keeps waiting, not loaded early
    ready.borrow_mut().clear();
    loaded_calls.borrow_mut().clear();
    let three = vec![field("gamma", None), field("delta", None)];
    show_values(&h.window, &three, None, &FxHashSet::from_iter(["gamma".to_string(), "delta".to_string()]));
    h.settle();
    ready.borrow_mut().insert("gamma".to_string());
    h.window.global::<KeyPanel>().set_pretty_ready_tick(h.window.global::<KeyPanel>().get_pretty_ready_tick() + 1);
    h.settle();
    assert_eq!(loaded_calls.borrow().as_slice(), ["gamma".to_string()], "only the field that's actually ready loads");
}

/// A row born already knowing its field is cached (pre-warmed) gets that
/// answer synchronously, inside its own `init` — a property write made
/// during a row's own construction never fires that row's own `changed`.
#[test]
#[ignore = "slow: renders the real window; run with --ignored"]
fn a_row_born_with_its_field_already_cached_loads_immediately() {
    let mut h = Harness::new();
    h.window.global::<KeyPanel>().set_view(KeyView::Fields);

    let ready = Rc::new(RefCell::new(FxHashSet::<String>::default()));
    h.window.global::<KeyPanel>().on_pretty_is_ready({
        let ready = ready.clone();
        move |field, _| ready.borrow().contains(field.as_str())
    });
    h.window.global::<KeyPanel>().on_request_pretty_value_async({
        let ready = ready.clone();
        let weak = h.window.as_weak();
        move |field, _| {
            // mirrors on_request_pretty_value_async: a cache hit signals
            // readiness right there, synchronously, nested inside whatever
            // call stack asked for it
            if ready.borrow().contains(field.as_str())
                && let Some(window) = weak.upgrade()
            {
                window.global::<KeyPanel>().set_pretty_ready_tick(window.global::<KeyPanel>().get_pretty_ready_tick() + 1);
            }
        }
    });
    let loaded_calls = Rc::new(RefCell::new(Vec::<String>::new()));
    h.window.global::<KeyPanel>().on_request_pretty_value({
        let loaded_calls = loaded_calls.clone();
        move |field, _query, _current, _| {
            loaded_calls.borrow_mut().push(field.to_string());
            StyledText::default()
        }
    });

    let warm = vec![field("warm", None)];
    ready.borrow_mut().insert("warm".to_string());
    show_values(&h.window, &warm, None, &FxHashSet::from_iter(["warm".to_string()]));
    h.frame();
    assert_eq!(
        loaded_calls.borrow().as_slice(),
        ["warm".to_string()],
        "a row born with its field already cached loads in the very frame it's built, not stuck waiting on a tick it can never see itself cause: {:?}",
        loaded_calls.borrow()
    );
}
