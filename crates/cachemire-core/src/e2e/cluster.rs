//! A cluster: reaching it through one node, then browsing, searching and writing keys spread
//! over its slots. The tests share the cluster, each with keys under a prefix of its own.

use super::*;
use crate::connection::connect_connection;
use crate::explorer::{
    Checked, EntriesBatch, FolderMove, NamespaceBatch, count_keys_under, delete_folder, get_key_details, rename_folder, rename_hash_field,
    rename_key, scan_entries, scan_namespaces, write_value,
};
use crate::redis_value_type::RedisValueType;
use crate::server_info::used_memory;
use crate::session::Session;
use crate::test_support::TestCluster;
use std::collections::BTreeMap;
use std::time::Duration;

/// The cluster slot of `name`.
fn slot(cluster: &mut TestCluster, name: &str) -> i64 {
    cluster.query(&["CLUSTER", "KEYSLOT", name])
}

#[test]
#[ignore = "needs a Redis cluster: cargo xtask e2e"]
fn a_cluster_is_reached_through_one_node_and_walked_on_every_primary() {
    let mut cluster = TestCluster::claim("walk");
    for i in 0..300 {
        let name = cluster.key(&format!("k:{i}"));
        cluster.run(&["SET", &name, "v"]);
    }
    let state = std::sync::Arc::new(cluster.state());

    block_on(async {
        connect_connection(&state, "e2e").await.unwrap();
        assert!(state.redis_connections.responds("e2e", Duration::from_secs(3)).await);
        let mut session = Session::open(&state, None).await.unwrap();
        assert!(session.is_cluster());
        assert!(session.dbsize().await.unwrap() >= 300, "the keys of every primary are counted");
        let info = session.info("server").await.unwrap();
        assert!(info.matches("redis_version:").count() >= 3, "one INFO per primary:\n{info}");
        assert!(used_memory(&state, None).await.unwrap() > 0);
        assert_eq!(count_keys_under(&state, None, &key(&cluster.prefix)).await.unwrap(), 300, "every primary is walked");
    });
}

#[test]
#[ignore = "needs a Redis cluster: cargo xtask e2e"]
fn a_level_of_a_cluster_holds_its_folders_and_typed_keys_from_every_slot() {
    let mut cluster = TestCluster::claim("level");
    for i in 0..50 {
        let name = cluster.key(&format!("dir:{i}"));
        cluster.run(&["SET", &name, "v"]);
    }
    let (hash, list, zset) = (cluster.key("h"), cluster.key("l"), cluster.key("z"));
    cluster.run(&["HSET", &hash, "f", "v"]);
    cluster.run(&["RPUSH", &list, "a", "b", "c"]);
    cluster.run(&["ZADD", &zset, "1", "one", "2", "two"]);
    let state = cluster.state();

    let mut batches: Vec<NamespaceBatch> = Vec::new();
    block_on(scan_namespaces(&state, None, &key(&cluster.prefix), ":", || true, |batch| batches.push(batch))).unwrap();
    let folders: Vec<String> = batches.iter().flat_map(|b| &b.folders).map(|f| f.id()).collect();
    let keys: BTreeMap<String, RedisValueType> = batches.into_iter().flat_map(|b| b.keys).map(|k| (k.name.id(), k.key_type)).collect();
    assert_eq!(folders, [cluster.key("dir:")]);
    assert_eq!(keys, BTreeMap::from([(hash, RedisValueType::Hash), (list.clone(), RedisValueType::List), (zset, RedisValueType::Zset)]));

    let details = block_on(get_key_details(&state, None, &key(&list), 100, 100, None, false)).unwrap();
    assert_eq!(details.value, serde_json::json!(["a", "b", "c"]));
}

/// The keys under the test's prefix that a search walks, with their entries.
fn search(state: &crate::state::AppState, prefix: &str, filter: Option<&str>) -> BTreeMap<String, Vec<(String, String)>> {
    let (_pause, paused) = tokio::sync::watch::channel(false);
    let mut batches: Vec<EntriesBatch> = Vec::new();
    block_on(scan_entries(state, None, 100, filter, || true, paused, |batch| batches.push(batch))).unwrap();
    assert!(batches.last().is_some_and(|b| b.done));
    batches.into_iter().flat_map(|b| b.keys).filter(|k| k.key.id().starts_with(prefix)).map(|k| (k.key.id(), k.entries)).collect()
}

