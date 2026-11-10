//! The global search's walk: each key's entries, read plainly or prefiltered by the server.

use super::*;
use crate::explorer::{EntriesBatch, scan_entries};
use crate::redis_value_type::RedisValueType;
use crate::test_support::e2e_state;
use std::collections::BTreeMap;

/// Each key walked, with its type and entries, and the batches' counts as (scanned, total, done).
type Walked = (BTreeMap<String, (RedisValueType, Vec<(String, String)>)>, Vec<(usize, usize, bool)>);

fn search(state: &crate::state::AppState, filter: Option<&str>) -> Walked {
    let (_pause, paused) = tokio::sync::watch::channel(false);
    let mut batches: Vec<EntriesBatch> = Vec::new();
    block_on(scan_entries(state, None, 100, filter, || true, paused, |batch| batches.push(batch))).unwrap();
    let counts = batches.iter().map(|b| (b.scanned, b.total, b.done)).collect();
    let keys = batches.into_iter().flat_map(|b| b.keys).map(|k| (k.key.id(), (k.key_type, k.entries))).collect();
    (keys, counts)
}

/// One key of each type, each holding "needle" once among other entries.
fn seed(redis: &mut TestRedis) {
    redis.run(&["SET", "note", "remember the NEEDLE"]);
    redis.run(&["HSET", "profile", "name", "Ada", "bio", "a needle here"]);
    redis.run(&["RPUSH", "queue", "first", "needle second"]);
    redis.run(&["SADD", "tags", "needle-tag"]);
    redis.run(&["ZADD", "board", "1", "plain", "2", "needle-member"]);
    redis.run(&["XADD", "events", "1-1", "kind", "needle"]);
    redis.run(&["SET", "unrelated", "nothing to see"]);
}

fn entries(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs.iter().map(|(f, v)| (f.to_string(), v.to_string())).collect()
}

#[test]
#[ignore = "needs a Redis server: cargo xtask e2e"]
fn a_plain_walk_reads_every_entry_of_every_type() {
    let mut redis = TestRedis::claim();
    seed(&mut redis);
    let state = redis.state();

    let (keys, counts) = search(&state, None);
    assert_eq!(keys.len(), 7);
    assert_eq!(keys["note"], (RedisValueType::String, entries(&[("", "remember the NEEDLE")])));
    let (kind, mut profile) = keys["profile"].clone();
    profile.sort();
    assert_eq!((kind, profile), (RedisValueType::Hash, entries(&[("bio", "a needle here"), ("name", "Ada")])));
    assert_eq!(keys["queue"].1, entries(&[("[0]", "first"), ("[1]", "needle second")]), "a list's entries are its indexes");
    assert_eq!(keys["tags"].1, entries(&[("", "needle-tag")]));
    assert_eq!(keys["board"].1, entries(&[("1", "plain"), ("2", "needle-member")]), "a sorted set's are its scores");
    assert_eq!(keys["events"].1, entries(&[("1-1", "{\"kind\":\"needle\"}")]), "a stream's are its ids");

    let (scanned, total, done) = *counts.last().unwrap();
    assert!(done && scanned == 7 && total == 7, "{counts:?}");
    assert!(counts.iter().rev().skip(1).all(|(_, _, done)| !done));
}

#[test]
#[ignore = "needs a Redis server: cargo xtask e2e"]
fn the_server_filter_hands_over_only_what_may_match() {
    let mut redis = TestRedis::claim();
    seed(&mut redis);
    redis.run(&["SET", "needle-in-the-name", "x"]);
    redis.run(&["SET", "unicode", "Ünïcode tëxt"]);
    let state = redis.state();

    let (keys, _) = search(&state, Some("needle"));
    let names: Vec<&str> = keys.keys().map(String::as_str).collect();
    assert_eq!(names, ["board", "events", "needle-in-the-name", "note", "profile", "queue", "tags", "unicode"]);
    assert_eq!(keys["profile"].1, entries(&[("bio", "a needle here")]), "the field that can't match is left out");
    assert_eq!(keys["board"].1, entries(&[("2", "needle-member")]));
    assert_eq!(keys["events"].1, entries(&[("1-1", "{\"kind\":\"needle\"}")]), "a stream is read the plain way");
    assert_eq!(keys["note"].1, entries(&[("", "remember the NEEDLE")]), "whatever its case");
    assert!(keys["needle-in-the-name"].1.is_empty(), "a key found by its name alone");
    assert_eq!(keys["unicode"].1.len(), 1, "text that isn't ASCII is left to the app to check");
}

#[test]
#[ignore = "needs a Redis server: cargo xtask e2e"]
fn where_scripts_are_refused_the_search_reads_everything_plainly() {
    let mut redis = TestRedis::claim();
    seed(&mut redis);
    let url = acl_user(&mut redis, "no-scripts", &["~*", "+@all", "-@scripting"]);
    let state = e2e_state(&url);

    let (keys, counts) = search(&state, Some("needle"));
    assert_eq!(keys.len(), 7, "every key, as without the filter");
    assert_eq!(keys["profile"].1.len(), 2);
    assert!(counts.last().unwrap().2);
}

#[test]
#[ignore = "needs a Redis server: cargo xtask e2e"]
fn a_paused_search_waits_and_goes_on_once_resumed() {
    let mut redis = TestRedis::claim();
    let pairs = many(300, |i| vec![format!("k:{i}"), "v".into()]);
    redis.run(&with_args(&["MSET"], &pairs));
    let state = redis.state();

    block_on(async {
        let (pause, paused) = tokio::sync::watch::channel(true);
        let seen = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counted = seen.clone();
        let walk = tokio::spawn(async move {
            scan_entries(
                &state,
                None,
                10,
                None,
                || true,
                paused,
                move |batch| {
                    counted.fetch_add(batch.keys.len(), std::sync::atomic::Ordering::SeqCst);
                },
            )
            .await
        });
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        assert_eq!(seen.load(std::sync::atomic::Ordering::SeqCst), 0, "nothing is read while paused");
        pause.send(false).unwrap();
        walk.await.unwrap().unwrap();
        assert_eq!(seen.load(std::sync::atomic::Ordering::SeqCst), 300);
    });
}
