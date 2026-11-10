//! The keyspace tree on a real server: the walk filling it, and its right-click menu's
//! Add key, Rename and Delete writing to Redis.

use super::*;
use crate::KeyNameDialog;

/// A few keys in two folders, and one at the root.
fn shop(redis: &mut TestRedis) {
    redis.run(&["HSET", "user:1", "name", "Ada"]);
    redis.run(&["SET", "user:2", "Linus"]);
    redis.run(&["SET", "user:admin:root", "x"]);
    redis.run(&["RPUSH", "order:7", "a", "b"]);
    redis.run(&["SADD", "solo", "m"]);
}

/// Types `text` in the name dialog, then presses its Create or Rename button.
fn submit(r: &mut Running, text: &str) {
    r.h.window.global::<KeyNameDialog>().set_text(text.into());
    // (the dialog clears its last error once it sees the new text)
    r.h.frame();
    r.h.window.global::<KeyNameDialog>().invoke_submit(text.into());
    r.until("the dialog's write", |h| !h.window.global::<KeyNameDialog>().get_busy());
}

fn dialog_error(r: &Running) -> String {
    r.h.window.global::<KeyNameDialog>().get_error().to_string()
}

#[test]
#[ignore = "needs a Redis server: cargo xtask e2e"]
fn connecting_walks_the_keyspace_into_folders_that_open_on_a_click() {
    let mut r = Running::start(shop);
    assert_eq!(r.rows(), ["order:", "user:", "solo"], "folders first");
    assert_eq!(r.h.window.global::<KeyTree>().get_footer(), "5 keys scanned");

    r.click_row("user:");
    assert_eq!(r.rows(), ["order:", "user:", "user:admin:", "user:1", "user:2", "solo"]);
    r.click_row("user:admin:");
    assert!(r.rows().contains(&"user:admin:root".to_string()));
    r.click_row("user:");
    assert_eq!(r.rows(), ["order:", "user:", "solo"], "closing a folder hides what is under it");

    r.open_key("solo");
    assert_eq!(r.h.window.global::<KeyPanel>().get_badge(), "SET");
    assert_eq!(r.h.window.global::<TabStrip>().get_tabs().iter().map(|t| t.key.to_string()).collect::<Vec<_>>(), ["solo"]);
}

#[test]
#[ignore = "needs a Redis server: cargo xtask e2e"]
fn the_refresh_button_finds_what_another_client_wrote() {
    let mut r = Running::start(shop);
    r.redis.run(&["SET", "late", "x"]);
    r.redis.run(&["DEL", "solo"]);
    r.h.window.global::<KeyTree>().invoke_refresh_requested();
    r.until("the new walk", |h| h.window.global::<KeyTree>().get_footer() == "5 keys scanned");
    r.until("the rows", |h| h.window.global::<KeyTree>().get_rows().iter().any(|row| row.full_name == "late"));
    assert_eq!(r.rows(), ["order:", "user:", "late"]);
}

#[test]
#[ignore = "needs a Redis server: cargo xtask e2e"]
fn a_key_added_from_a_folder_opens_empty_and_is_stored_with_its_first_field() {
    let mut r = Running::start(shop);
    r.h.window.global::<KeyTree>().invoke_add_key_requested("user:1".into());
    let dialog = r.h.window.global::<KeyNameDialog>();
    assert!(dialog.get_open());
    assert_eq!(dialog.get_text(), "user:", "the name starts in the row's folder");

    submit(&mut r, "user:1");
    assert_eq!(dialog_error(&r), "A key with this name already exists");
    r.redis.run(&["SET", "user:9", "written since the walk"]);
    submit(&mut r, "user:9");
    assert_eq!(dialog_error(&r), "A key with this name already exists", "Redis is asked too, not only the tree");
    submit(&mut r, "user:3");
    assert!(!r.h.window.global::<KeyNameDialog>().get_open());
    r.until("the new key's tab", |h| h.window.global::<KeyPanel>().get_key_id() == "user:3");
    assert!(r.rows().contains(&"user:3".to_string()), "shown in its folder, opened");
    assert_eq!(r.redis.query::<i64>(&["EXISTS", "user:3"]), 0, "an empty hash isn't in Redis yet");

    r.h.window.global::<KeyPanel>().invoke_add_field();
    let editor = r.h.window.global::<ValueEditor>();
    assert!(editor.get_open() && editor.get_creating());
    editor.invoke_save("\"hello\"".into(), "greeting".into());
    r.editor_closes();
    assert_eq!(r.redis.query::<String>(&["HGET", "user:3", "greeting"]), "hello");
    r.until("the key read again", |h| h.window.global::<KeyPanel>().get_all_values_rows().row_count() == 1);
    assert_eq!(r.fields(), ["greeting"]);
}

