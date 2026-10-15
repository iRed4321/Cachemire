use super::*;

fn index_of(ids: &[&str]) -> Index {
    let mut index = Index::default();
    for id in ids {
        index.place(id, LeafKind::Other, &mut Vec::new());
    }
    index
}

fn names(children: Vec<CachedChild>) -> Vec<(String, bool)> {
    let mut names: Vec<(String, bool)> = children.into_iter().map(|c| (c.full_name, c.is_folder)).collect();
    names.sort();
    names
}

#[test]
fn a_key_is_filed_under_each_folder_of_its_path() {
    let index = index_of(&["user:1:profile", "user:1:cart", "user:2", "config"]);
    assert_eq!(names(index.children("")), [("config".to_string(), false), ("user:".to_string(), true)]);
    assert_eq!(names(index.children("user:")), [("user:1:".to_string(), true), ("user:2".to_string(), false)]);
    assert_eq!(names(index.children("user:1:")), [("user:1:cart".to_string(), false), ("user:1:profile".to_string(), false)]);
    assert_eq!(index.keys, 4);
    assert_eq!(index.count("user:"), Some(2));
    assert_eq!(index.count("nothing:"), None);
}

#[test]
fn placing_reports_only_what_is_new() {
    let mut index = Index::default();
    let mut added = Vec::new();
    index.place("a:b:c", LeafKind::Hash, &mut added);
    let first: Vec<(String, String)> = added.drain(..).map(|(folder, c)| (folder, c.full_name)).collect();
    assert_eq!(first, [("".into(), "a:".into()), ("a:".into(), "a:b:".into()), ("a:b:".into(), "a:b:c".into())]);

    index.place("a:b:c", LeafKind::Hash, &mut added);
    assert!(added.is_empty(), "a key SCAN gives twice is added once");
    index.place("a:b:d", LeafKind::List, &mut added);
    assert_eq!(added.len(), 1);
    assert_eq!(index.keys, 2);
}

#[test]
fn a_name_ending_with_the_delimiter_is_only_a_folder() {
    let index = index_of(&["queue:", "queue:job"]);
    assert_eq!(index.keys, 1);
    assert_eq!(names(index.children("")), [("queue:".to_string(), true)]);
}

#[test]
fn empty_segments_make_their_own_folders() {
    let index = index_of(&["a::b", ":lead"]);
    assert_eq!(names(index.children("a:")), [("a::".to_string(), true)]);
    assert_eq!(names(index.children("")), [(":".to_string(), true), ("a:".to_string(), true)]);
    assert_eq!(names(index.children(":")), [(":lead".to_string(), false)]);
}

#[test]
fn every_key_comes_back_with_its_kind() {
    let mut index = Index::default();
    index.place("h:1", LeafKind::Hash, &mut Vec::new());
    index.place("l", LeafKind::List, &mut Vec::new());
    let mut keys: Vec<(String, bool)> = index.keys().map(|(id, kind)| (id, kind == LeafKind::Hash)).collect();
    keys.sort();
    assert_eq!(keys, [("h:1".to_string(), true), ("l".to_string(), false)]);
}
