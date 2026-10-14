//! The tree's search box: the keys of the index whose name matches a glob, the way
//! Redis's MATCH reads it, shown in place of the browse listing.

use super::index::Index;
use super::{LeafKind, ScanContext, merge_children, nodes_from_children, selection, show_additions, with_model};
use super::walk::show_footer;
use crate::backend::LockExt;
use crate::backend::key_name::KeyName;

/// The glob pattern for the search box: used as-is if it already has glob
/// syntax (`*`, `?`, `[`, `\`), otherwise wrapped as a "contains" search.
/// Case-sensitive, like Redis's MATCH.
pub(super) fn search_pattern(text: &str) -> String {
    if text.contains(['*', '?', '[', '\\']) { text.to_string() } else { format!("*{text}*") }
}

/// Whether `text` matches glob `pattern` the way Redis's MATCH does: `*`, `?`,
/// `[abc]`/`[a-z]`/`[^a]` classes, and `\` taking the next character literally.
pub(super) fn glob_match(pattern: &[u8], text: &[u8]) -> bool {
    let (mut p, mut t) = (0, 0);
    // where to resume after the last `*`: its pattern position and the text it covers up to
    let mut star: Option<(usize, usize)> = None;
    while t < text.len() {
        if p < pattern.len() && pattern[p] == b'*' {
            p += 1;
            star = Some((p, t));
            continue;
        }
        let next = if p < pattern.len() {
            match pattern[p] {
                b'?' => Some(p + 1),
                b'[' => class_match(pattern, p, text[t]),
                b'\\' if p + 1 < pattern.len() => (pattern[p + 1] == text[t]).then_some(p + 2),
                c => (c == text[t]).then_some(p + 1),
            }
        } else {
            None
        };
        match (next, star) {
            (Some(next), _) => {
                p = next;
                t += 1;
            }
            // the last `*` takes one more character, and matching resumes after it
            (None, Some((after_star, covered))) => {
                p = after_star;
                t = covered + 1;
                star = Some((after_star, covered + 1));
            }
            (None, None) => return false,
        }
    }
    pattern[p..].iter().all(|&c| c == b'*')
}

/// The `[...]` class at `pattern[open]`: where the pattern goes on if `c` is in it.
fn class_match(pattern: &[u8], open: usize, c: u8) -> Option<usize> {
    let mut i = open + 1;
    let negate = pattern.get(i) == Some(&b'^');
    i += negate as usize;
    let mut found = false;
    while i < pattern.len() && pattern[i] != b']' {
        if pattern[i] == b'\\' && i + 1 < pattern.len() {
            found |= pattern[i + 1] == c;
            i += 2;
        } else if i + 2 < pattern.len() && pattern[i + 1] == b'-' && pattern[i + 2] != b']' {
            let (low, high) = (pattern[i].min(pattern[i + 2]), pattern[i].max(pattern[i + 2]));
            found |= (low..=high).contains(&c);
            i += 3;
        } else {
            found |= pattern[i] == c;
            i += 1;
        }
    }
    (found != negate).then_some(i + 1)
}

/// Whether key `id` matches glob `pattern` (matched on the key's bytes).
fn id_matches(pattern: &[u8], id: &str) -> bool {
    if id.contains('\\') { glob_match(pattern, KeyName::from_id(id).as_bytes()) } else { glob_match(pattern, id.as_bytes()) }
}

/// The search being shown in place of the browse listing: the walk's keys that match.
pub(super) struct Search {
    /// the glob the search box's text stands for
    pub(super) pattern: Vec<u8>,
    pub(super) index: Index,
}

/// Shows the keys of the index that match the box's `text` (an empty one leaves
/// search mode); keys the walk finds later join in as they come.
pub(super) fn start_search(ctx: &ScanContext, text: String) {
    let text = text.trim().to_string();
    if text.is_empty() {
        exit_search(ctx);
        return;
    }
    let Some(window) = ctx.weak.upgrade() else { return };
    let mut tree = ctx.tree.lock_recover();
    let pattern = search_pattern(&text).into_bytes();
    let mut index = Index::default();
    let mut added = Vec::new();
    let matches: Vec<(String, LeafKind)> = tree.index.keys().filter(|(id, _)| id_matches(&pattern, id)).collect();
    for (id, leaf) in matches {
        index.place(&id, leaf, &mut added);
    }
    tree.nodes.clear();
    tree.pending_reveal = None;
    tree.search = Some(Search { pattern, index });
    with_model(&window, |model| model.set_vec(Vec::new()));
    show_additions(&window, &mut tree, added, true);
    show_footer(&window, &tree);
    if tree.export.on {
        selection::refresh(&window, &mut tree);
    }
}

/// Leaves search mode for the browse listing, folders all closed. Does nothing if
/// no search is shown.
pub(super) fn exit_search(ctx: &ScanContext) {
    let Some(window) = ctx.weak.upgrade() else { return };
    let mut tree = ctx.tree.lock_recover();
    if tree.search.take().is_none() {
        return;
    }
    tree.nodes.clear();
    tree.pending_reveal = None;
    with_model(&window, |model| model.set_vec(Vec::new()));
    let nodes = nodes_from_children(&tree.index, 0, &tree.index.children(""));
    merge_children(&window, &mut tree, None, nodes);
    show_footer(&window, &tree);
    if tree.export.on {
        selection::refresh(&window, &mut tree);
    }
}

#[cfg(test)]
mod tests {
    use super::search_pattern;

    #[test]
    fn plain_text_searches_contain_globs_are_used_as_is() {
        assert_eq!(search_pattern("session"), "*session*");
        assert_eq!(search_pattern("user:42"), "*user:42*");
        assert_eq!(search_pattern("user:*"), "user:*");
        assert_eq!(search_pattern("h?llo"), "h?llo");
        assert_eq!(search_pattern("id:[0-9]"), "id:[0-9]");
        assert_eq!(search_pattern(r"a\*b"), r"a\*b");
    }
}
