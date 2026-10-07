//! The `KeyspacePanel` tree: one walk of the keyspace per connection fills an index
//! of every key by folder, so opening a folder needs no scan; the rows on screen
//! take in each page the walk brings, and a search filters the index.

use crate::app::App;
use crate::backend::LockExt;
use rustc_hash::FxHashMap;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::{Arc, Mutex};

use slint::{ComponentHandle, Model, ModelRc, VecModel, Weak};
use tokio::runtime::Handle;

use crate::backend::key_name::KeyName;
use crate::backend::redis_value_type::RedisValueType;
use crate::backend::state::AppState;
use crate::{KeyTree, KeyspaceRowData, MainWindow, TreeRowKind};

mod index;
mod search;
mod selection;
mod walk;

use index::{CachedChild, Index};
use search::{Search, start_search};
use selection::Selection;
use walk::{Walk, load_root};

const DELIMITER: &str = ":";

/// Folders holding search matches auto-open while the tree is under this
/// many rows; past it they arrive closed, so a broad search doesn't build a
/// huge fully-expanded tree.
const SEARCH_AUTO_EXPAND_ROWS: usize = 2000;

/// Display order for one tree level: numeric labels first (numeric order),
/// then everything else alphabetically, case-insensitively; the original
/// label breaks ties so the order is stable (see [`Node::cmp_sibling`]).
type SortKey = (u8, u128, String);

fn label_order(label: &str) -> SortKey {
    match label.parse::<u128>() {
        Ok(n) => (0, n, String::new()),
        Err(_) => (1, 0, label.to_lowercase()),
    }
}

/// What a leaf key's glyph depends on; the tree only tells hashes and lists
/// apart from everything else, so that's all that's kept of the Redis type.
#[derive(Clone, Copy, PartialEq)]
enum LeafKind {
    Hash,
    List,
    Other,
}

impl From<&RedisValueType> for LeafKind {
    fn from(value_type: &RedisValueType) -> Self {
        match value_type {
            RedisValueType::Hash => LeafKind::Hash,
            RedisValueType::List => LeafKind::List,
            RedisValueType::String
            | RedisValueType::Set
            | RedisValueType::Zset
            | RedisValueType::Stream
            | RedisValueType::Missing
            | RedisValueType::Other(_) => LeafKind::Other,
        }
    }
}

struct Node {
    indent: i32,
    is_folder: bool,
    expanded: bool,
    leaf: LeafKind,
    full_name: String,
    // where this row's own name starts in `full_name`: what comes before is
    // the folder path it sits under, so the label is never stored twice
    label_start: usize,
    order: SortKey,
    // direct children this folder is known to have: None until it has been
    // scanned at least once, then kept (and shown) even while collapsed
    child_count: Option<usize>,
    // export mode's box (see `Selection::check_of`)
    checked: i32,
}

impl Node {
    fn new(indent: i32, full_name: String, label_start: usize, is_folder: bool, leaf: LeafKind, child_count: Option<usize>) -> Self {
        let mut node = Self {
            indent,
            is_folder,
            expanded: false,
            leaf,
            full_name,
            label_start,
            order: (0, 0, String::new()),
            child_count,
            checked: 0,
        };
        node.order = label_order(node.label());
        node
    }

    fn folder(indent: i32, full_name: String, label_start: usize, child_count: Option<usize>) -> Self {
        Self::new(indent, full_name, label_start, true, LeafKind::Other, child_count)
    }

    fn key(indent: i32, full_name: String, label_start: usize, leaf: LeafKind) -> Self {
        Self::new(indent, full_name, label_start, false, leaf, None)
    }

    /// The name shown in the row: a folder's without its trailing delimiter.
    fn label(&self) -> &str {
        self.full_name[self.label_start..].trim_end_matches(DELIMITER)
    }

    fn kind(&self) -> TreeRowKind {
        if self.is_folder {
            if self.expanded { TreeRowKind::FolderOpen } else { TreeRowKind::FolderClosed }
        } else {
            match self.leaf {
                LeafKind::Hash => TreeRowKind::Hash,
                LeafKind::List => TreeRowKind::List,
                LeafKind::Other => TreeRowKind::Other,
            }
        }
    }

