use std::collections::BTreeSet;

use super::super::LeafKind;
use super::*;

const KEYS: [&str; 8] = ["app:users:1", "app:users:2", "app:orders:1", "app:orders:2", "app:flag", "cfg", "x:y:z:deep", "x:other"];

fn index_of(ids: &[&str]) -> Index {
    let mut index = Index::default();
    for id in ids {
        index.place(id, LeafKind::Other, &mut Vec::new());
    }
    index
}

fn selection() -> Selection {
    Selection { on: true, ..Default::default() }
}

/// Every key the selection's export would write.
fn exported(selection: &Selection, index: &Index) -> BTreeSet<String> {
    let plan = selection.plan(index, false);
    plan.folders.into_iter().flat_map(|(_, keys)| keys).chain(plan.keys).collect()
}

fn set(ids: &[&str]) -> BTreeSet<String> {
    ids.iter().map(|id| id.to_string()).collect()
}

/// What is ticked never overlaps: no key or folder lies under a ticked folder.
fn assert_no_overlap(selection: &Selection) {
    for folder in &selection.folders {
        let under = |id: &String| id != folder && id.starts_with(folder.as_str());
        assert!(!selection.folders.iter().any(under), "{folder} holds another ticked folder: {:?}", selection.folders);
        assert!(!selection.keys.iter().any(|k| k.starts_with(folder.as_str())), "{folder} holds a ticked key: {:?}", selection.keys);
    }
}

#[test]
fn ticking_a_folder_ticks_every_key_under_it() {
    let index = index_of(&KEYS);
    let mut s = selection();
    s.tick("app:users:", true, &index, &index, false);
    assert_eq!(exported(&s, &index), set(&["app:users:1", "app:users:2"]));
    assert_eq!(s.count(&index), 2);
    assert_eq!((s.check_of("app:users:1", false), s.check_of("app:users:", true)), (1, 1));
    s.refresh_view(&index, false);
    assert_eq!(s.check_of("app:", true), 2, "the folder above shows it is ticked in part");
    assert_eq!(s.check_of("cfg", false), 0);
}

#[test]
fn ticking_every_child_of_a_folder_ticks_the_folder_itself() {
    let index = index_of(&KEYS);
    let mut s = selection();
    s.tick("app:users:1", true, &index, &index, false);
    s.tick("app:users:2", true, &index, &index, false);
    let plan = s.plan(&index, false);
    assert_eq!(plan.folders, [("app:users:".to_string(), vec!["app:users:1".to_string(), "app:users:2".to_string()])]);
    assert!(plan.keys.is_empty());

    for id in ["app:orders:", "app:flag"] {
        s.tick(id, true, &index, &index, false);
    }
    assert_eq!(s.plan(&index, false).folders.len(), 1, "all of app: is ticked: one folder");
    for id in ["cfg", "x:"] {
        s.tick(id, true, &index, &index, false);
    }
    assert_eq!(s.plan(&index, false).folders[0].0, "", "everything is ticked: the whole keyspace");
    assert_eq!(s.count(&index), KEYS.len());
}

#[test]
fn unticking_a_key_of_a_ticked_folder_keeps_everything_else() {
    let index = index_of(&KEYS);
    let mut s = selection();
    s.tick("app:", true, &index, &index, false);
    s.tick("app:users:1", false, &index, &index, false);
    assert_eq!(exported(&s, &index), set(&["app:users:2", "app:orders:1", "app:orders:2", "app:flag"]));
    assert_no_overlap(&s);
    assert_eq!(s.count(&index), 4);
}

#[test]
fn unticking_a_folder_out_of_the_whole_keyspace_keeps_the_rest() {
    let index = index_of(&KEYS);
    let mut s = selection();
    s.select_all(&index, false);
    assert_eq!(s.count(&index), KEYS.len());
    s.tick("x:y:", false, &index, &index, false);
    assert_eq!(exported(&s, &index).len(), KEYS.len() - 1);
    assert!(!exported(&s, &index).contains("x:y:z:deep"));
    assert_no_overlap(&s);
}

#[test]
fn ticking_a_folder_over_ticked_keys_counts_them_once() {
    let index = index_of(&KEYS);
    let mut s = selection();
    s.tick("app:users:1", true, &index, &index, false);
    s.tick("app:", true, &index, &index, false);
    s.tick("app:users:1", true, &index, &index, false);
    assert_eq!(s.count(&index), 5);
    assert_no_overlap(&s);
}

#[test]
fn unticking_a_folder_in_part_unticks_all_of_it() {
    let index = index_of(&KEYS);
    let mut s = selection();
    s.tick("app:users:1", true, &index, &index, false);
    s.tick("app:flag", true, &index, &index, false);
    s.tick("app:", false, &index, &index, false);
    assert!(exported(&s, &index).is_empty());
}

#[test]
fn in_a_search_a_folder_stands_for_the_keys_it_shows() {
    let index = index_of(&KEYS);
    let shown = index_of(&["app:users:1", "app:orders:1"]);
    let mut s = selection();
    s.tick("app:", true, &index, &shown, true);
    assert_eq!(exported(&s, &index), set(&["app:users:1", "app:orders:1"]), "the keys the search hides stay unticked");
    s.refresh_view(&shown, true);
    assert_eq!(s.check_of("app:", true), 1, "all that the search shows of it is ticked");

    s.tick("app:users:1", false, &index, &shown, true);
    s.refresh_view(&shown, true);
    assert_eq!((s.check_of("app:", true), s.check_of("app:users:", true)), (2, 0));
}