#[test]
#[ignore = "needs a Redis cluster: cargo xtask e2e"]
fn a_search_reads_keys_of_every_slot_plainly_or_prefiltered() {
    let mut cluster = TestCluster::claim("search");
    let needle = format!("needle{}", std::process::id());
    for i in 0..20 {
        let name = cluster.key(&format!("note:{i}"));
        cluster.run(&["SET", &name, &format!("a {needle} {i}")]);
    }
    let (hash, other) = (cluster.key("profile"), cluster.key("other"));
    cluster.run(&["HSET", &hash, "bio", &format!("the {needle}")]);
    cluster.run(&["SET", &other, "nothing to see"]);
    let state = cluster.state();

    let plain = search(&state, &cluster.prefix, None);
    assert_eq!(plain.len(), 22, "every key, whichever primary holds it");
    let filtered = search(&state, &cluster.prefix, Some(&needle));
    assert_eq!(filtered.len(), 21, "the server's script drops what can't match: {:?}", filtered.keys());
    assert!(!filtered.contains_key(&other));
    assert_eq!(filtered[&hash], [("bio".to_string(), format!("the {needle}"))]);
}

#[test]
#[ignore = "needs a Redis cluster: cargo xtask e2e"]
fn a_key_is_renamed_to_another_slot_with_its_ttl_unless_the_name_is_taken() {
    let mut cluster = TestCluster::claim("rename");
    let (from, to, taken) = (cluster.key("from"), cluster.key("to"), cluster.key("taken"));
    assert_ne!(slot(&mut cluster, &from), slot(&mut cluster, &to), "the names lie in different slots");
    cluster.run(&["SET", &from, "v", "EX", "1000"]);
    cluster.run(&["SET", &taken, "t"]);
    let state = cluster.state();

    block_on(async {
        assert!(rename_key(&state, None, &key(&from), &key(&to)).await.unwrap());
        assert!(!rename_key(&state, None, &key(&to), &key(&taken)).await.unwrap(), "never over another key");
        let written = write_value(&state, None, &key(&to), None, "new", Some("v")).await.unwrap();
        assert_eq!(written, Checked::Done, "the checked write's script runs on the key's node");
    });
    assert_eq!(cluster.query::<i64>(&["EXISTS", &from]), 0);
    assert_eq!(cluster.query::<String>(&["GET", &to]), "new");
    assert!(cluster.query::<i64>(&["TTL", &to]) > 990, "the TTL moves with the key");
    assert_eq!(cluster.query::<String>(&["GET", &taken]), "t");

    let hash = cluster.key("h");
    cluster.run(&["HSET", &hash, "a", "1"]);
    let renamed = block_on(rename_hash_field(&state, None, &key(&hash), "a", "b", "1", None)).unwrap();
    assert_eq!(renamed, Checked::Done);
    assert_eq!(cluster.query::<Option<String>>(&["HGET", &hash, "b"]).as_deref(), Some("1"));
}

#[test]
#[ignore = "needs a Redis cluster: cargo xtask e2e"]
fn a_folder_spread_over_the_slots_is_moved_then_deleted_whole() {
    let mut cluster = TestCluster::claim("folder");
    for i in 0..40 {
        let name = cluster.key(&format!("old:{i}"));
        cluster.run(&["SET", &name, &i.to_string()]);
    }
    let state = cluster.state();
    let (old, new) = (key(&cluster.key("old:")), key(&cluster.key("new:")));

    block_on(async {
        match rename_folder(&state, None, &old, &new).await.unwrap() {
            FolderMove::Moved { moves, failed } => assert!(moves.len() == 40 && failed.is_none(), "{} moved, {failed:?}", moves.len()),
            FolderMove::Taken(name) => panic!("{} is taken", name.id()),
        }
        assert_eq!(count_keys_under(&state, None, &old).await.unwrap(), 0);
        assert_eq!(count_keys_under(&state, None, &new).await.unwrap(), 40);
        assert_eq!(delete_folder(&state, None, &new).await.unwrap(), 40);
        assert_eq!(count_keys_under(&state, None, &new).await.unwrap(), 0);
    });
    let moved = cluster.key("new:7");
    assert_eq!(cluster.query::<i64>(&["EXISTS", &moved]), 0);
}
