use super::*;
use crate::test_support::temp_dir;

fn run(connection_id: &str, text: &str, at: i64) -> HistoryEntry {
    HistoryEntry { connection_id: connection_id.into(), text: text.into(), executed_at: at, duration_ms: 3, ok: true }
}

fn texts(store: &HistoryStore, connection_id: &str) -> Vec<String> {
    store.for_connection(connection_id).into_iter().map(|e| e.text).collect()
}

#[test]
fn runs_come_newest_first_and_a_rerun_moves_back_to_the_top() {
    let mut store = HistoryStore::new(temp_dir("history").join("h.json"));
    store.record(run("a", "q1", 1));
    store.record(run("a", "q2", 2));
    store.record(run("a", "q1", 3));
    assert_eq!(texts(&store, "a"), ["q1", "q2"]);
    assert_eq!(store.for_connection("a")[0].executed_at, 3);
}

#[test]
fn each_connection_keeps_its_own_newest_runs() {
    let mut store = HistoryStore::new(temp_dir("history").join("h.json"));
    store.record(run("b", "kept", 0));
    for i in 0..PER_CONNECTION + 5 {
        store.record(run("a", &format!("q{i}"), i as i64));
    }
    let a = texts(&store, "a");
    assert_eq!(a.len(), PER_CONNECTION);
    assert_eq!(a.first().map(String::as_str), Some(format!("q{}", PER_CONNECTION + 4).as_str()));
    assert_eq!(a.last().map(String::as_str), Some("q5"));
    assert_eq!(texts(&store, "b"), ["kept"], "another connection's runs don't count toward the limit");
    store.record(run("b", "kept", 1));
    assert_eq!(texts(&store, "b"), ["kept"], "the same text on another connection is its own entry");
}

#[test]
fn the_history_survives_a_restart_and_clearing_touches_one_connection() {
    let file = temp_dir("history").join("h.json");
    let mut store = HistoryStore::new(file.clone());
    store.record(run("a", "q1", 1));
    store.record(run("b", "q2", 2));
    store.clear("a");

    let reopened = HistoryStore::new(file);
    assert!(reopened.for_connection("a").is_empty());
    assert_eq!(reopened.for_connection("b"), vec![run("b", "q2", 2)]);
}

#[test]
fn an_unreadable_file_is_set_aside_not_overwritten() {
    let dir = temp_dir("history");
    let file = dir.join("h.json");
    std::fs::write(&file, "not json").unwrap();
    let mut store = HistoryStore::new(file.clone());
    assert!(store.for_connection("a").is_empty());
    store.record(run("a", "q1", 1));
    let kept = std::fs::read_dir(&dir).unwrap().filter_map(Result::ok).filter(|e| e.path() != file).count();
    assert_eq!(kept, 1, "the unreadable file is kept beside the new one");
    assert_eq!(texts(&HistoryStore::new(file), "a"), ["q1"]);
}
