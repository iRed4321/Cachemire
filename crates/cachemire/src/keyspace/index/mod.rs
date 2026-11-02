//! Every key the walk has found, by folder: what the tree's rows are made from,
//! so opening a folder or searching needs no scan of the server.

use rustc_hash::{FxHashMap, FxHashSet};

use super::{DELIMITER, LeafKind};

/// A child to show under a folder: plain data (not a `Node`), since it's made into
/// one at whatever indent the folder sits at.
#[derive(Clone)]
pub(super) struct CachedChild {
    pub(super) full_name: String,
    pub(super) label_start: usize,
    pub(super) is_folder: bool,
    pub(super) leaf: LeafKind,
}

/// One folder's direct children, by label (the name past the folder's own).
#[derive(Default)]
pub(super) struct Level {
    pub(super) folders: FxHashSet<Box<str>>,
    pub(super) keys: FxHashMap<Box<str>, LeafKind>,
}

impl Level {
    fn len(&self) -> usize {
        self.folders.len() + self.keys.len()
    }
}

/// Every key found so far, by folder ("" = the root); a key's folders are the
/// prefixes of its id that end in [`DELIMITER`].
#[derive(Default)]
pub(super) struct Index {
    pub(super) levels: FxHashMap<String, Level>,
    pub(super) keys: usize,
}

impl Index {
    pub(super) fn level_mut(&mut self, folder: &str) -> &mut Level {
        if !self.levels.contains_key(folder) {
            self.levels.insert(folder.to_string(), Level::default());
        }
        self.levels.get_mut(folder).expect("inserted above")
    }

    /// Adds key `id`, pushing the children it adds (each folder on its path seen for
    /// the first time, then the key) onto `added` with their folder.
    pub(super) fn place(&mut self, id: &str, leaf: LeafKind, added: &mut Vec<(String, CachedChild)>) {
        let mut parent_end = 0;
        for (i, _) in id.match_indices(DELIMITER) {
            let end = i + DELIMITER.len();
            if self.level_mut(&id[..parent_end]).folders.insert(id[parent_end..end].into()) {
                let child =
                    CachedChild { full_name: id[..end].to_string(), label_start: parent_end, is_folder: true, leaf: LeafKind::Other };
                added.push((id[..parent_end].to_string(), child));
            }
            parent_end = end;
        }
        // a name ending with the delimiter is shown as that folder only
        if parent_end == id.len() {
            return;
        }
        if self.level_mut(&id[..parent_end]).keys.insert(id[parent_end..].into(), leaf).is_none() {
            self.keys += 1;
            added.push((
                id[..parent_end].to_string(),
                CachedChild { full_name: id.to_string(), label_start: parent_end, is_folder: false, leaf },
            ));
        }
    }

    /// The kind of key `id`, if it is in the index.
    pub(super) fn leaf(&self, id: &str) -> Option<LeafKind> {
        let parent_end = parent_end(id);
        self.levels.get(&id[..parent_end])?.keys.get(&id[parent_end..]).copied()
    }

    /// Removes key `id`, and then each folder above it left empty; whether it was there.
    pub(super) fn remove(&mut self, id: &str) -> bool {
        let parent_end = parent_end(id);
        let Some(level) = self.levels.get_mut(&id[..parent_end]) else { return false };
        if level.keys.remove(&id[parent_end..]).is_none() {
            return false;
        }
        self.keys -= 1;
        self.prune(&id[..parent_end]);
        true
    }

    /// Removes `folder` (its name ends with the delimiter) and every key under it;
    /// then each folder above it left empty. How many keys went.
    pub(super) fn remove_folder(&mut self, folder: &str) -> usize {
        let mut removed = 0;
        self.levels.retain(|name, level| {
            let under = name.starts_with(folder);
            if under {
                removed += level.keys.len();
            }
            !under
        });
        self.keys -= removed;
        let parent_end = parent_end(folder.strip_suffix(DELIMITER).unwrap_or(folder));
        if let Some(parent) = self.levels.get_mut(&folder[..parent_end]) {
            parent.folders.remove(&folder[parent_end..]);
        }
        self.prune(&folder[..parent_end]);
        removed
    }

    /// Drops `folder` while it holds nothing, then its parent the same way, up to the root.
    fn prune(&mut self, folder: &str) {
        let mut folder = folder.to_string();
        while !folder.is_empty() && self.levels.get(&folder).is_none_or(|level| level.len() == 0) {
            self.levels.remove(&folder);
            let parent_end = parent_end(&folder[..folder.len() - DELIMITER.len()]);
            if let Some(parent) = self.levels.get_mut(&folder[..parent_end]) {
                parent.folders.remove(&folder[parent_end..]);
            }
            folder.truncate(parent_end);
        }
    }

    /// `folder`'s direct children.
    pub(super) fn children(&self, folder: &str) -> Vec<CachedChild> {
        let Some(level) = self.levels.get(folder) else { return Vec::new() };
        let child = |label: &str, is_folder: bool, leaf: LeafKind| CachedChild {
            full_name: format!("{folder}{label}"),
            label_start: folder.len(),
            is_folder,
            leaf,
        };
        let folders = level.folders.iter().map(|label| child(label, true, LeafKind::Other));
        folders.chain(level.keys.iter().map(|(label, leaf)| child(label, false, *leaf))).collect()
    }

    /// How many direct children `folder` has, if any are known.
    pub(super) fn count(&self, folder: &str) -> Option<usize> {
        self.levels.get(folder).map(Level::len)
    }

    /// Every key's id, with its kind.
    pub(super) fn keys(&self) -> impl Iterator<Item = (String, LeafKind)> + '_ {
        self.levels.iter().flat_map(|(folder, level)| level.keys.iter().map(move |(label, leaf)| (format!("{folder}{label}"), *leaf)))
    }
}

/// Where the name past `id`'s folder starts: just past its last delimiter, else 0.
fn parent_end(id: &str) -> usize {
    id.rfind(DELIMITER).map_or(0, |i| i + DELIMITER.len())
}

#[cfg(test)]
mod tests;