#[test]
fn select_all_in_a_search_ticks_only_what_it_shows() {
    let index = index_of(&KEYS);
    let shown = index_of(&["cfg", "x:other"]);
    let mut s = selection();
    s.select_all(&shown, true);
    assert_eq!(exported(&s, &index), set(&["cfg", "x:other"]));
}

#[test]
fn boxes_are_empty_outside_export_mode() {
    let index = index_of(&KEYS);
    let mut s = selection();
    s.select_all(&index, false);
    s.on = false;
    assert_eq!((s.check_of("cfg", false), s.check_of("app:", true)), (0, 0));
    s.clear();
    assert_eq!(s.count(&index), 0);
}

#[test]
fn the_plan_lists_folders_and_lone_keys_in_order() {
    let index = index_of(&KEYS);
    let mut s = selection();
    for id in ["x:other", "cfg", "app:orders:", "app:users:2"] {
        s.tick(id, true, &index, &index, false);
    }
    let plan = s.plan(&index, true);
    assert_eq!(plan.folders, [("app:orders:".to_string(), vec!["app:orders:1".to_string(), "app:orders:2".to_string()])]);
    assert_eq!(plan.keys, ["app:users:2", "cfg", "x:other"]);
    assert!(plan.group);
}

#[test]
fn any_series_of_ticks_ends_on_exactly_the_keys_a_user_would_expect() {
    // the ids a user can tick: every key and every folder
    let index = index_of(&KEYS);
    let mut ids: Vec<String> = KEYS.iter().map(|k| k.to_string()).collect();
    ids.extend(KEYS.iter().flat_map(|k| ancestors(k).map(str::to_string).collect::<Vec<_>>()));
    ids.sort();
    ids.dedup();

    let mut seed: u64 = 0x2545_f491_4f6c_dd1d;
    let mut next = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    for _run in 0..200 {
        let mut s = selection();
        let mut expected: BTreeSet<String> = BTreeSet::new();
        for _step in 0..25 {
            let id = &ids[(next() % ids.len() as u64) as usize];
            let on = next() % 3 != 0;
            s.tick(id, on, &index, &index, false);
            for key in KEYS.iter().filter(|k| if id.ends_with(DELIMITER) { k.starts_with(id.as_str()) } else { **k == id.as_str() }) {
                if on {
                    expected.insert(key.to_string());
                } else {
                    expected.remove(*key);
                }
            }
            assert_eq!(exported(&s, &index), expected, "after ticking {id} {}", if on { "on" } else { "off" });
            assert_eq!(s.count(&index), expected.len());
            assert_no_overlap(&s);
            for key in KEYS {
                assert_eq!(s.check_of(key, false) == 1, expected.contains(key), "{key}'s box");
            }
        }
    }
}

#[test]
fn any_series_of_ticks_in_a_search_ends_on_the_keys_a_user_would_expect() {
    let index = index_of(&KEYS);
    let mut seed: u64 = 0x9e37_79b9_7f4a_7c15;
    let mut next = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    for _run in 0..200 {
        // a search showing some of the keys
        let shown_keys: Vec<&str> = KEYS.iter().copied().filter(|_| next() % 2 == 0).collect();
        if shown_keys.is_empty() {
            continue;
        }
        let shown = index_of(&shown_keys);
        let mut ids: Vec<String> = shown_keys.iter().map(|k| k.to_string()).collect();
        ids.extend(shown_keys.iter().flat_map(|k| ancestors(k).map(str::to_string).collect::<Vec<_>>()));
        ids.sort();
        ids.dedup();

        let mut s = selection();
        let mut expected: BTreeSet<String> = BTreeSet::new();
        for _step in 0..20 {
            if next() % 10 == 0 {
                s.select_all(&shown, true);
                expected.extend(shown_keys.iter().map(|k| k.to_string()));
            } else {
                let id = &ids[(next() % ids.len() as u64) as usize];
                let on = next() % 3 != 0;
                s.tick(id, on, &index, &shown, true);
                let hit = |k: &str| if id.ends_with(DELIMITER) { k.starts_with(id.as_str()) } else { k == id.as_str() };
                for key in shown_keys.iter().filter(|k| hit(k)) {
                    if on {
                        expected.insert(key.to_string());
                    } else {
                        expected.remove(*key);
                    }
                }
            }
            assert_eq!(exported(&s, &index), expected, "shown: {shown_keys:?}");
            assert_eq!(s.count(&index), expected.len());
            assert_no_overlap(&s);
            s.refresh_view(&shown, true);
            for folder in ids.iter().filter(|id| id.ends_with(DELIMITER)) {
                let under: Vec<&&str> = shown_keys.iter().filter(|k| k.starts_with(folder.as_str())).collect();
                let ticked = under.iter().filter(|k| expected.contains(***k)).count();
                let want = if ticked == under.len() {
                    1
                } else if ticked > 0 {
                    2
                } else {
                    0
                };
                assert_eq!(s.check_of(folder, true), want, "{folder}'s box, shown: {shown_keys:?}, ticked: {expected:?}");
            }
        }
    }
}
