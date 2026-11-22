//! Search and query tabs on a real server: a search over every key and a hit opened, Redisql
//! run from the query box with its history; and the last session kept at exit and reopened.

use super::*;
use crate::QueryTab;
use serde_json::json;

/// Keys holding "needle" in a hash field, a string and a key's name, and one that doesn't.
fn haystack(redis: &mut TestRedis) {
    redis.run(&["HSET", "user:1", "name", "Ada", "bio", "loves needlework"]);
    redis.run(&["SET", "note", "a Needle here"]);
    redis.run(&["HSET", "needles:box", "a", "x"]);
    redis.run(&["HSET", "user:2", "name", "Linus"]);
}

/// Opens a search tab and searches it for `text`, until the search is over.
fn search(r: &mut Running, text: &str) {
    if !r.h.window.global::<TabStrip>().get_active_is_search() {
        crate::tabs::open_search(&r.app);
    }
    r.h.window.global::<SearchTab>().set_query(text.into());
    r.h.window.global::<SearchTab>().invoke_submitted(text.into());
    r.until("the search", |h| !h.window.global::<SearchTab>().get_loading());
}

/// The hits listed: (key, row of the key the hit opens).
fn hits(r: &Running) -> Vec<(String, String)> {
    let mut hits: Vec<(String, String)> =
        r.h.window.global::<SearchTab>().get_rows().iter().map(|row| (row.key_name.to_string(), row.target_field.to_string())).collect();
    hits.sort();
    hits
}

fn hit(key: &str, row: &str) -> (String, String) {
    (key.to_string(), row.to_string())
}

#[test]
#[ignore = "needs a Redis server: cargo xtask e2e"]
fn a_search_finds_the_word_in_values_fields_and_key_names_whatever_its_case() {
    let mut r = Running::start(haystack);
    search(&mut r, "needle");
    let found = [hit("needles:box", ""), hit("note", "(value)"), hit("user:1", "bio")];
    assert_eq!(hits(&r), found);
    assert_eq!(r.h.window.global::<SearchTab>().get_status(), "3 matches in 3 keys, of 4 scanned");

    // Redis can leave out what doesn't match first: the hits are the same
    r.app.state.settings.lock().set_server_search(true).unwrap();
    search(&mut r, "NEEDLE");
    assert_eq!(hits(&r), found);
}

#[test]
#[ignore = "needs a Redis server: cargo xtask e2e"]
fn a_search_without_a_hit_says_so_and_an_empty_box_clears_it() {
    let mut r = Running::start(haystack);
    search(&mut r, "haystack");
    let tab = r.h.window.global::<SearchTab>();
    assert!(hits(&r).is_empty());
    assert_eq!(tab.get_status(), "no match for “haystack” in 4 keys");

    search(&mut r, "  ");
    let tab = r.h.window.global::<SearchTab>();
    assert!(!tab.get_searched() && tab.get_status().is_empty(), "back to the empty tab");
}

#[test]
#[ignore = "needs a Redis server: cargo xtask e2e"]
fn a_hit_clicked_opens_its_key_in_a_tab_of_its_own() {
    let mut r = Running::start(haystack);
    search(&mut r, "needle");
    r.h.window.global::<SearchTab>().invoke_hit_opened("user:1".into(), "bio".into());
    r.until("the key", |h| h.window.global::<KeyPanel>().get_key_id() == "user:1");
    let tabs: Vec<(String, bool)> = r.h.window.global::<TabStrip>().get_tabs().iter().map(|t| (t.key.to_string(), t.search)).collect();
    assert_eq!(tabs.len(), 2, "the search tab stays: {tabs:?}");
    assert!(tabs[0].1 && tabs[1].0 == "user:1");
}