    /// Sibling order: folders first, then by [`label_order`].
    fn cmp_sibling(&self, other: &Node) -> std::cmp::Ordering {
        other
            .is_folder
            .cmp(&self.is_folder)
            .then_with(|| self.order.cmp(&other.order))
            .then_with(|| self.label().cmp(other.label()))
    }

    fn row_data(&self) -> KeyspaceRowData {
        KeyspaceRowData {
            indent: self.indent,
            kind: self.kind(),
            name: KeyName::display_id(self.label()).as_ref().into(),
            count: self.child_count.map(|n| n.to_string()).unwrap_or_default().into(),
            full_name: self.full_name.as_str().into(),
            loading: false,
            checked: self.checked,
        }
    }
}

struct Tree {
    nodes: Vec<Node>,
    /// every key the walk has found
    index: Index,
    walk: Walk,
    // key the tree is opening its way towards, while browsing (see
    // [`advance_reveal`]); None once its row is on screen
    pending_reveal: Option<String>,
    // Some while the tree shows search results instead of the browse listing
    search: Option<Search>,
    // folders a refresh found expanded and wants back open once their row exists
    // again, shallowest first (see `load_root` and `continue_pending_expand`)
    pending_expand: Vec<String>,
    // what export mode has ticked
    export: Selection,
}

impl Tree {
    /// The index the rows come from: the search's while there is one.
    fn shown(&self) -> &Index {
        self.search.as_ref().map_or(&self.index, |s| &s.index)
    }
}

/// Plain data only (no Slint handles), so the walk's task can hold it; mutation
/// still only happens on the UI thread, inside `invoke_from_event_loop`
/// (see [`with_model`] for the row model itself).
type SharedTree = Arc<Mutex<Tree>>;

/// The panel's row model, as installed by `init`.
fn with_model(window: &MainWindow, f: impl FnOnce(&VecModel<KeyspaceRowData>)) {
    let rows = window.global::<KeyTree>().get_rows();
    if let Some(model) = rows.as_any().downcast_ref::<VecModel<KeyspaceRowData>>() {
        f(model);
    }
}

fn refresh_row(window: &MainWindow, tree: &Tree, pos: usize) {
    with_model(window, |model| model.set_row_data(pos, tree.nodes[pos].row_data()));
}

fn position_of(tree: &Tree, full_name: &str) -> Option<usize> {
    tree.nodes.iter().position(|n| n.full_name == full_name)
}

/// Index one past the last node of `pos`'s subtree (its descendants are the
/// contiguous run of deeper-indented nodes that follows it).
fn subtree_end(tree: &Tree, pos: usize) -> usize {
    let indent = tree.nodes[pos].indent;
    let mut end = pos + 1;
    while end < tree.nodes.len() && tree.nodes[end].indent > indent {
        end += 1;
    }
    end
}

/// Merges `new` into `parent`'s direct children (`None` = root), keeping
/// [`Node::cmp_sibling`] order via one forward merge over the sorted level —
/// O(N log N + M), not O(N·M). Drops duplicates.
fn merge_children(window: &MainWindow, tree: &mut Tree, parent: Option<usize>, mut new: Vec<Node>) {
    if new.is_empty() {
        return;
    }
    new.sort_by(|a, b| a.cmp_sibling(b));

    let (parent_indent, mut idx) = match parent {
        Some(p) => (tree.nodes[p].indent, p + 1),
        None => (-1, 0),
    };
    let child_indent = parent_indent + 1;
    let mut inserted = 0usize;

    with_model(window, |model| {
        for mut node in new {
            // advance past existing siblings (and their subtrees) that sort
            // before `node`; stop at the end of the parent's subtree
            let mut duplicate = false;
            while idx < tree.nodes.len() && tree.nodes[idx].indent > parent_indent {
                let existing = &tree.nodes[idx];
                if existing.indent == child_indent {
                    match existing.cmp_sibling(&node) {
                        std::cmp::Ordering::Greater => break,
                        std::cmp::Ordering::Equal if existing.full_name == node.full_name => {
                            duplicate = true;
                            break;
                        }
                        _ => {}
                    }
                }
                idx += 1;
            }
            if duplicate {
                continue;
            }
            node.checked = tree.export.check_of(&node.full_name, node.is_folder);
            model.insert(idx, node.row_data());
            tree.nodes.insert(idx, node);
            idx += 1;
            inserted += 1;
        }
    });

    if let Some(p) = parent
        && inserted > 0
    {
        let node = &mut tree.nodes[p];
        node.child_count = Some(node.child_count.unwrap_or(0) + inserted);
        refresh_row(window, tree, p);
    }
}

