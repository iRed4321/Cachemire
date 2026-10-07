//! Export mode of the keyspace tree: the keys and folders ticked in it, what that makes of the
//! rows' boxes and of the count, and the export itself (see `export_files`).

use std::rc::Rc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use rust_i18n::t;
use rustc_hash::{FxHashMap, FxHashSet};
use slint::{ComponentHandle, Model};

use super::{DELIMITER, Index, ScanContext, Tree, position_of, with_model};
use crate::app::App;
use crate::backend::LockExt;
use crate::export_files::{self, Plan};
use crate::{KeyTree, MainWindow};

/// What is ticked: lone keys, and folders by prefix (`""` = the whole keyspace), the two never
/// overlapping. `full` and `partial` are the folders ticked in all or in part of what a search shows.
#[derive(Default)]
pub(super) struct Selection {
    pub on: bool,
    keys: FxHashSet<String>,
    folders: FxHashSet<String>,
    full: FxHashSet<String>,
    partial: FxHashSet<String>,
}

/// The folders above `id` (a key or a folder), shallowest first.
fn ancestors(id: &str) -> impl Iterator<Item = &str> {
    let own = id.strip_suffix(DELIMITER).unwrap_or(id);
    own.match_indices(DELIMITER).map(move |(i, _)| &own[..i + DELIMITER.len()])
}

/// The folder `id` sits in (`""` at the root).
fn parent_of(id: &str) -> &str {
    ancestors(id).last().unwrap_or("")
}

/// Every key id under folder `prefix`.
fn collect_keys(index: &Index, prefix: &str, out: &mut Vec<String>) {
    let Some(level) = index.levels.get(prefix) else { return };
    out.extend(level.keys.keys().map(|label| format!("{prefix}{label}")));
    for label in &level.folders {
        collect_keys(index, &format!("{prefix}{label}"), out);
    }
}

/// How many keys are under folder `prefix`.
fn keys_under(index: &Index, prefix: &str) -> usize {
    if prefix.is_empty() {
        return index.keys;
    }
    let Some(level) = index.levels.get(prefix) else { return 0 };
    level.keys.len() + level.folders.iter().map(|label| keys_under(index, &format!("{prefix}{label}"))).sum::<usize>()
}

impl Selection {
    fn covered(&self, id: &str) -> bool {
        !self.folders.is_empty()
            && (self.folders.contains("") || (id.ends_with(DELIMITER) && self.folders.contains(id)) || ancestors(id).any(|a| self.folders.contains(a)))
    }

    fn key_ticked(&self, id: &str) -> bool {
        self.keys.contains(id) || self.covered(id)
    }

    /// A row's box: 1 ticked, 2 for a folder with some of its keys ticked, else 0.
    pub fn check_of(&self, id: &str, is_folder: bool) -> i32 {
        if !self.on {
            0
        } else if !is_folder {
            self.key_ticked(id) as i32
        } else if self.covered(id) || self.full.contains(id) {
            1
        } else if self.partial.contains(id) {
            2
        } else {
            0
        }
    }

    pub fn clear(&mut self) {
        self.keys.clear();
        self.folders.clear();
        self.full.clear();
        self.partial.clear();
    }

    /// How many keys are ticked.
    pub fn count(&self, index: &Index) -> usize {
        self.keys.len() + self.folders.iter().map(|folder| keys_under(index, folder)).sum::<usize>()
    }

    /// Ticks or unticks `id`; in a search, a folder stands for the keys of it that the search shows.
    pub fn tick(&mut self, id: &str, on: bool, index: &Index, shown: &Index, searching: bool) {
        if !id.ends_with(DELIMITER) {
            self.tick_key(id, on, index);
        } else if searching {
            let mut keys = Vec::new();
            collect_keys(shown, id, &mut keys);
            for key in keys {
                self.tick_key(&key, on, index);
            }
        } else {
            self.tick_folder(id, on, index);
        }
    }

    fn tick_key(&mut self, id: &str, on: bool, index: &Index) {
        if on {
            if !self.covered(id) {
                self.keys.insert(id.to_string());
                self.merge_up(id, index);
            }
        } else if !self.keys.remove(id) && self.covered(id) {
            self.split(id, index);
        }
    }

