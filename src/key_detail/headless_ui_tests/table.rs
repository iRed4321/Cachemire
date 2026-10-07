//! The table view: ticking rows, selecting and copying cell text, resizing
//! columns, and keeping its scroll position across rebuilds and tab switches.

use super::*;

/// The checkbox column: clicking a row's cell ticks it, and the header's
/// dash control (there only while something is ticked) clears them all.
#[test]
#[ignore = "slow: renders the real window; run with --ignored"]
fn table_checkbox_selection_ticks_unticks_and_header_clears_all() {
    let mut h = Harness::new();
    table_ready(&mut h, &hash_fields(60, "user"));

    h.window.global::<KeyPanel>().on_table_clear_selection({
        let weak = h.window.as_weak();
        move || {
            let window = weak.upgrade().unwrap();
            reset_table_selection(&window, window.global::<KeyPanel>().get_table_rows().row_count());
        }
    });
    let ticked = |window: &MainWindow| window.global::<KeyPanel>().get_table_selected().iter().filter(|on| *on).count();
    // the table starts 10px in from x=331; its checkbox column is 34px wide; the
    // header is under the tab bar and the key header, the first row's checkbox
    // just below it
    h.click(358.0, TABLE_FIRST_ROW_Y);
    assert_eq!((h.window.global::<KeyPanel>().get_table_selected_count(), ticked(&h.window)), (1, 1), "a click on the first row's cell ticks it");
    assert_eq!(h.window.global::<KeyPanel>().get_table_selected().row_data(0), Some(true));
    h.click(358.0, TABLE_FIRST_ROW_Y);
    assert_eq!((h.window.global::<KeyPanel>().get_table_selected_count(), ticked(&h.window)), (0, 0), "and a second click unticks it");
    h.click(358.0, TABLE_FIRST_ROW_Y);
    h.frame();
    h.click(358.0, TABLE_HEADER_Y);
    assert_eq!((h.window.global::<KeyPanel>().get_table_selected_count(), ticked(&h.window)), (0, 0), "the header control clears every tick");
    h.click(358.0, TABLE_HEADER_Y);
    assert_eq!(h.window.global::<KeyPanel>().get_table_selected_count(), 0, "with nothing ticked there is no select-all");
}

/// Right-click on a selected cell's text offers "Copy selection"; a click
/// elsewhere drops the selection, and a right-click with nothing selected
/// offers nothing.
#[test]
#[ignore = "slow: renders the real window; run with --ignored"]
fn table_cell_right_click_copies_the_selected_text() {
    let mut h = Harness::new();
    table_ready(&mut h, &hash_fields(60, "user"));

    let copied = Rc::new(RefCell::new(Vec::<String>::new()));
    h.window.global::<KeyPanel>().on_copy_selection({
        let copied = copied.clone();
        move |text| copied.borrow_mut().push(text.to_string())
    });
    let widths: Vec<f32> = h.window.global::<KeyPanel>().get_table_col_widths().iter().collect();
    let x0 = 341.0 + 34.0 + widths[0] + widths[1];
    let y = TABLE_FIRST_ROW_Y;
    let at = |x: f32| LogicalPosition::new(x, y);
    h.window.window().dispatch_event(WindowEvent::PointerMoved { position: at(x0 + 12.0) });
    h.window.window().dispatch_event(WindowEvent::PointerPressed { position: at(x0 + 12.0), button: PointerEventButton::Left });
    h.window.window().dispatch_event(WindowEvent::PointerMoved { position: at(x0 + 60.0) });
    h.window.window().dispatch_event(WindowEvent::PointerReleased { position: at(x0 + 60.0), button: PointerEventButton::Left });
    h.frame();
    let right = |h: &Harness, x: f32, button: PointerEventButton| {
        h.window.window().dispatch_event(WindowEvent::PointerMoved { position: at(x) });
        h.window.window().dispatch_event(WindowEvent::PointerPressed { position: at(x), button });
        h.window.window().dispatch_event(WindowEvent::PointerReleased { position: at(x), button });
    };
    right(&h, x0 + 30.0, PointerEventButton::Right);
    h.frame();
    // the menu opened at the click: its "Copy selection" row sits just under it
    let menu_item = LogicalPosition::new(x0 + 60.0, y + 16.0);
    h.window.window().dispatch_event(WindowEvent::PointerMoved { position: menu_item });
    h.window.window().dispatch_event(WindowEvent::PointerPressed { position: menu_item, button: PointerEventButton::Left });
    h.window.window().dispatch_event(WindowEvent::PointerReleased { position: menu_item, button: PointerEventButton::Left });
    h.frame();
    assert_eq!(copied.borrow().len(), 1, "one Copy selection");
    assert!(!copied.borrow()[0].is_empty());
    // a click in another cell drops the selection, and a right-click with
    // nothing selected offers nothing
    h.click(x0 + 12.0, y + 81.0);
    right(&h, x0 + 30.0, PointerEventButton::Right);
    h.frame();
    h.window.window().dispatch_event(WindowEvent::PointerMoved { position: menu_item });
    h.window.window().dispatch_event(WindowEvent::PointerPressed { position: menu_item, button: PointerEventButton::Left });
    h.window.window().dispatch_event(WindowEvent::PointerReleased { position: menu_item, button: PointerEventButton::Left });
    h.frame();
    assert_eq!(copied.borrow().len(), 1, "no second copy without a selection");
}

