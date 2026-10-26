//! A row's "Try to load": reads a hash field's value left out of its page for its
//! size, and shows it with the row opened.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use rust_i18n::t;
use slint::{ComponentHandle, Model};

use crate::error::{Error, Localize};
use crate::{AllValuesRowData, KeyPanel, MainWindow};
use cachemire_core::explorer;
use cachemire_core::key_name::KeyName;

use super::field::Field;
use super::scroll::{current_expanded_fields, set_field_expanded};
use super::value_list::build_all_values_table;
use super::{Ctx, current_search_filter};

/// Changes the value-list row of `name` with `change`, if there is one.
fn update_row(window: &MainWindow, name: &str, change: impl FnOnce(&mut AllValuesRowData)) {
    let rows = window.global::<KeyPanel>().get_all_values_rows();
    if let Some((i, mut row)) = rows.iter().enumerate().find(|(_, row)| row.field == name) {
        change(&mut row);
        rows.set_row_data(i, row);
    }
}

/// Puts `new` in place of `old` in a list of fields, if `old` is in it.
fn replace(fields: &mut [Arc<Field>], old: &Arc<Field>, new: &Arc<Field>) {
    if let Some(slot) = fields.iter_mut().find(|f| Arc::ptr_eq(f, old)) {
        *slot = new.clone();
    }
}

impl Ctx {
    /// Reads the whole value of field `name`, left out of its page for its size, then
    /// shows it in place of its size (see `show_full_value`).
    pub(super) fn load_full_value(&self, name: &str) {
        let Some(window) = self.weak.upgrade() else { return };
        let shared = &self.shared;
        let (key, old) = {
            let content = shared.content.lock();
            let Some(old) = content.loaded.iter().find(|f| f.name == name && f.too_long.is_some()).cloned() else { return };
            (content.key.clone(), old)
        };
        update_row(&window, name, |row| row.loading = true);
        let connection = window.connection_id().to_string();
        let epoch = shared.view_epoch.load(Ordering::Relaxed);
        let ctx = self.clone();
        self.rt.spawn(async move {
            let read = explorer::hash_full_values(&ctx.state, None, &KeyName::from_id(&key), std::slice::from_ref(&old.name)).await;
            // parsing a big value would hold up the runtime's thread
            let name = old.name.clone();
            let read = tokio::task::spawn_blocking(move || -> Result<Option<Arc<Field>>, Error> {
                Ok(read?.into_iter().next().flatten().map(|raw| Arc::new(Field::hash_value(name, raw))))
            })
            .await
            .map_err(Error::from)
            .and_then(|read| read);
            let _ = slint::invoke_from_event_loop(move || ctx.show_full_value(&connection, &key, epoch, &old, read));
        });
    }

    /// Puts a field read in full in place of `old` (the one shown by its size), unless the
    /// key was loaded again since (`epoch`): its row opens.
    fn show_full_value(&self, connection: &str, key: &str, epoch: u64, old: &Arc<Field>, read: Result<Option<Arc<Field>>, Error>) {
        let shared = &self.shared;
        let Some(window) = self.weak.upgrade().filter(|_| shared.view_epoch.load(Ordering::Relaxed) == epoch) else { return };
        let new = match read {
            Ok(Some(new)) => new,
            Ok(None) => {
                update_row(&window, &old.name, |row| row.loading = false);
                crate::toast::show(&window, &t!("This field no longer exists"));
                return;
            }
            Err(e) => {
                update_row(&window, &old.name, |row| row.loading = false);
                crate::toast::show(&window, &t!("Loading failed: %{error}", error = e.localized()));
                return;
            }
        };
        if let Some(fetch) = shared.fetched.lock().get_mut(&(connection.to_string(), key.to_string())) {
            replace(&mut fetch.fields, old, &new);
        }
        {
            let mut content = shared.content.lock();
            replace(&mut content.loaded, old, &new);
            replace(&mut content.current, old, &new);
        }
        set_field_expanded(&window, &new.name, true);
        let (_, rows) = build_all_values_table(&[new.clone()], &current_search_filter(&window), &current_expanded_fields(&window));
        match rows.into_iter().next() {
            Some(row) => update_row(&window, &new.name, |shown| *shown = row),
            // the search bar's badges no longer match it: every row is built again under them
            None => self.on_search_changed(),
        }
        self.show_view(&window, None);
    }
}
