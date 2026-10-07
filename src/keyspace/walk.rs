//! The walk of the keyspace: its pages go into the index and onto the tree as they
//! come, with the footer telling how far it is.

use std::sync::atomic::Ordering;

use rust_i18n::t;
use slint::ComponentHandle;

use super::index::Index;
use super::search::glob_match;
use super::{DELIMITER, LeafKind, ScanContext, Tree, continue_pending_expand, continue_reveal, selection, show_additions, with_model};
use crate::KeyTree;
use crate::backend::LockExt;
use crate::backend::explorer::{self, WalkBatch};
use crate::backend::session::TypedKey;
use crate::MainWindow;

/// The walk stops after this many keys: the index holds every key name, so a
/// keyspace past this would cost too much memory to list whole.
const MAX_INDEXED_KEYS: i64 = 5_000_000;

/// Where the walk is, for the footer.
#[derive(Default)]
pub(super) struct Walk {
    pub(super) scanned: i64,
    pub(super) total: i64,
    pub(super) done: bool,
    pub(super) capped: bool,
}

/// The footer: how far the walk is, or how many keys match the search.
pub(super) fn show_footer(window: &MainWindow, tree: &Tree) {
    let walk = &tree.walk;
    let text = match &tree.search {
        Some(search) => match (walk.done, search.index.keys) {
            (false, n) => t!("searching... %{count} matches", count = n).into_owned(),
            (true, 0) => t!("No keys match").into_owned(),
            (true, 1) => t!("1 matching key").into_owned(),
            (true, n) => t!("%{count} matching keys", count = n).into_owned(),
        },
        None if walk.capped => t!("stopped after %{count} keys: the keyspace is too large to list whole, search it instead", count = walk.scanned).into_owned(),
        None if walk.done => t!("%{count} keys scanned", count = walk.scanned).into_owned(),
        None if walk.total > 0 => t!("scanning... %{count} of %{total} keys", count = walk.scanned.min(walk.total), total = walk.total).into_owned(),
        None => t!("scanning... %{count} keys", count = walk.scanned).into_owned(),
    };
    window.global::<KeyTree>().set_footer(text.into());
    window.global::<KeyTree>().set_export_ready(walk.done);
}

/// Takes in one page of the walk: its keys go into the index (and into the
/// search's when they match it), and those that belong on screen are shown.
pub(super) fn apply_walk_batch(window: &MainWindow, tree: &mut Tree, batch: WalkBatch) {
    let (mut added, mut matched) = (Vec::new(), Vec::new());
    for TypedKey { name, key_type } in batch.keys {
        let id = name.id();
        let leaf = LeafKind::from(&key_type);
        tree.index.place(&id, leaf, &mut added);
        if let Some(search) = tree.search.as_mut()
            && glob_match(&search.pattern, name.as_bytes())
        {
            search.index.place(&id, leaf, &mut matched);
        }
    }
    tree.walk = Walk { scanned: batch.scanned, total: batch.total, done: batch.done, capped: batch.capped };
    if tree.search.is_some() {
        show_additions(window, tree, matched, true);
    } else {
        show_additions(window, tree, added, false);
    }
    show_footer(window, tree);
    if tree.export.on {
        selection::refresh(window, tree);
    }
}

/// Starts walking the keyspace into a fresh index, its pages taken in on the UI thread.
pub(super) fn start_walk(ctx: &ScanContext) {
    let generation = ctx.generation.fetch_add(1, Ordering::Relaxed) + 1;
    let (weak, state, tree, latest) = (ctx.weak.clone(), ctx.state.clone(), ctx.tree.clone(), ctx.generation.clone());
    let reveal_ctx = ctx.clone();
    ctx.rt.spawn(async move {
        let result = {
            let (weak, tree, latest) = (weak.clone(), tree.clone(), latest.clone());
            let still_wanted = latest.clone();
            explorer::walk_keyspace(
                &state,
                None,
                MAX_INDEXED_KEYS,
                move || still_wanted.load(Ordering::Relaxed) == generation,
                move |batch| {
                    let (weak, tree, latest, reveal_ctx) = (weak.clone(), tree.clone(), latest.clone(), reveal_ctx.clone());
                    let _ = slint::invoke_from_event_loop(move || {
                        let Some(window) = weak.upgrade() else { return };
                        if latest.load(Ordering::Relaxed) != generation {
                            return;
                        }
                        apply_walk_batch(&window, &mut tree.lock_recover(), batch);
                        continue_reveal(&reveal_ctx);
                        continue_pending_expand(&reveal_ctx);
                    });
                },
            )
            .await
        };
        if let Err(e) = result {
            let _ = slint::invoke_from_event_loop(move || {
                let Some(window) = weak.upgrade() else { return };
                if latest.load(Ordering::Relaxed) == generation {
                    window.global::<KeyTree>().set_footer(t!("scan failed: %{error}", error = e).as_ref().into());
                }
            });
        }
    });
}

/// Walks the keyspace afresh into a new index (on connecting, and on refresh). The
/// folders open now reopen once the walk reaches them; a search shown with
/// `keep_search` stays, its matches found again as the walk goes.
pub(super) fn load_root(ctx: &ScanContext, keep_search: bool) {
    {
        let mut tree = ctx.tree.lock_recover();
        // shallowest first, so a folder is never looked for before the parent that reveals it
        let mut reexpand: Vec<String> = Vec::new();
        if tree.search.is_none() {
            reexpand = tree.nodes.iter().filter(|n| n.is_folder && n.expanded).map(|n| n.full_name.clone()).collect();
            reexpand.sort_by_key(|name| name.matches(DELIMITER).count());
        }
        tree.nodes.clear();
        tree.index = Index::default();
        tree.walk = Walk::default();
        tree.pending_reveal = None;
        tree.pending_expand = reexpand;
        if keep_search {
            if let Some(search) = tree.search.as_mut() {
                search.index = Index::default();
            }
        } else {
            tree.search = None;
        }
    }
    if let Some(window) = ctx.weak.upgrade() {
        with_model(&window, |model| model.set_vec(Vec::new()));
        window.global::<KeyTree>().set_selected("".into());
        window.global::<KeyTree>().set_footer(t!("scanning...").as_ref().into());
    }
    start_walk(ctx);
}