#[test]
#[ignore = "needs a Redis server: cargo xtask e2e"]
fn renaming_a_key_moves_it_unless_the_name_is_taken() {
    let mut r = Running::start(shop);
    r.click_row("user:");
    r.open_key("user:2");
    r.h.window.global::<KeyTree>().invoke_rename_requested("user:2".into());
    assert_eq!(r.h.window.global::<KeyNameDialog>().get_text(), "user:2");

    submit(&mut r, "user:1");
    assert_eq!(dialog_error(&r), "A key with this name already exists");
    assert_eq!(r.redis.query::<String>(&["GET", "user:2"]), "Linus", "nothing moved");

    submit(&mut r, "user:22");
    assert!(!r.h.window.global::<KeyNameDialog>().get_open());
    assert_eq!(r.redis.query::<String>(&["GET", "user:22"]), "Linus");
    assert_eq!(r.redis.query::<i64>(&["EXISTS", "user:2"]), 0);
    let rows = r.rows();
    assert!(rows.contains(&"user:22".to_string()) && !rows.contains(&"user:2".to_string()));
    let tabs: Vec<String> = r.h.window.global::<TabStrip>().get_tabs().iter().map(|t| t.key.to_string()).collect();
    assert_eq!(tabs, ["user:22"], "its tab follows it");
}

#[test]
#[ignore = "needs a Redis server: cargo xtask e2e"]
fn renaming_a_folder_moves_every_key_under_it() {
    let mut r = Running::start(shop);
    r.h.window.global::<KeyTree>().invoke_rename_requested("user:".into());
    assert_eq!(r.h.window.global::<KeyNameDialog>().get_text(), "user");

    submit(&mut r, "member");
    assert!(!r.h.window.global::<KeyNameDialog>().get_open(), "{}", dialog_error(&r));
    let mut keys: Vec<String> = r.redis.query(&["KEYS", "member:*"]);
    keys.sort();
    assert_eq!(keys, ["member:1", "member:2", "member:admin:root"]);
    assert!(r.redis.query::<Vec<String>>(&["KEYS", "user:*"]).is_empty());
    assert_eq!(r.rows(), ["member:", "order:", "solo"]);

    r.redis.run(&["SET", "order:1", "in the way"]);
    r.h.window.global::<KeyTree>().invoke_rename_requested("member:".into());
    submit(&mut r, "order");
    assert_eq!(dialog_error(&r), "The key order:1 already exists");
    assert_eq!(r.redis.query::<i64>(&["EXISTS", "member:1"]), 1, "nothing moved");
}

#[test]
#[ignore = "needs a Redis server: cargo xtask e2e"]
fn deleting_a_key_or_a_folder_removes_it_from_redis_and_the_tree() {
    let mut r = Running::start(shop);
    r.open_key("solo");
    r.h.window.global::<KeyTree>().invoke_delete_requested("solo".into());
    r.until("the key to go", |h| !h.window.global::<KeyTree>().get_rows().iter().any(|row| row.full_name == "solo"));
    assert_eq!(r.redis.query::<i64>(&["EXISTS", "solo"]), 0);
    assert_eq!(r.h.window.global::<TabStrip>().get_tabs().row_count(), 0, "its tab closes");

    r.h.window.global::<KeyTree>().invoke_delete_requested("user:".into());
    r.until("the folder to go", |h| !h.window.global::<KeyTree>().get_rows().iter().any(|row| row.full_name == "user:"));
    assert!(r.redis.query::<Vec<String>>(&["KEYS", "user:*"]).is_empty());
    assert_eq!(r.rows(), ["order:"]);
    assert_eq!(r.redis.query::<i64>(&["DBSIZE"]), 1);
}