/// Customers as a JSON array in a string.
fn customers(redis: &mut TestRedis) {
    redis.run(&["SET", "customers", r#"[{"cid":"c1","name":"Ada"},{"cid":"c2","name":"Linus"}]"#]);
}

/// Opens a query tab holding `text` and runs it: the statement at its end, or all of it.
fn run_query(r: &mut Running, text: &str, whole: bool) {
    if !r.h.window.global::<TabStrip>().get_active_is_query() {
        crate::tabs::open_query(&r.app, text.into());
    }
    let end = text.len() as i32;
    r.h.window.global::<QueryTab>().set_text(text.into());
    r.h.window.global::<QueryTab>().invoke_run_requested(text.into(), end, end, whole);
    r.until("the query", |h| h.window.global::<QueryTab>().get_status() != "running...");
}

fn status(r: &Running) -> (String, bool) {
    let tab = r.h.window.global::<QueryTab>();
    (tab.get_status().to_string(), tab.get_status_is_error())
}

#[test]
#[ignore = "needs a Redis server: cargo xtask e2e"]
fn the_statement_at_the_caret_runs_with_the_variables_declared_above_it() {
    let mut r = Running::start(customers);
    let text = "FROM KEY 'customers' AS c SELECT c.name;\nvar who = 'c2';\nFROM KEY 'customers' AS c WHERE c.cid = '&who' SELECT c.name";
    run_query(&mut r, text, false);
    assert_eq!(status(&r), ("1 rows".to_string(), false));
    assert_eq!(r.h.window.global::<QueryTab>().get_results().row_count(), 0, "one result needs no list");

    // the whole script: a result per query, the last one shown, the others a click away
    run_query(&mut r, text, true);
    let labels: Vec<String> = r.h.window.global::<QueryTab>().get_results().iter().map(|l| l.to_string()).collect();
    assert_eq!(labels, ["1 · 2", "2 · 1"]);
    assert_eq!(status(&r).0, "1 rows");
    r.h.window.global::<QueryTab>().invoke_result_selected(0);
    assert_eq!(status(&r).0, "2 rows");
}

#[test]
#[ignore = "needs a Redis server: cargo xtask e2e"]
fn a_query_that_fails_says_why_and_marks_where() {
    let mut r = Running::start(customers);
    run_query(&mut r, "FROM KEY 'missing' AS m", false);
    let (text, error) = status(&r);
    assert!(error && text.contains("missing"), "{text}");

    let text = "FROM KEY 'customers' AS c ORDER BY x";
    run_query(&mut r, text, false);
    let tab = r.h.window.global::<QueryTab>();
    assert!(tab.get_status_is_error());
    assert_eq!(&text[tab.get_error_start() as usize..tab.get_error_end() as usize], "ORDER", "the box marks the word");

    run_query(&mut r, "  ", false);
    assert_eq!(status(&r), ("The query is empty.".to_string(), true));
}

#[test]
#[ignore = "needs a Redis server: cargo xtask e2e"]
fn the_history_lists_the_runs_newest_first_and_ctrl_up_walks_back_through_them() {
    let mut r = Running::start(customers);
    let first = "FROM KEY 'customers' AS c SELECT c.name";
    let second = "FROM KEY 'missing' AS m";
    run_query(&mut r, first, false);
    run_query(&mut r, second, false);

    let tab = r.h.window.global::<QueryTab>();
    tab.invoke_history_requested();
    let rows: Vec<(String, bool)> = tab.get_history().iter().map(|row| (row.label.to_string(), row.failed)).collect();
    assert_eq!(rows, [(second.to_string(), true), (first.to_string(), false)]);

    let step = |direction: i32, current: &str| tab.invoke_history_step(direction, current.into()).text.to_string();
    assert_eq!(step(-1, "draft"), second);
    assert_eq!(step(-1, second), first);
    assert_eq!(step(-1, first), first, "nothing older: it stays");
    assert_eq!(step(1, first), second);
    assert_eq!(step(1, second), "draft", "past the newest, what was being typed comes back");

    tab.invoke_history_picked(1, false);
    assert_eq!(tab.get_text(), first, "a row picked goes into the box");
    tab.invoke_history_cleared();
    tab.invoke_history_requested();
    assert_eq!(tab.get_history().row_count(), 0);
}

/// The last session's file, next to the settings.
fn session_file(state: &cachemire_core::state::AppState) -> std::path::PathBuf {
    state.settings.lock().file_path().with_file_name("last_session.json")
}

#[test]
#[ignore = "needs a Redis server: cargo xtask e2e"]
fn the_tabs_and_open_folders_are_kept_at_exit_when_the_setting_asks() {
    let mut r = Running::start(|redis| {
        haystack(redis);
        customers(redis);
    });
    r.click_row("user:");
    r.open_key("user:1");
    run_query(&mut r, "FROM KEY 'customers' AS c", false);
    search(&mut r, "ada");
    let file = session_file(&r.app.state);

    crate::last_session::save(&r.app, &r.h.window);
    assert!(!file.exists(), "nothing is kept with the setting off");

    r.app.state.settings.lock().set_restore_session(true).unwrap();
    crate::last_session::save(&r.app, &r.h.window);
    let saved: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    assert_eq!(
        saved,
        json!({
            "connection": "e2e",
            "cloudProfile": "",
            "tabs": [
                {"kind": "key", "name": "user:1"},
                {"kind": "query", "text": "FROM KEY 'customers' AS c"},
                {"kind": "search", "query": "ada"}
            ],
            "active": 2,
            "openFolders": ["user:"]
        })
    );

    crate::last_session::forget(&r.app.state);
    assert!(!file.exists(), "a disconnect drops it");
}

#[test]
#[ignore = "needs a Redis server: cargo xtask e2e"]
fn the_next_start_reconnects_with_the_tabs_and_folders_kept() {
    let kept = json!({
        "connection": "e2e",
        "tabs": [{"kind": "search", "query": "ada"}, {"kind": "key", "name": "user:1"}],
        "active": 1,
        "openFolders": ["user:"]
    });
    let mut r = Running::launch(
        haystack,
        |state| {
            state.settings.lock().set_restore_session(true).unwrap();
            std::fs::write(session_file(state), kept.to_string()).unwrap();
        },
        crate::last_session::restore,
    );
    r.until("the key kept", |h| h.window.global::<KeyPanel>().get_key_id() == "user:1");
    let tabs: Vec<(String, bool)> = r.h.window.global::<TabStrip>().get_tabs().iter().map(|t| (t.key.to_string(), t.active)).collect();
    assert_eq!(tabs.len(), 2);
    assert_eq!(tabs[1], ("user:1".to_string(), true), "the tab that was active is again");
    r.until("the folder", |h| h.window.global::<KeyTree>().get_rows().iter().any(|row| row.full_name == "user:1"));
}