/// Nodes (all folders closed) for `children`, each folder with its count in `index`.
fn nodes_from_children(index: &Index, indent: i32, children: &[CachedChild]) -> Vec<Node> {
    children
        .iter()
        .map(|c| {
            if c.is_folder {
                Node::folder(indent, c.full_name.clone(), c.label_start, index.count(&c.full_name))
            } else {
                Node::key(indent, c.full_name.clone(), c.label_start, c.leaf)
            }
        })
        .collect()
}

fn collapse(window: &MainWindow, tree: &mut Tree, pos: usize) {
    let end = subtree_end(tree, pos);
    with_model(window, |model| {
        for _ in pos + 1..end {
            model.remove(pos + 1);
        }
    });
    tree.nodes.drain(pos + 1..end);
    tree.nodes[pos].expanded = false;
    refresh_row(window, tree, pos);
}

/// Puts the children just added to the shown index on screen: merged under a
/// folder that is open (the root always is), a count update for a closed one.
/// With `auto_expand`, new folders open while the tree is small (search results).
fn show_additions(window: &MainWindow, tree: &mut Tree, added: Vec<(String, CachedChild)>, auto_expand: bool) {
    let mut by_parent: FxHashMap<String, Vec<CachedChild>> = FxHashMap::default();
    for (parent, child) in added {
        by_parent.entry(parent).or_default().push(child);
    }
    // shallowest first, so a folder is in place before its own children
    let mut parents: Vec<(String, Vec<CachedChild>)> = by_parent.into_iter().collect();
    parents.sort_by(|(a, _), (b, _)| a.matches(DELIMITER).count().cmp(&b.matches(DELIMITER).count()).then_with(|| a.cmp(b)));
    for (parent, children) in parents {
        let parent_pos = if parent.is_empty() {
            None
        } else {
            match position_of(tree, &parent) {
                Some(pos) => Some(pos),
                None => continue, // inside a folder that isn't shown
            }
        };
        if let Some(pos) = parent_pos
            && !tree.nodes[pos].expanded
        {
            tree.nodes[pos].child_count = tree.shown().count(&parent);
            refresh_row(window, tree, pos);
            continue;
        }
        let indent = parent_pos.map_or(0, |p| tree.nodes[p].indent + 1);
        let mut nodes = nodes_from_children(tree.shown(), indent, &children);
        if auto_expand && tree.nodes.len() + nodes.len() < SEARCH_AUTO_EXPAND_ROWS {
            for node in nodes.iter_mut().filter(|n| n.is_folder) {
                node.expanded = true;
                // re-derived from the inserts that follow (all its children so
                // far are among these additions: the folder itself is new)
                node.child_count = Some(0);
            }
        }
        merge_children(window, tree, parent_pos, nodes);
    }
}

#[derive(Clone)]
struct ScanContext {
    weak: Weak<MainWindow>,
    state: Arc<AppState>,
    rt: Handle,
    tree: SharedTree,
    // Bumped on every new walk. A walk remembers its starting value and stops
    // (dropping queued pages) once it no longer matches, so a reload can't
    // splice stale keys into the new index.
    generation: Arc<AtomicU64>,
    // set to stop the export under way
    export_cancel: Arc<AtomicBool>,
}

/// Opens the folder at `pos` with the children the shown index knows; those the
/// walk finds later join in as they come.
fn expand(window: &MainWindow, tree: &mut Tree, pos: usize) {
    tree.nodes[pos].expanded = true;
    let indent = tree.nodes[pos].indent + 1;
    let children = tree.shown().children(&tree.nodes[pos].full_name);
    let nodes = nodes_from_children(tree.shown(), indent, &children);
    // child_count is re-derived from the inserts, so reset it first
    tree.nodes[pos].child_count = Some(0);
    merge_children(window, tree, Some(pos), nodes);
    refresh_row(window, tree, pos);
}

