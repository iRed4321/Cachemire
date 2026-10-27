//! Writing from the tree's right-click menu: a new key (an empty hash, which Redis
//! holds once its first field is written), and renaming or deleting a key or a
//! folder (every key under it). The tree and the tabs follow what was done.

use std::cell::RefCell;
use std::rc::Rc;

use rust_i18n::t;
use slint::ComponentHandle;

use super::search::id_matches;
use super::{DELIMITER, LeafKind, Tree, focus_key, rebuild, show_additions};
use crate::app::App;
use crate::error::Localize;
use crate::{KeyNameAction, KeyNameDialog, KeyTree, MainWindow};
use cachemire_core::explorer::{self, FolderMove};
use cachemire_core::key_name::KeyName;

/// The folder `name` sits in (its own name for a folder), "" at the root.
fn folder_of(name: &str) -> &str {
    if name.ends_with(DELIMITER) {
        return name;
    }
    &name[..name.rfind(DELIMITER).map_or(0, |i| i + DELIMITER.len())]
}

/// Why `id` can't be a new key's name ("" when it can).
fn check_key_name(id: &str) -> String {
    if id.is_empty() {
        t!("A key needs a name").into_owned()
    } else if id.ends_with(DELIMITER) {
        t!("A key name can't end with ':'").into_owned()
    } else {
        String::new()
    }
}

/// Opens the name dialog: for a key in `row`'s folder, or on `row` to rename it.
fn open_dialog(window: &MainWindow, action: KeyNameAction, original: &str, text: &str) {
    let dialog = window.global::<KeyNameDialog>();
    dialog.set_action(action);
    dialog.set_original(KeyName::display_id(original).as_ref().into());
    dialog.set_text(KeyName::display_id(text).as_ref().into());
    dialog.set_error("".into());
    dialog.set_busy(false);
    dialog.set_open(true);
}

/// The dialog's outcome, once back on the UI thread: closed, or left open with why.
fn finish(window: &MainWindow, error: Option<String>) {
    let dialog = window.global::<KeyNameDialog>();
    dialog.set_busy(false);
    match error {
        Some(error) => dialog.set_error(error.into()),
        None => dialog.set_open(false),
    }
}

/// Adds key `id` to `tree`'s index (and the search's, when it matches) and puts it on screen.
fn place(window: &MainWindow, tree: &mut Tree, id: &str, leaf: LeafKind) {
    let mut added = Vec::new();
    tree.index.place(id, leaf, &mut added);
    match tree.search.as_mut() {
        None => show_additions(window, tree, added, false),
        Some(search) if id_matches(&search.pattern, id) => {
            let mut matched = Vec::new();
            search.index.place(id, leaf, &mut matched);
            show_additions(window, tree, matched, true);
        }
        Some(_) => {}
    }
}

/// A new key `id` (an empty hash): shown in the tree, its folders opened, and its tab opened.
fn created(app: &App, window: &MainWindow, id: String) {
    app.key_detail.mark_new(window.connection_id().as_str(), &id);
    place(window, &mut app.keyspace.0.tree.lock(), &id, LeafKind::Hash);
    focus_key(&app.keyspace.0, id.clone());
    crate::tabs::select_key(app, id);
}

/// Keys renamed, as (old id, new id): moved in the tree, their tabs following them.
fn moved(app: &App, window: &MainWindow, moves: Vec<(String, String)>) {
    let connection = window.connection_id().to_string();
    {
        let mut tree = app.keyspace.0.tree.lock();
        for (old, new) in &moves {
            let leaf = tree.index.leaf(old).unwrap_or(LeafKind::Other);
            tree.index.remove(old);
            if let Some(search) = tree.search.as_mut() {
                search.index.remove(old);
            }
            place(window, &mut tree, new, leaf);
        }
        rebuild(window, &mut tree);
    }
    for (old, new) in &moves {
        if app.key_detail.is_new(&connection, old) {
            app.key_detail.forget_new(&connection, |k| k == old);
            app.key_detail.mark_new(&connection, new);
        }
        if window.global::<KeyTree>().get_selected().as_str() == old {
            window.global::<KeyTree>().set_selected(new.as_str().into());
        }
    }
    crate::tabs::rename_keys(app, window, &moves);
}