    fn tick_folder(&mut self, id: &str, on: bool, index: &Index) {
        if on {
            if self.covered(id) {
                return;
            }
            self.drop_under(id);
            self.folders.insert(id.to_string());
            self.merge_up(id, index);
        } else if self.covered(id) {
            if !self.folders.remove(id) {
                self.split(id, index);
            }
        } else {
            self.drop_under(id);
        }
    }

    fn drop_under(&mut self, prefix: &str) {
        self.folders.retain(|folder| !folder.starts_with(prefix));
        self.keys.retain(|key| !key.starts_with(prefix));
    }

    /// Once everything in the folder holding `id` is ticked, ticks the folder instead, and so up.
    fn merge_up(&mut self, id: &str, index: &Index) {
        let mut parent = parent_of(id).to_string();
        loop {
            let Some(level) = index.levels.get(&parent) else { return };
            let child = |label: &str| format!("{parent}{label}");
            if !level.folders.iter().all(|l| self.folders.contains(&child(l))) || !level.keys.keys().all(|l| self.keys.contains(&child(l))) {
                return;
            }
            for label in &level.folders {
                self.folders.remove(&child(label));
            }
            for label in level.keys.keys() {
                self.keys.remove(&child(label));
            }
            self.folders.insert(parent.clone());
            if parent.is_empty() {
                return;
            }
            parent = parent_of(&parent).to_string();
        }
    }

    /// Unticks `target` (a key or a folder) out of the folder ticked above it, by ticking
    /// everything else of that folder: its other children, down the path to `target`.
    fn split(&mut self, target: &str, index: &Index) {
        let covering = if self.folders.contains("") { "" } else { ancestors(target).find(|a| self.folders.contains(*a)).unwrap_or("") }.to_string();
        self.folders.remove(&covering);
        let mut path = vec![covering.as_str()];
        path.extend(ancestors(target).filter(|a| a.len() > covering.len()));
        for (i, folder) in path.iter().enumerate() {
            let next = path.get(i + 1).copied().unwrap_or(target);
            let Some(level) = index.levels.get(*folder) else { continue };
            for label in &level.folders {
                let child = format!("{folder}{label}");
                if child != next {
                    self.folders.insert(child);
                }
            }
            for label in level.keys.keys() {
                let child = format!("{folder}{label}");
                if child != next {
                    self.keys.insert(child);
                }
            }
        }
    }

    /// Recomputes `full` and `partial` for the folders of the rows on screen.
    fn refresh_view(&mut self, shown: &Index, searching: bool) {
        self.full.clear();
        self.partial.clear();
        if self.folders.contains("") {
            return;
        }
        if !searching {
            let above: FxHashSet<String> = self.keys.iter().chain(&self.folders).flat_map(|id| ancestors(id)).map(str::to_string).collect();
            self.partial = above;
            return;
        }
        let mut counts: FxHashMap<String, (usize, usize)> = FxHashMap::default();
        for (id, _) in shown.keys() {
            let ticked = self.key_ticked(&id) as usize;
            for folder in ancestors(&id) {
                match counts.get_mut(folder) {
                    Some(count) => {
                        count.0 += ticked;
                        count.1 += 1;
                    }
                    None => {
                        counts.insert(folder.to_string(), (ticked, 1));
                    }
                }
            }
        }
        for (folder, (ticked, total)) in counts {
            if ticked == total {
                self.full.insert(folder);
            } else if ticked > 0 {
                self.partial.insert(folder);
            }
        }
    }

    /// Ticks everything: the whole keyspace, or every key a search shows.
    fn select_all(&mut self, shown: &Index, searching: bool) {
        if !searching {
            self.keys.clear();
            self.folders.clear();
            self.folders.insert(String::new());
            return;
        }
        let ids: Vec<String> = shown.keys().map(|(id, _)| id).collect();
        for id in ids {
            if !self.covered(&id) {
                self.keys.insert(id);
            }
        }
    }