/// The folders `key` lives under, shallowest first: `a:b:c` yields `a:`, `a:b:`.
fn ancestor_folders(key: &str) -> Vec<String> {
    key.match_indices(DELIMITER).map(|(i, _)| key[..i + DELIMITER.len()].to_string()).collect()
}

fn scroll_to_row(window: &MainWindow, row: Option<usize>) {
    window.global::<KeyTree>().set_scroll_row(row.map_or(-1, |pos| pos as i32));
    window.global::<KeyTree>().set_scroll_tick(window.global::<KeyTree>().get_scroll_tick().wrapping_add(1));
}

/// Walks `tree.pending_reveal`'s folder path, opening folders until its row
/// exists, then scrolls to it. A folder the walk hasn't found yet stops it; it
/// goes on with the next page (see [`continue_reveal`]).
fn advance_reveal(window: &MainWindow, tree: &mut Tree) {
    while let Some(target) = tree.pending_reveal.clone() {
        if let Some(pos) = position_of(tree, &target) {
            tree.pending_reveal = None;
            scroll_to_row(window, Some(pos));
            return;
        }
        let closed = ancestor_folders(&target).into_iter().map(|folder| position_of(tree, &folder)).find(|pos| pos.is_none_or(|p| !tree.nodes[p].expanded));
        match closed {
            // opened, so deeper rows exist now: look again
            Some(Some(pos)) => expand(window, tree, pos),
            // a folder with no row yet, or every folder open and no row for the
            // key: the walk may bring it in later
            _ => return,
        }
    }
}

fn continue_reveal(ctx: &ScanContext) {
    let Some(window) = ctx.weak.upgrade() else { return };
    advance_reveal(&window, &mut ctx.tree.lock_recover());
}

/// Re-opens the `pending_expand` folders that now have a row; a folder whose
/// parent hasn't appeared yet stays queued for the next page.
fn continue_pending_expand(ctx: &ScanContext) {
    let Some(window) = ctx.weak.upgrade() else { return };
    let mut tree = ctx.tree.lock_recover();
    for name in std::mem::take(&mut tree.pending_expand) {
        match position_of(&tree, &name) {
            Some(pos) if !tree.nodes[pos].expanded => expand(&window, &mut tree, pos),
            Some(_) => {}
            None => tree.pending_expand.push(name),
        }
    }
}

/// Selects `full_name`'s row and brings it into view (the "show active tab's
/// key" button): while browsing, opens folders until its row exists; during a
/// search the tree is left as-is, and the key lights up if present.
fn focus_key(ctx: &ScanContext, full_name: String) {
    let Some(window) = ctx.weak.upgrade() else { return };
    window.global::<KeyTree>().set_selected(full_name.clone().into());

    let mut tree = ctx.tree.lock_recover();
    if tree.search.is_some() {
        let row = position_of(&tree, &full_name);
        scroll_to_row(&window, row);
        return;
    }
    tree.pending_reveal = Some(full_name);
    advance_reveal(&window, &mut tree);
}

fn on_row_clicked(app: &App, full_name: String) {
    let ctx = &app.keyspace.0;
    let Some(window) = ctx.weak.upgrade() else { return };
    window.global::<KeyTree>().set_selected(full_name.clone().into());

    if !full_name.ends_with(DELIMITER) {
        crate::tabs::select_key(app, full_name);
        return;
    }
    let mut tree = ctx.tree.lock_recover();
    let Some(pos) = position_of(&tree, &full_name) else { return }; // e.g. a refresh raced the click
    if tree.nodes[pos].expanded {
        collapse(&window, &mut tree, pos);
    } else {
        expand(&window, &mut tree, pos);
    }
}

/// The keyspace tree: what it shows, and the walk filling it.
pub struct Keyspace(ScanContext);

