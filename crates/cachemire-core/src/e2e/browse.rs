//! Listing the keyspace: a level at a time into folders, and walked whole.

use super::*;
use crate::explorer::{NamespaceBatch, WalkBatch, scan_namespaces, walk_keyspace};
use crate::redis_value_type::RedisValueType;
use std::collections::BTreeMap;

/// One level under `prefix`, its batches put together: the folders, and each key's type.
fn level(state: &crate::state::AppState, prefix: &str) -> (Vec<String>, BTreeMap<String, RedisValueType>) {
    let mut batches: Vec<NamespaceBatch> = Vec::new();
    block_on(scan_namespaces(state, None, &key(prefix), ":", || true, |batch| batches.push(batch))).unwrap();
    let mut folders: Vec<String> = batches.iter().flat_map(|b| b.folders.iter().map(KeyName::id)).collect();
    folders.sort();
    let keys = batches.into_iter().flat_map(|b| b.keys).map(|k| (k.name.id(), k.key_type)).collect();
    (folders, keys)
}

#[test]
#[ignore = "needs a Redis server: cargo xtask e2e"]
fn a_level_holds_the_folders_under_it_once_and_its_own_keys_typed() {
    let mut redis = TestRedis::claim();
    redis.run(&["HSET", "user:1", "name", "Ada"]);
    redis.run(&["SET", "user:2", "Linus"]);
    redis.run(&["SET", "user:admin:root", "x"]);
    redis.run(&["SET", "user:admin:guest", "y"]);
    redis.run(&["RPUSH", "order:7", "a"]);
    redis.run(&["SADD", "solo", "m"]);
    redis.run(&["ZADD", "board", "1", "m"]);
    redis.run(&["XADD", "events", "1-1", "kind", "start"]);
    let state = redis.state();

    let (folders, keys) = level(&state, "");
    assert_eq!(folders, ["order:", "user:"]);
    let root: Vec<(&str, &RedisValueType)> = keys.iter().map(|(k, t)| (k.as_str(), t)).collect();
    assert_eq!(root, [("board", &RedisValueType::Zset), ("events", &RedisValueType::Stream), ("solo", &RedisValueType::Set)]);

    let (folders, keys) = level(&state, "user:");
    assert_eq!(folders, ["user:admin:"], "a folder is listed once, however many keys it holds");
    assert_eq!(keys.into_iter().collect::<Vec<_>>(), [("user:1".into(), RedisValueType::Hash), ("user:2".into(), RedisValueType::String)]);

    let (folders, keys) = level(&state, "user:admin:");
    assert!(folders.is_empty());
    assert_eq!(keys.keys().collect::<Vec<_>>(), ["user:admin:guest", "user:admin:root"]);
}

#[test]
#[ignore = "needs a Redis server: cargo xtask e2e"]
fn a_folder_named_like_a_pattern_lists_only_its_own_keys() {
    let mut redis = TestRedis::claim();
    redis.run(&["SET", "odd*:x", "1"]);
    redis.run(&["SET", "oddball:y", "2"]);
    redis.run(&["SET", "odd[1]:z", "3"]);
    redis.run(&["SET", "odd1:w", "4"]);
    let state = redis.state();

    assert_eq!(level(&state, "odd*:").1.keys().collect::<Vec<_>>(), ["odd*:x"], "the * is a character, not a wildcard");
    assert_eq!(level(&state, "odd[1]:").1.keys().collect::<Vec<_>>(), ["odd[1]:z"]);
}

#[test]
#[ignore = "needs a Redis server: cargo xtask e2e"]
fn a_name_that_isnt_utf8_comes_back_to_the_same_bytes() {
    let mut redis = TestRedis::claim();
    let name: &[u8] = b"bin:\xff\xfe-k";
    redis.query_bytes::<()>(&[b"HSET", name, b"f", b"v"]);
    let state = redis.state();

    let (_, keys) = level(&state, "bin:");
    let (id, kind) = keys.into_iter().next().expect("the key is listed");
    assert_eq!(id, "bin:\\xff\\xfe-k", "its id writes the bytes that aren't UTF-8");
    assert_eq!(kind, RedisValueType::Hash);
    assert_eq!(key(&id).as_bytes(), name);
    let details = block_on(crate::explorer::get_key_details(&state, None, &key(&id), 100, 100, None, false)).unwrap();
    assert_eq!(details.value, serde_json::json!({"f": "v"}), "the key is read by its exact name");
}

/// Every batch of a walk of the database.
fn walk(state: &crate::state::AppState, max_keys: usize) -> Vec<WalkBatch> {
    let mut batches = Vec::new();
    block_on(walk_keyspace(state, None, max_keys, || true, |batch| batches.push(batch))).unwrap();
    batches
}

#[test]
#[ignore = "needs a Redis server: cargo xtask e2e"]
fn a_walk_goes_over_every_key_page_by_page_and_says_when_it_is_done() {
    let mut redis = TestRedis::claim();
    let pairs = many(2500, |i| vec![format!("k:{i}"), "v".into()]);
    redis.run(&with_args(&["MSET"], &pairs));
    redis.run(&["HSET", "a-hash", "f", "v"]);
    let state = redis.state();

    let batches = walk(&state, 100_000);
    assert!(batches.len() > 1, "a page at a time");
    let (last, earlier) = batches.split_last().unwrap();
    assert!(last.done && !last.capped && earlier.iter().all(|b| !b.done));
    assert!(batches.iter().all(|b| b.total == 2501), "measured against DBSIZE");
    assert!(batches.windows(2).all(|w| w[0].scanned <= w[1].scanned));
    let mut names: Vec<String> = batches.iter().flat_map(|b| b.keys.iter().map(|k| k.name.id())).collect();
    names.sort();
    names.dedup();
    assert_eq!(names.len(), 2501, "every key, once duplicates are dropped");
    let hash = batches.iter().flat_map(|b| &b.keys).find(|k| k.name.id() == "a-hash").unwrap();
    assert_eq!(hash.key_type, RedisValueType::Hash);
}

#[test]
#[ignore = "needs a Redis server: cargo xtask e2e"]
fn a_walk_stops_once_it_has_its_fill_of_keys_or_is_called_off() {
    let mut redis = TestRedis::claim();
    let pairs = many(3000, |i| vec![format!("k:{i}"), "v".into()]);
    redis.run(&with_args(&["MSET"], &pairs));
    let state = redis.state();

    let batches = walk(&state, 1000);
    let last = batches.last().unwrap();
    assert!(last.done && last.capped, "it says it stopped short");
    assert!(last.scanned >= 1000 && last.scanned < 3000);

    let mut called = 0;
    block_on(walk_keyspace(&state, None, 100_000, || false, |_| called += 1)).unwrap();
    assert_eq!(called, 0, "a walk no longer wanted hands nothing over");
}

#[test]
#[ignore = "needs a Redis server: cargo xtask e2e"]
fn a_key_the_acl_hides_is_still_listed_with_an_unknown_type() {
    let mut redis = TestRedis::claim();
    redis.run(&["SET", "public:1", "x"]);
    redis.run(&["SET", "secret:1", "y"]);
    let url = acl_user(&mut redis, "public", &["~public:*", "+@all"]);
    let state = crate::test_support::e2e_state(&url);

    let mut keys: Vec<(String, RedisValueType)> =
        walk(&state, 100_000).into_iter().flat_map(|b| b.keys).map(|k| (k.name.id(), k.key_type)).collect();
    keys.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(keys, [("public:1".into(), RedisValueType::String), ("secret:1".into(), RedisValueType::Other(String::new()))]);
}
