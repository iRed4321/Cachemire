//! Writing: values, hash fields, keys and whole folders.

use super::*;
use crate::explorer::{
    Checked, FolderMove, delete_folder, delete_hash_field, delete_key, key_exists, rename_folder, rename_hash_field, rename_key,
    write_new_hash_field, write_value,
};
use crate::test_support::e2e_state;
use std::collections::HashMap;

#[test]
#[ignore = "needs a Redis server: cargo xtask e2e"]
fn a_string_is_replaced_its_ttl_kept_and_a_hash_field_is_written() {
    let mut redis = TestRedis::claim();
    redis.run(&["SET", "s", "old", "EX", "1000"]);
    redis.run(&["HSET", "h", "f", "old"]);
    let state = redis.state();

    block_on(async {
        write_value(&state, None, &key("s"), None, "new", None).await.unwrap();
        write_value(&state, None, &key("h"), Some("f"), "{\"n\":2}", None).await.unwrap();
        write_value(&state, None, &key("h"), Some("added"), "x", None).await.unwrap();
    });
    assert_eq!(redis.query::<String>(&["GET", "s"]), "new");
    let ttl: i64 = redis.query(&["TTL", "s"]);
    assert!(ttl > 990, "the TTL is kept ({ttl})");
    let hash: HashMap<String, String> = redis.query(&["HGETALL", "h"]);
    assert_eq!(hash, HashMap::from([("f".into(), "{\"n\":2}".into()), ("added".into(), "x".into())]));
}

#[test]
#[ignore = "needs a Redis server: cargo xtask e2e"]
fn a_hash_field_is_added_renamed_and_deleted_but_never_over_another() {
    let mut redis = TestRedis::claim();
    redis.run(&["HSET", "h", "a", "1", "b", "2"]);
    let state = redis.state();

    block_on(async {
        assert!(!write_new_hash_field(&state, None, &key("h"), "a", "other").await.unwrap(), "a is taken");
        assert!(write_new_hash_field(&state, None, &key("h"), "c", "3").await.unwrap());
        assert!(write_new_hash_field(&state, None, &key("fresh"), "first", "1").await.unwrap(), "a new hash is made");

        let taken = rename_hash_field(&state, None, &key("h"), "a", "b", "edited", None).await.unwrap();
        assert_eq!(taken, Checked::Taken, "b is taken");
        let renamed = rename_hash_field(&state, None, &key("h"), "a", "renamed", "edited", None).await.unwrap();
        assert_eq!(renamed, Checked::Done);
        delete_hash_field(&state, None, &key("h"), "c").await.unwrap();
    });
    let hash: HashMap<String, String> = redis.query(&["HGETALL", "h"]);
    assert_eq!(hash, HashMap::from([("b".into(), "2".into()), ("renamed".into(), "edited".into())]), "the refusals changed nothing");
    assert_eq!(redis.query::<String>(&["HGET", "fresh", "first"]), "1");
}

#[test]
#[ignore = "needs a Redis server: cargo xtask e2e"]
fn a_field_is_renamed_in_two_steps_where_scripts_are_refused() {
    let mut redis = TestRedis::claim();
    redis.run(&["HSET", "h", "a", "1", "taken", "2"]);
    let url = acl_user(&mut redis, "no-scripts", &["~*", "+@all", "-@scripting"]);
    let state = e2e_state(&url);

    block_on(async {
        let moved = rename_hash_field(&state, None, &key("h"), "a", "moved", "1+", None).await.unwrap();
        assert_eq!(moved, Checked::Done);
        let taken = rename_hash_field(&state, None, &key("h"), "moved", "taken", "x", None).await.unwrap();
        assert_eq!(taken, Checked::Taken);
    });
    let hash: HashMap<String, String> = redis.query(&["HGETALL", "h"]);
    assert_eq!(hash, HashMap::from([("moved".into(), "1+".into()), ("taken".into(), "2".into())]));
}

#[test]
#[ignore = "needs a Redis server: cargo xtask e2e"]
fn a_key_is_renamed_unless_its_new_name_is_taken_then_deleted() {
    let mut redis = TestRedis::claim();
    redis.run(&["SET", "a", "1", "EX", "1000"]);
    redis.run(&["SET", "b", "2"]);
    let state = redis.state();

    block_on(async {
        assert!(key_exists(&state, None, &key("a")).await.unwrap());
        assert!(!rename_key(&state, None, &key("a"), &key("b")).await.unwrap(), "b is taken");
        assert!(rename_key(&state, None, &key("a"), &key("c")).await.unwrap());
        assert!(!key_exists(&state, None, &key("a")).await.unwrap());
        delete_key(&state, None, &key("b")).await.unwrap();
    });
    assert_eq!(redis.query::<Option<String>>(&["GET", "b"]), None);
    assert_eq!(redis.query::<String>(&["GET", "c"]), "1");
    assert!(redis.query::<i64>(&["TTL", "c"]) > 0, "a renamed key keeps its TTL");
}

#[test]
#[ignore = "needs a Redis server: cargo xtask e2e"]
fn deleting_a_folder_removes_every_key_under_it_and_nothing_else() {
    let mut redis = TestRedis::claim();
    // more than one UNLINK takes
    let pairs = many(1200, |i| vec![format!("tmp:{i}"), "v".into()]);
    redis.run(&with_args(&["MSET"], &pairs));
    redis.run(&["SET", "tmp:deep:er", "v"]);
    redis.run(&["SET", "tmpx", "kept"]);
    redis.run(&["SET", "tm*:kept", "kept"]);
    let state = redis.state();

    let deleted = block_on(delete_folder(&state, None, &key("tmp:"))).unwrap();
    assert_eq!(deleted, 1201);
    let mut left: Vec<String> = redis.query(&["KEYS", "*"]);
    left.sort();
    assert_eq!(left, ["tm*:kept", "tmpx"]);
}

#[test]
#[ignore = "needs a Redis server: cargo xtask e2e"]
fn a_folder_moves_whole_or_not_at_all_when_one_new_name_is_taken() {
    let mut redis = TestRedis::claim();
    let pairs = many(700, |i| vec![format!("old:{i}"), i.to_string()]);
    redis.run(&with_args(&["MSET"], &pairs));
    redis.run(&["HSET", "old:sub:h", "f", "v"]);
    redis.run(&["SET", "new:5", "in the way"]);
    let state = redis.state();

    match block_on(rename_folder(&state, None, &key("old:"), &key("new:"))).unwrap() {
        FolderMove::Taken(taken) => assert_eq!(taken.id(), "new:5"),
        FolderMove::Moved { .. } => panic!("a key was in the way"),
    }
    assert_eq!(redis.query::<i64>(&["DBSIZE"]), 702, "nothing moved");

    redis.run(&["DEL", "new:5"]);
    match block_on(rename_folder(&state, None, &key("old:"), &key("new:"))).unwrap() {
        FolderMove::Moved { moves, failed } => {
            assert!(failed.is_none());
            assert_eq!(moves.len(), 701);
            assert!(moves.iter().any(|(from, to)| from.id() == "old:sub:h" && to.id() == "new:sub:h"));
        }
        FolderMove::Taken(taken) => panic!("{} isn't taken any more", taken.id()),
    }
    assert!(redis.query::<Vec<String>>(&["KEYS", "old:*"]).is_empty());
    assert_eq!(redis.query::<String>(&["GET", "new:699"]), "699");
    assert_eq!(redis.query::<String>(&["HGET", "new:sub:h", "f"]), "v");
}