impl Keyspace {
    /// An empty tree; nothing is connected at startup, so it is only filled by
    /// [`reload`], once the user connects.
    pub fn new(window: &MainWindow, state: Arc<AppState>, rt: Handle) -> Self {
        // one persistent model, mutated incrementally (insert/remove/set_row_data)
        // so a batch of new rows never rebuilds the rows already on screen
        window.global::<KeyTree>().set_rows(ModelRc::new(VecModel::<KeyspaceRowData>::default()));
        let tree = Tree { nodes: Vec::new(), index: Index::default(), walk: Walk::default(), pending_reveal: None, search: None, pending_expand: Vec::new(), export: Selection::default() };
        Self(ScanContext { weak: window.as_weak(), state, rt, tree: Arc::new(Mutex::new(tree)), generation: Arc::new(AtomicU64::new(0)), export_cancel: Arc::new(AtomicBool::new(false)) })
    }
}

/// Connecting (possibly to a different server): the keyspace is walked anew, and
/// any search belonged to the previous one, so the box is cleared with the tree.
pub fn reload(app: &App) {
    let ctx = &app.keyspace.0;
    if let Some(window) = ctx.weak.upgrade() {
        window.global::<KeyTree>().set_search_text("".into());
    }
    selection::reset(ctx);
    // no rows left to re-expand: another connection starts with its folders closed
    ctx.tree.lock_recover().nodes.clear();
    load_root(ctx, false);
}

/// Wires the tree's rows, its search box and its refresh button.
pub fn init(app: &Rc<App>, window: &MainWindow) {
    selection::init(app, window);
    window.global::<KeyTree>().on_row_clicked({
        let app = app.clone();
        move |full_name| on_row_clicked(&app, full_name.to_string())
    });
    window.global::<KeyTree>().on_focus_key({
        let app = app.clone();
        move |full_name| focus_key(&app.keyspace.0, full_name.to_string())
    });
    window.global::<KeyTree>().on_search_submitted({
        let app = app.clone();
        move |text| start_search(&app.keyspace.0, text.to_string())
    });
    // the refresh button: a new walk, any search shown found again on it
    window.global::<KeyTree>().on_refresh_requested({
        let app = app.clone();
        move || {
            let ctx = &app.keyspace.0;
            if let Some(window) = ctx.weak.upgrade() {
                crate::connect_active(&window, ctx.state.clone(), &ctx.rt);
            }
            load_root(ctx, true);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::{ancestor_folders, label_order, LeafKind, Node, TreeRowKind};
    use std::cmp::Ordering;

    #[test]
    fn numeric_labels_first_in_numeric_order_then_alpha_case_insensitive() {
        let mut labels = vec!["b", "10", "A", "2", "c", "1", "abc"];
        labels.sort_by_cached_key(|l| label_order(l));
        assert_eq!(labels, vec!["1", "2", "10", "A", "abc", "b", "c"]);
    }

    #[test]
    fn a_key_reveals_through_every_folder_above_it_shallowest_first() {
        assert_eq!(ancestor_folders("app:users:42:name"), vec!["app:", "app:users:", "app:users:42:"]);
        assert_eq!(ancestor_folders("plain-key"), Vec::<String>::new());
    }

    #[test]
    fn a_row_derives_its_label_from_the_full_name() {
        let folder = Node::folder(1, "app:users:".into(), 4, None);
        assert_eq!(folder.label(), "users");
        let key = Node::key(2, "app:users:42".into(), 10, LeafKind::Hash);
        assert_eq!(key.label(), "42");
        assert_eq!(key.kind(), TreeRowKind::Hash);
        assert_eq!(Node::key(0, "plain".into(), 0, LeafKind::Other).label(), "plain");
    }

    #[test]
    fn siblings_sort_folders_first_then_by_label_with_case_as_the_last_tie_break() {
        let folder = Node::folder(0, "z:".into(), 0, None);
        let key = |name: &str| Node::key(0, name.into(), 0, LeafKind::Other);
        assert_eq!(folder.cmp_sibling(&key("a")), Ordering::Less);
        assert_eq!(key("10").cmp_sibling(&key("9")), Ordering::Greater);
        assert_eq!(key("B").cmp_sibling(&key("b")), Ordering::Less);
        assert_eq!(key("b").cmp_sibling(&key("b")), Ordering::Equal);
    }
}