/// Key or folder `row` was deleted: it leaves the tree, and its tabs close.
fn removed(app: &App, window: &MainWindow, row: &str) {
    let connection = window.connection_id().to_string();
    let folder = row.ends_with(DELIMITER);
    let gone = |k: &str| if folder { k.starts_with(row) } else { k == row };
    {
        let mut tree = app.keyspace.0.tree.lock();
        let tree = &mut *tree;
        for index in std::iter::once(&mut tree.index).chain(tree.search.as_mut().map(|s| &mut s.index)) {
            if folder {
                index.remove_folder(row);
            } else {
                index.remove(row);
            }
        }
        rebuild(window, tree);
    }
    app.key_detail.forget_new(&connection, gone);
    crate::tabs::close_keys(app, window, gone);
}

/// "Add key…": the dialog, its name starting with the folder of `row` ("" for none).
fn open_add(window: &MainWindow, target: &RefCell<String>, row: &str) {
    target.replace(String::new());
    open_dialog(window, KeyNameAction::Add, "", folder_of(row));
}

/// "Rename…" on key or folder `row`: the dialog, holding its name.
fn open_rename(window: &MainWindow, target: &RefCell<String>, row: &str) {
    target.replace(row.to_string());
    match row.strip_suffix(DELIMITER) {
        Some(folder) => open_dialog(window, KeyNameAction::RenameFolder, row, folder),
        None => open_dialog(window, KeyNameAction::RenameKey, row, row),
    }
}

/// The dialog's Create: once no key has the name, the key is made (in the app only).
fn submit_add(app: &App, window: &MainWindow, id: String) {
    let problem = check_key_name(&id);
    if !problem.is_empty() {
        return finish(window, Some(problem));
    }
    if app.keyspace.0.tree.lock().index.leaf(&id).is_some() {
        return finish(window, Some(t!("A key with this name already exists").into_owned()));
    }
    let state = app.state.clone();
    app.rt.spawn(async move {
        let exists = explorer::key_exists(&state, None, &KeyName::from_id(&id)).await;
        crate::app::later(move |app, window| match exists {
            Ok(true) => finish(window, Some(t!("A key with this name already exists").into_owned())),
            Ok(false) => {
                finish(window, None);
                created(app, window, id);
            }
            Err(e) => finish(window, Some(t!("Creating failed: %{error}", error = e.localized()).into_owned())),
        });
    });
}

/// The dialog's Rename on key `from`: renamed unless `to` is a key already.
fn submit_rename_key(app: &App, window: &MainWindow, from: String, to: String) {
    let problem = check_key_name(&to);
    if !problem.is_empty() {
        return finish(window, Some(problem));
    }
    if to == from {
        return finish(window, None);
    }
    // a new hash isn't in Redis yet: only its name in the app changes
    if app.key_detail.is_new(window.connection_id().as_str(), &from) {
        if app.keyspace.0.tree.lock().index.leaf(&to).is_some() {
            return finish(window, Some(t!("A key with this name already exists").into_owned()));
        }
        finish(window, None);
        return moved(app, window, vec![(from, to)]);
    }
    let state = app.state.clone();
    app.rt.spawn(async move {
        let renamed = explorer::rename_key(&state, None, &KeyName::from_id(&from), &KeyName::from_id(&to)).await;
        crate::app::later(move |app, window| match renamed {
            Ok(true) => {
                finish(window, None);
                crate::toast::show(window, &t!("Renamed"));
                moved(app, window, vec![(from, to)]);
            }
            Ok(false) => finish(window, Some(t!("A key with this name already exists").into_owned())),
            Err(e) => finish(window, Some(t!("Renaming failed: %{error}", error = e.localized()).into_owned())),
        });
    });
}