    /// The export of what is ticked.
    fn plan(&self, index: &Index, group: bool) -> Plan {
        let mut prefixes: Vec<&String> = self.folders.iter().collect();
        prefixes.sort();
        let folders = prefixes
            .into_iter()
            .map(|prefix| {
                let mut keys = Vec::new();
                collect_keys(index, prefix, &mut keys);
                keys.sort();
                (prefix.clone(), keys)
            })
            .collect();
        let mut keys: Vec<String> = self.keys.iter().cloned().collect();
        keys.sort();
        Plan { folders, keys, group }
    }
}

/// Shows what the selection implies: each row's box and the count.
pub(super) fn refresh(window: &MainWindow, tree: &mut Tree) {
    let Tree { export, nodes, index, search, .. } = tree;
    let shown = search.as_ref().map_or(&*index, |s| &s.index);
    export.refresh_view(shown, search.is_some());
    let checks: Vec<i32> = nodes.iter().map(|n| export.check_of(&n.full_name, n.is_folder)).collect();
    with_model(window, |model| {
        for (pos, (node, check)) in nodes.iter_mut().zip(checks).enumerate() {
            if node.checked != check {
                node.checked = check;
                model.set_row_data(pos, node.row_data());
            }
        }
    });
    window.global::<KeyTree>().set_export_count(export.count(index) as i32);
}

/// Forgets the selection and leaves export mode.
pub(super) fn reset(ctx: &ScanContext) {
    ctx.tree.lock_recover().export = Selection::default();
    if let Some(window) = ctx.weak.upgrade() {
        window.global::<KeyTree>().set_export_mode(false);
        window.global::<KeyTree>().set_export_count(0);
    }
}

fn with_tree(app: &App, f: impl FnOnce(&MainWindow, &mut Tree)) {
    let ctx = &app.keyspace.0;
    let Some(window) = ctx.weak.upgrade() else { return };
    f(&window, &mut ctx.tree.lock_recover());
}

fn tick(tree: &mut Tree, id: &str, on: bool) {
    let Tree { export, index, search, .. } = tree;
    let shown = search.as_ref().map_or(&*index, |s| &s.index);
    export.tick(id, on, index, shown, search.is_some());
}

fn start_selection(app: &App) {
    let ctx = &app.keyspace.0;
    let Some(window) = ctx.weak.upgrade() else { return };
    let group = window.global::<KeyTree>().get_export_group();
    let plan = {
        let tree = ctx.tree.lock_recover();
        tree.export.plan(&tree.index, group)
    };
    start(ctx, plan);
}

/// Exports `id` alone (a key, or the keys of a folder that the tree shows), whatever is ticked.
fn start_row(app: &App, id: String) {
    let ctx = &app.keyspace.0;
    let Some(window) = ctx.weak.upgrade() else { return };
    let group = window.global::<KeyTree>().get_export_group();
    let plan = {
        let tree = ctx.tree.lock_recover();
        if id.ends_with(DELIMITER) {
            let mut keys = Vec::new();
            collect_keys(tree.shown(), &id, &mut keys);
            keys.sort();
            Plan { folders: vec![(id, keys)], keys: Vec::new(), group }
        } else {
            Plan { folders: Vec::new(), keys: vec![id], group }
        }
    };
    start(ctx, plan);
}

