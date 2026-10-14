//! The Query view of the left panel: the saved Redisql queries (the connection's
//! own, then the global ones) filtered by the box above them. Clicking one opens it
//! in a new tab and runs it (see `tabs.rs`); its × removes it.

use std::sync::Arc;

use slint::{ComponentHandle, ModelRc, VecModel};

use crate::backend::LockExt;
use crate::backend::state::AppState;
use crate::{MainWindow, SavedQueries, SavedQueryRowData};

/// A one-line preview of a query's text: its first non-blank line, cut short.
fn preview(text: &str) -> String {
    const MAX_CHARS: usize = 40;
    let first_line = text.lines().find(|line| !line.trim().is_empty()).unwrap_or_default().trim();
    if first_line.chars().count() > MAX_CHARS {
        format!("{}…", first_line.chars().take(MAX_CHARS).collect::<String>())
    } else {
        first_line.to_string()
    }
}

/// The saved queries of `connection_id` (empty: the global ones) whose name
/// or text holds `filter`, whatever the case.
fn matching(state: &AppState, filter: &str, connection_id: &str) -> Vec<SavedQueryRowData> {
    let filter = filter.trim().to_lowercase();
    state
        .settings
        .lock_recover()
        .saved_queries()
        .into_iter()
        .filter(|q| q.connection_id == connection_id)
        .filter(|q| filter.is_empty() || q.name.to_lowercase().contains(&filter) || q.text.to_lowercase().contains(&filter))
        .map(|q| SavedQueryRowData { id: q.id.into(), name: q.name.into(), detail: preview(&q.text).into() })
        .collect()
}

/// Shows the queries matching `filter`: the connected one's own, then the global ones.
fn show(window: &MainWindow, state: &AppState, filter: &str) {
    let connection = window.connection_id().to_string();
    let own = if connection.is_empty() { Vec::new() } else { matching(state, filter, &connection) };
    let any_shown = {
        let settings = state.settings.lock_recover();
        settings.saved_queries().iter().any(|q| q.connection_id.is_empty() || q.connection_id == connection)
    };
    window.global::<SavedQueries>().set_has_saved_queries(any_shown);
    window.global::<SavedQueries>().set_saved_queries(ModelRc::new(VecModel::from(own)));
    window.global::<SavedQueries>().set_global_queries(ModelRc::new(VecModel::from(matching(state, filter, ""))));
}

/// Puts the saved queries into the window's lists — call after any change to them.
pub fn refresh(window: &MainWindow, state: &AppState) {
    let text = window.global::<SavedQueries>().get_filter().to_string();
    show(window, state, &text);
}

pub fn init(window: &MainWindow, state: Arc<AppState>) {
    refresh(window, &state);

    window.global::<SavedQueries>().on_filter_changed({
        let weak = window.as_weak();
        let state = state.clone();
        move |text| {
            if let Some(window) = weak.upgrade() {
                show(&window, &state, &text);
            }
        }
    });

    // one click, no confirm: unlike a connection or a profile, losing a saved
    // query costs nothing but writing it again
    window.global::<SavedQueries>().on_deleted({
        let weak = window.as_weak();
        move |id| {
            let Some(window) = weak.upgrade() else { return };
            if state.settings.lock_recover().remove_saved_query(&id).is_ok() {
                refresh(&window, &state);
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_preview_is_the_first_non_blank_line_cut_short() {
        assert_eq!(preview("FROM KEY 'orders' AS o\nSELECT *"), "FROM KEY 'orders' AS o");
        assert_eq!(preview("\n\n  SELECT * FROM KEY 'x'  \nWHERE a = 1"), "SELECT * FROM KEY 'x'");
        assert_eq!(preview(""), "");
        let long = "a".repeat(60);
        let shown = preview(&long);
        assert_eq!(shown.chars().count(), 41, "40 chars plus the ellipsis");
        assert!(shown.ends_with('…'));
    }
}