/// The dialog's Rename on folder `from`: every key under it moved under `to`, unless
/// one of their new names is a key already.
fn submit_rename_folder(app: &App, window: &MainWindow, from: String, text: &str) {
    let name = text.strip_suffix(DELIMITER).unwrap_or(text);
    if name.is_empty() {
        return finish(window, Some(t!("A folder needs a name").into_owned()));
    }
    let to = format!("{name}{DELIMITER}");
    if to == from {
        return finish(window, None);
    }
    let state = app.state.clone();
    app.rt.spawn(async move {
        let renamed = explorer::rename_folder(&state, None, &KeyName::from_id(&from), &KeyName::from_id(&to)).await;
        crate::app::later(move |app, window| match renamed {
            Ok(FolderMove::Moved(moves)) => {
                finish(window, None);
                crate::toast::show(window, &t!("Renamed"));
                moved(app, window, moves.into_iter().map(|(old, new)| (old.id(), new.id())).collect());
            }
            Ok(FolderMove::Taken(key)) => {
                finish(window, Some(t!("The key %{name} already exists", name = KeyName::display_id(&key.id())).into_owned()))
            }
            Err(e) => finish(window, Some(t!("Renaming failed: %{error}", error = e.localized()).into_owned())),
        });
    });
}

/// "Delete" on key or folder `row` (the menu asked twice): removed from Redis.
fn delete(app: &App, window: &MainWindow, row: String) {
    if !row.ends_with(DELIMITER) && app.key_detail.is_new(window.connection_id().as_str(), &row) {
        return removed(app, window, &row);
    }
    let state = app.state.clone();
    app.rt.spawn(async move {
        let name = KeyName::from_id(&row);
        let deleted = if row.ends_with(DELIMITER) {
            explorer::delete_folder(&state, None, &name).await.map(Some)
        } else {
            explorer::delete_key(&state, None, &name).await.map(|()| None)
        };
        crate::app::later(move |app, window| match deleted {
            Ok(count) => {
                let done = match count {
                    Some(count) => t!("%{count} keys deleted", count = count),
                    None => t!("Deleted"),
                };
                crate::toast::show(window, &done);
                removed(app, window, &row);
            }
            Err(e) => crate::toast::show(window, &t!("Deleting failed: %{error}", error = e.localized())),
        });
    });
}

/// Wires the row menu's writing entries and the name dialog.
pub(super) fn init(app: &Rc<App>, window: &MainWindow) {
    // the key or folder the dialog renames ("" while it makes a key)
    let target: Rc<RefCell<String>> = Rc::default();
    let tree = window.global::<KeyTree>();
    tree.on_add_key_requested({
        let target = target.clone();
        move |row| crate::app::with(|_, window| open_add(window, &target, &row))
    });
    tree.on_rename_requested({
        let target = target.clone();
        move |row| crate::app::with(|_, window| open_rename(window, &target, &row))
    });
    tree.on_delete_requested(|row| crate::app::with(|app, window| delete(app, window, row.to_string())));
    window.global::<KeyNameDialog>().on_submit({
        let app = app.clone();
        move |text| {
            let Some(window) = app.window.upgrade() else { return };
            let dialog = window.global::<KeyNameDialog>();
            if dialog.get_busy() {
                return;
            }
            dialog.set_busy(true);
            let from = target.borrow().clone();
            match dialog.get_action() {
                KeyNameAction::Add => submit_add(&app, &window, KeyName::from_id(&text).id()),
                KeyNameAction::RenameKey => submit_rename_key(&app, &window, from, KeyName::from_id(&text).id()),
                KeyNameAction::RenameFolder => submit_rename_folder(&app, &window, from, &KeyName::from_id(&text).id()),
            }
        }
    });
}