/// Asks for a folder, then writes `plan` there off the UI thread, with progress in the bar.
fn start(ctx: &ScanContext, plan: Plan) {
    let Some(window) = ctx.weak.upgrade() else { return };
    let global = window.global::<KeyTree>();
    let total = plan.total();
    if total == 0 || global.get_export_busy() {
        return;
    }
    let connection_id = match ctx.state.connections.lock_recover().get_active() {
        Ok(connection) => connection.id,
        Err(e) => {
            crate::toast::show(&window, &t!("Export failed: %{error}", error = e));
            return;
        }
    };
    global.set_export_busy(true);
    global.set_export_progress(t!("Choose a folder...").as_ref().into());
    ctx.export_cancel.store(false, Ordering::Relaxed);
    let (weak, state, cancel, rt) = (ctx.weak.clone(), ctx.state.clone(), ctx.export_cancel.clone(), ctx.rt.clone());
    rt.spawn(async move {
        let dir = tokio::task::spawn_blocking(|| rfd::FileDialog::new().pick_folder()).await.ok().flatten();
        let Some(dir) = dir else {
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(window) = weak.upgrade() {
                    window.global::<KeyTree>().set_export_busy(false);
                }
            });
            return;
        };
        let mut last = Instant::now();
        let progress_weak = weak.clone();
        let result = export_files::run(&state, &connection_id, plan, &dir, &cancel, |done, total| {
            if done != total && last.elapsed() < Duration::from_millis(100) {
                return;
            }
            last = Instant::now();
            let weak = progress_weak.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(window) = weak.upgrade() {
                    window.global::<KeyTree>().set_export_progress(t!("Exporting %{done} of %{total}...", done = done, total = total).as_ref().into());
                }
            });
        })
        .await;
        let _ = slint::invoke_from_event_loop(move || {
            let Some(window) = weak.upgrade() else { return };
            window.global::<KeyTree>().set_export_busy(false);
            window.global::<KeyTree>().set_export_progress("".into());
            let text = match result {
                Ok(outcome) if outcome.cancelled => t!("Export cancelled").into_owned(),
                Ok(outcome) => t!("Exported %{count} keys", count = outcome.keys).into_owned(),
                Err(e) => t!("Export failed: %{error}", error = e).into_owned(),
            };
            crate::toast::show(&window, &text);
        });
    });
}

pub(super) fn init(app: &Rc<App>, window: &MainWindow) {
    let global = window.global::<KeyTree>();
    global.on_export_mode_toggled({
        let app = app.clone();
        move || {
            with_tree(&app, |window, tree| {
                tree.export.on = !tree.export.on;
                if !tree.export.on {
                    tree.export.clear();
                }
                window.global::<KeyTree>().set_export_mode(tree.export.on);
                refresh(window, tree);
            })
        }
    });
    global.on_export_row_toggled({
        let app = app.clone();
        move |name| {
            with_tree(&app, |window, tree| {
                let on = tree.export.check_of(&name, name.ends_with(DELIMITER)) != 1;
                tick(tree, &name, on);
                refresh(window, tree);
            })
        }
    });
    global.on_export_range_toggled({
        let app = app.clone();
        move |from, to| {
            with_tree(&app, |window, tree| {
                let (Some(a), Some(b)) = (position_of(tree, &from), position_of(tree, &to)) else {
                    let on = tree.export.check_of(&to, to.ends_with(DELIMITER)) != 1;
                    tick(tree, &to, on);
                    refresh(window, tree);
                    return;
                };
                let on = tree.export.check_of(&from, from.ends_with(DELIMITER)) == 1;
                let names: Vec<String> = tree.nodes[a.min(b)..=a.max(b)].iter().map(|n| n.full_name.clone()).collect();
                for name in names {
                    tick(tree, &name, on);
                }
                refresh(window, tree);
            })
        }
    });
    global.on_export_select_row({
        let app = app.clone();
        move |name| {
            with_tree(&app, |window, tree| {
                tree.export.on = true;
                window.global::<KeyTree>().set_export_mode(true);
                tick(tree, &name, true);
                refresh(window, tree);
            })
        }
    });
    global.on_export_select_all({
        let app = app.clone();
        move || {
            with_tree(&app, |window, tree| {
                let Tree { export, index, search, .. } = &mut *tree;
                let shown = search.as_ref().map_or(&*index, |s| &s.index);
                export.select_all(shown, search.is_some());
                refresh(window, tree);
            })
        }
    });
    global.on_export_clear({
        let app = app.clone();
        move || {
            with_tree(&app, |window, tree| {
                tree.export.clear();
                refresh(window, tree);
            })
        }
    });
    global.on_export_start({
        let app = app.clone();
        move || start_selection(&app)
    });
    global.on_export_row({
        let app = app.clone();
        move |name| start_row(&app, name.to_string())
    });
    global.on_export_cancel({
        let app = app.clone();
        move || app.keyspace.0.export_cancel.store(true, Ordering::Relaxed)
    });
}