/// A double click on a table cell selects all of its text; a single click
/// selects nothing.
#[test]
#[ignore = "slow: renders the real window; run with --ignored"]
fn table_cell_double_click_selects_the_whole_cell() {
    let mut h = Harness::new();
    table_ready(&mut h, &hash_fields(60, "user"));

    let copied = Rc::new(RefCell::new(Vec::<String>::new()));
    h.window.global::<KeyPanel>().on_copy_selection({
        let copied = copied.clone();
        move |text| copied.borrow_mut().push(text.to_string())
    });
    let widths: Vec<f32> = h.window.global::<KeyPanel>().get_table_col_widths().iter().collect();
    let x0 = 341.0 + 34.0 + widths[0] + widths[1];
    let y = TABLE_FIRST_ROW_Y;
    let at = |x: f32| LogicalPosition::new(x, y);
    let choose_copy = |h: &Harness| {
        h.window.window().dispatch_event(WindowEvent::PointerMoved { position: at(x0 + 30.0) });
        h.window.window().dispatch_event(WindowEvent::PointerPressed { position: at(x0 + 30.0), button: PointerEventButton::Right });
        h.window.window().dispatch_event(WindowEvent::PointerReleased { position: at(x0 + 30.0), button: PointerEventButton::Right });
        let menu_item = LogicalPosition::new(x0 + 60.0, y + 16.0);
        h.window.window().dispatch_event(WindowEvent::PointerMoved { position: menu_item });
        h.window.window().dispatch_event(WindowEvent::PointerPressed { position: menu_item, button: PointerEventButton::Left });
        h.window.window().dispatch_event(WindowEvent::PointerReleased { position: menu_item, button: PointerEventButton::Left });
    };

    h.click(x0 + 12.0, y);
    h.frame();
    choose_copy(&h);
    h.frame();
    assert!(copied.borrow().is_empty(), "a single click selects nothing");

    // long enough after that click for it not to pair with the next ones
    std::thread::sleep(std::time::Duration::from_millis(500));
    h.frame();
    h.click(x0 + 12.0, y);
    h.click(x0 + 12.0, y);
    h.frame();
    choose_copy(&h);
    h.frame();
    assert_eq!(copied.borrow().len(), 1, "one Copy selection");
    assert!(copied.borrow()[0].contains("person number 0"), "the whole cell, got {:?}", copied.borrow()[0]);
}

/// Wheel scrolling then a pointer sweep over the whole window — mostly a
/// "doesn't panic" check, since hit-testing under an unusual pointer path is
/// where binding loops tend to surface.
#[test]
#[ignore = "slow: renders the real window; run with --ignored"]
fn wheel_scroll_and_pointer_sweep_do_not_panic() {
    let mut h = Harness::new();
    table_ready(&mut h, &hash_fields(60, "user"));

    for step in 0..12 {
        h.window.window().dispatch_event(WindowEvent::PointerScrolled {
            position: LogicalPosition::new(700.0, 400.0),
            delta_x: if step % 4 == 0 { -40.0 } else { 0.0 },
            delta_y: -60.0,
        });
        if step % 4 == 3 {
            h.frame();
        }
    }
    for y in (0..HEIGHT).step_by(50) {
        for x in (0..WIDTH).step_by(70) {
            h.window.window().dispatch_event(WindowEvent::PointerMoved { position: LogicalPosition::new(x as f32, y as f32) });
        }
    }
    h.frame();
}

