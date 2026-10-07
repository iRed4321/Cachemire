//! Where a tab's key panel was scrolled to, and which value-list rows were
//! left open — both saved per tab when it's left and handed back when it's
//! loaded again (see `KeyScroll` in key-detail.slint).

use rustc_hash::FxHashSet;

use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};

use crate::{KeyPanel, KeyScroll, MainWindow};

/// Where a tab's key panel was scrolled to (logical px, 0 or negative): the
/// value list vertically, the table both ways. Saved per tab when it is left
/// and handed back when it is loaded again, so a tab keeps its place.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ScrollPos {
    pub all_y: f32,
    pub table_x: f32,
    pub table_y: f32,
}

/// What the views report as their current scroll position (see `KeyScroll`
/// in key-detail.slint).
pub fn current_scroll(window: &MainWindow) -> ScrollPos {
    let scroll = window.global::<KeyScroll>();
    ScrollPos { all_y: scroll.get_all_y(), table_x: scroll.get_table_x(), table_y: scroll.get_table_y() }
}

/// Makes the value list jump to `pos` once the rows set next have arrived.
pub(super) fn arm_all_scroll(window: &MainWindow, pos: ScrollPos) {
    let scroll = window.global::<KeyScroll>();
    scroll.set_restore_all_y(pos.all_y);
    scroll.set_restore_all_pending(true);
}

/// Makes the table jump to `pos` once the rows set next have arrived.
pub(super) fn arm_table_scroll(window: &MainWindow, pos: ScrollPos) {
    let scroll = window.global::<KeyScroll>();
    scroll.set_restore_table_x(pos.table_x);
    scroll.set_restore_table_y(pos.table_y);
    scroll.set_restore_table_pending(true);
}

/// Drops a jump that no view took (there was nothing to show it in), so it
/// can't fire on unrelated content later.
pub(super) fn disarm_scroll(window: &MainWindow) {
    let scroll = window.global::<KeyScroll>();
    scroll.set_restore_all_pending(false);
    scroll.set_restore_table_pending(false);
}

/// The value-list fields currently open, for saving into a tab's view state
/// when it's left (see `KeyScroll.all-values-expanded`).
pub fn current_expanded_fields(window: &MainWindow) -> FxHashSet<String> {
    window.global::<KeyScroll>().get_all_values_expanded().iter().map(|f| f.to_string()).collect()
}

/// Writes `fields` into the window ahead of loading a tab, so the rows it
/// builds start with these already open (see `build_all_values_table`).
pub fn restore_expanded_fields(window: &MainWindow, fields: &FxHashSet<String>) {
    let model: Vec<SharedString> = fields.iter().map(SharedString::from).collect();
    window.global::<KeyScroll>().set_all_values_expanded(ModelRc::new(VecModel::from(model)));
}

/// Adds or removes `field` in the live set of open value-list fields, in step with a
/// row opening/folding (`AllValuesRow.expanded-changed`). Factored out of `init`'s
/// callback so tests can exercise the same logic.
pub(super) fn set_field_expanded(window: &MainWindow, field: &str, expanded: bool) {
    let scroll = window.global::<KeyScroll>();
    let mut fields: Vec<SharedString> = scroll.get_all_values_expanded().iter().collect();
    if expanded {
        if !fields.iter().any(|f| f.as_str() == field) {
            fields.push(field.into());
        }
    } else {
        fields.retain(|f| f.as_str() != field);
    }
    scroll.set_all_values_expanded(ModelRc::new(VecModel::from(fields)));
}

/// Makes each row's `initial_expanded` match the live set of open fields, for the rows
/// about to be built again (the list view coming back after the table view).
pub(super) fn sync_rows_expansion(window: &MainWindow) {
    let open = current_expanded_fields(window);
    let rows = window.global::<KeyPanel>().get_all_values_rows();
    for (i, mut row) in rows.iter().enumerate() {
        let wanted = open.contains(row.field.as_str());
        if row.initial_expanded != wanted {
            row.initial_expanded = wanted;
            rows.set_row_data(i, row);
        }
    }
}
