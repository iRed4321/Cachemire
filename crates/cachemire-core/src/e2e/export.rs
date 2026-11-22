//! Exporting keys to JSON files: a folder's keys into one file, each lone key into its own,
//! keys gone or refused left out, files already there kept, a cancelled export leaving nothing.

use super::*;
use crate::export_files::{Outcome, Plan, run};
use serde_json::{Value, json};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

fn plan(folders: &[(&str, &[&str])], keys: &[&str]) -> Plan {
    let owned = |list: &[&str]| list.iter().map(|k| k.to_string()).collect::<Vec<_>>();
    Plan { folders: folders.iter().map(|(prefix, keys)| (prefix.to_string(), owned(keys))).collect(), keys: owned(keys), group: false }
}

/// Runs `plan` into `dir`, the progress reported kept.
fn export(state: &crate::state::AppState, plan: Plan, dir: &Path, file_name: Option<&str>) -> (Outcome, Vec<(usize, usize)>) {
    let mut progress = Vec::new();
    let cancel = AtomicBool::new(false);
    let outcome = block_on(run(state, "e2e", plan, dir, file_name, &cancel, |done, total| progress.push((done, total)))).unwrap();
    (outcome, progress)
}

fn read(path: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// The files in `dir`, by name.
fn files(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> =
        std::fs::read_dir(dir).unwrap().map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    names
}

#[test]
#[ignore = "needs a Redis server: cargo xtask e2e"]
fn a_folder_goes_into_one_file_and_a_lone_key_into_its_own() {
    let mut redis = TestRedis::claim();
    redis.run(&["HSET", "user:1", "name", "Ada"]);
    redis.run(&["SET", "user:2", "Linus", "EX", "1000"]);
    redis.run(&["RPUSH", "order:7", "a", "b"]);
    let dir = crate::test_support::temp_dir("export");

    let (outcome, progress) = export(&redis.state(), plan(&[("user:", &["user:1", "user:2"])], &["order:7"]), &dir, None);
    assert_eq!((outcome.keys, outcome.cancelled, outcome.skipped), (3, false, 0));
    assert_eq!(progress, [(1, 3), (2, 3), (3, 3)], "told after each key");
    assert_eq!(files(&dir), ["redis-folder-user_.json", "redis-key-order_7.json"]);

    let folder = read(&dir.join("redis-folder-user_.json"));
    assert_eq!(folder["prefix"], "user:");
    assert_eq!(folder["totalKeys"], 2);
    assert_eq!(folder["items"][0], json!({"key": "user:1", "type": "hash", "ttl": -1, "value": {"name": "Ada"}}));
    let ttl = folder["items"][1]["ttl"].as_i64().unwrap();
    assert!((990..=1000).contains(&ttl), "its time to live: {ttl}");
    assert_eq!(read(&dir.join("redis-key-order_7.json")), json!({"key": "order:7", "type": "list", "ttl": -1, "value": ["a", "b"]}));
}

#[test]
#[ignore = "needs a Redis server: cargo xtask e2e"]
fn keys_gone_before_their_turn_are_left_out_and_write_no_empty_file() {
    let mut redis = TestRedis::claim();
    redis.run(&["SET", "kept", "x"]);
    let dir = crate::test_support::temp_dir("export");
    let (outcome, _) = export(&redis.state(), plan(&[("tmp:", &["tmp:1", "tmp:2"])], &["kept", "ghost"]), &dir, None);
    assert_eq!((outcome.keys, outcome.skipped), (4, 0), "a gone key counts as done, not as refused");
    assert_eq!(files(&dir), ["redis-key-kept.json"]);
}

#[test]
#[ignore = "needs a Redis server: cargo xtask e2e"]
fn a_key_the_user_may_not_read_is_skipped_and_counted() {
    let mut redis = TestRedis::claim();
    redis.run(&["SET", "public:1", "x"]);
    redis.run(&["SET", "secret:1", "y"]);
    let url = acl_user(&mut redis, "export", &["~public:*", "+@all"]);
    let state = crate::test_support::e2e_state(&url);
    let dir = crate::test_support::temp_dir("export");

    let (outcome, _) = export(&state, plan(&[("", &["public:1", "secret:1"])], &[]), &dir, None);
    assert_eq!((outcome.keys, outcome.skipped, outcome.cancelled), (2, 1, false));
    let file = read(&dir.join("redis-folder-root.json"));
    assert_eq!(file["totalKeys"], 1);
    assert_eq!(file["items"][0]["key"], "public:1");
}

#[test]
#[ignore = "needs a Redis server: cargo xtask e2e"]
fn a_file_already_there_is_kept_unless_it_is_the_one_picked() {
    let mut redis = TestRedis::claim();
    redis.run(&["SET", "config", "new"]);
    let state = redis.state();
    let dir = crate::test_support::temp_dir("export");
    std::fs::write(dir.join("redis-key-config.json"), "old").unwrap();
    std::fs::write(dir.join("picked.json"), "old").unwrap();

    // several files: none replaces one already there
    export(&state, plan(&[], &["config", "other"]), &dir, Some("ignored.json"));
    assert_eq!(std::fs::read_to_string(dir.join("redis-key-config.json")).unwrap(), "old");
    assert_eq!(read(&dir.join("redis-key-config (2).json"))["value"], "new");

    // a single file goes where the save dialog said, over what is there
    export(&state, plan(&[], &["config"]), &dir, Some("picked.json"));
    assert_eq!(read(&dir.join("picked.json"))["value"], "new");
    assert!(!dir.join("ignored.json").exists());
}

#[test]
#[ignore = "needs a Redis server: cargo xtask e2e"]
fn a_cancelled_export_stops_and_removes_the_file_it_was_writing() {
    let mut redis = TestRedis::claim();
    for i in 0..5 {
        redis.run(&["SET", &format!("job:{i}"), "x"]);
    }
    let keys: Vec<String> = (0..5).map(|i| format!("job:{i}")).collect();
    let keys: Vec<&str> = keys.iter().map(String::as_str).collect();
    let dir = crate::test_support::temp_dir("export");
    let cancel = AtomicBool::new(false);
    // the user cancels once the second key is written
    let outcome = block_on(run(&redis.state(), "e2e", plan(&[("job:", &keys)], &[]), &dir, None, &cancel, |done, _| {
        if done == 2 {
            cancel.store(true, Ordering::Relaxed);
        }
    }))
    .unwrap();
    assert!(outcome.cancelled);
    assert_eq!(outcome.keys, 2);
    assert!(files(&dir).is_empty(), "no half-written file is left: {:?}", files(&dir));
}

#[test]
#[ignore = "needs a Redis server: cargo xtask e2e"]
fn an_export_to_a_place_that_cant_be_written_fails_with_the_reason() {
    let mut redis = TestRedis::claim();
    redis.run(&["SET", "k", "v"]);
    let dir = crate::test_support::temp_dir("export");
    // a file stands where the folder should be made
    let blocked = dir.join("not-a-folder");
    std::fs::write(&blocked, "x").unwrap();
    let cancel = AtomicBool::new(false);
    let result = block_on(run(&redis.state(), "e2e", plan(&[], &["k"]), &blocked.join("inside"), None, &cancel, |_, _| {}));
    assert!(matches!(result, Err(crate::error::Error::File(_))), "{:?}", result.err().map(|e| e.to_string()));
}