/// Dragging the right edge of a column title (its resize handle) widens that
/// column, and only that one; the table's reported total width follows.
#[test]
#[ignore = "slow: renders the real window; run with --ignored"]
fn dragging_the_column_edge_resizes_only_that_column() {
    let mut h = Harness::new();
    table_ready(&mut h, &hash_fields(60, "user"));

    // drag the "Field" title's 8px resize handle 40px wider — coordinates
    // match this fixed layout: 10px table inset, 34px checkbox column,
    // the header row, 72.5px starting column width
    let before: Vec<f32> = h.window.global::<KeyPanel>().get_table_col_widths().iter().collect();
    let (y, grab_x) = (TABLE_HEADER_Y, 331.0 + 10.0 + 34.0 + before[0] - 4.0);
    h.window.window().dispatch_event(WindowEvent::PointerMoved { position: LogicalPosition::new(grab_x, y) });
    h.window.window().dispatch_event(WindowEvent::PointerPressed { position: LogicalPosition::new(grab_x, y), button: PointerEventButton::Left });
    for step in 1..=40 {
        h.window.window().dispatch_event(WindowEvent::PointerMoved { position: LogicalPosition::new(grab_x + step as f32, y) });
    }
    h.window.window().dispatch_event(WindowEvent::PointerReleased { position: LogicalPosition::new(grab_x + 40.0, y), button: PointerEventButton::Left });
    h.frame();
    let after: Vec<f32> = h.window.global::<KeyPanel>().get_table_col_widths().iter().collect();
    assert!((after[0] - before[0] - 40.0).abs() < 2.0, "the Field column should be ~40px wider: {before:?} -> {after:?}");
    assert_eq!(before[1..], after[1..], "only the dragged column changes");
    assert!((h.window.global::<KeyPanel>().get_table_total_w() - after.iter().sum::<f32>()).abs() < 0.5, "total width must follow the columns");
}

/// The table jumps to a position armed for content that arrives next (a tab
/// coming back), even sideways; the strip under its left margin stays
/// background rather than showing column content run to the panel's edge.
#[test]
#[ignore = "slow: renders the real window; run with --ignored"]
fn table_scroll_restores_after_the_content_is_rebuilt() {
    let mut h = Harness::new();
    let fields = hash_fields(60, "user");
    table_ready(&mut h, &fields);

    // make the columns wide enough to scroll sideways
    let wide = vec![400.0f32; h.window.global::<KeyPanel>().get_table_columns().row_count()];
    h.window.global::<KeyPanel>().set_table_total_w(wide.iter().sum());
    h.window.global::<KeyPanel>().set_table_col_widths(ModelRc::new(VecModel::from(wide)));
    h.frame();

    let target = ScrollPos { all_y: -200.0, table_x: -30.0, table_y: -100.0 };
    arm_table_scroll(&h.window, target);
    apply_table(&h.window, build_table(&fields, &[], false));
    // (apply_table sizes the columns to the content again)
    let wide = vec![400.0f32; h.window.global::<KeyPanel>().get_table_columns().row_count()];
    h.window.global::<KeyPanel>().set_table_total_w(wide.iter().sum());
    h.window.global::<KeyPanel>().set_table_col_widths(ModelRc::new(VecModel::from(wide)));
    for _ in 0..3 {
        h.frame();
    }
    let restored = current_scroll(&h.window);
    assert_eq!((restored.table_x, restored.table_y), (-30.0, -100.0), "the table comes back to where the tab left it");
    // scrolled sideways, the columns slide under the table's left margin
    // instead of running up to the panel's edge: that strip stays background
    let bg = h.buffer[210 * WIDTH + 283];
    for y in 200..400 {
        for x in 283..290 {
            assert!(h.buffer[y * WIDTH + x] == bg, "content in the left margin at ({x}, {y})");
        }
    }
}

/// The table view scrolled far down comes back to exactly where it was when
/// the tab is switched back to — same guarantee as the value list, checked
/// through `apply_table` instead of `show_values`.
#[test]
#[ignore = "slow: renders the real window; run with --ignored"]
fn table_scroll_restores_when_a_tab_is_switched_back_to() {
    let mut h = Harness::new();
    let (long, short) = (hash_fields(400, "long"), hash_fields(30, "short"));

    h.window.global::<KeyPanel>().set_view(KeyView::Table);
    apply_table(&h.window, build_table(&long, &[], false));
    h.settle();
    wheel_down_to(&mut h, |w| current_scroll(w).table_y);
    let saved = current_scroll(&h.window);
    assert!(saved.table_y < -2700.0, "the wheel scrolled the table far down: {saved:?}");
    for _ in 0..2 {
        disarm_scroll(&h.window);
        arm_table_scroll(&h.window, ScrollPos::default());
        apply_table(&h.window, build_table(&short, &[], false));
        h.settle();
        assert_eq!(current_scroll(&h.window).table_y, 0.0);
        arm_table_scroll(&h.window, saved);
        apply_table(&h.window, build_table(&long, &[], false));
        h.settle();
        assert_eq!(current_scroll(&h.window).table_y, saved.table_y, "the table comes back to the same deep position, every time");
    }
}

/// Switching to the value list and back (which rebuilds the table) doesn't panic.
#[test]
#[ignore = "slow: renders the real window; run with --ignored"]
fn switching_views_back_and_forth_rebuilds_the_table_without_panicking() {
    let mut h = Harness::new();
    let fields = hash_fields(60, "user");
    table_ready(&mut h, &fields);

    h.window.global::<KeyPanel>().set_view(KeyView::Fields);
    clear_table(&h.window);
    h.frame();
    h.window.global::<KeyPanel>().set_view(KeyView::Table);
    apply_table(&h.window, build_table(&fields, &[], false));
    for _ in 0..3 {
        h.frame();
    }
}
