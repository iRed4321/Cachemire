//! Reading one key: every type, big keys a part at a time, a hash a page at a time.

use super::*;
use crate::error::{Error, NotFound};
use crate::explorer::{
    HASH_NAMES_BATCH, HashValues, MAX_FIELD_BYTES, MAX_STRING_BYTES, get_key_details, hash_field_page, hash_full_values, hash_more_names,
    read_for_export,
};
use crate::redis_value_type::RedisValueType;
use crate::session::Session;
use serde_json::json;

fn details(state: &crate::state::AppState, id: &str, max_items: usize) -> crate::explorer::KeyDetails {
    block_on(get_key_details(state, None, &key(id), max_items, 100, None, false)).unwrap()
}

#[test]
#[ignore = "needs a Redis server: cargo xtask e2e"]
fn each_type_is_read_with_its_ttl() {
    let mut redis = TestRedis::claim();
    redis.run(&["SET", "s", "{\"a\":1}", "EX", "1000"]);
    redis.run(&["RPUSH", "l", "x", "y"]);
    redis.run(&["SADD", "set", "only"]);
    redis.run(&["ZADD", "z", "2", "b", "1.5", "a"]);
    redis.run(&["XADD", "st", "5-1", "kind", "start", "n", "1"]);
    redis.run(&["HSET", "h", "b", "2", "a", "1"]);
    let state = redis.state();

    let s = details(&state, "s", 100);
    assert_eq!((s.key_type, s.value, s.truncated), (RedisValueType::String, json!("{\"a\":1}"), false));
    assert!(s.ttl > 990 && s.ttl <= 1000, "{}", s.ttl);
    assert_eq!(details(&state, "l", 100).value, json!(["x", "y"]));
    assert_eq!(details(&state, "set", 100).value, json!(["only"]));
    assert_eq!(details(&state, "z", 100).value, json!([["a", 1.5], ["b", 2.0]]), "by score, with it");
    let stream = details(&state, "st", 100);
    assert_eq!(stream.value, json!([{"id": "5-1", "values": {"kind": "start", "n": "1"}}]));
    let hash = details(&state, "h", 100);
    assert_eq!(hash.ttl, -1, "no TTL");
    assert_eq!(hash.value, json!({"a": "1", "b": "2"}));
    assert_eq!(hash.all_field_names, Some(vec!["a".to_string(), "b".to_string()]), "sorted by name");
    assert_eq!((hash.field_count, hash.names_cursor), (2, None));

    let missing = block_on(get_key_details(&state, None, &key("gone"), 100, 100, None, false));
    assert!(matches!(missing, Err(Error::NotFound(NotFound::Key))));
}

#[test]
#[ignore = "needs a Redis server: cargo xtask e2e"]
fn a_big_key_is_cut_at_the_number_of_items_asked_for_in_its_order() {
    let mut redis = TestRedis::claim();
    let items = many(2500, |i| vec![format!("item-{i:04}")]);
    redis.run(&with_args(&["RPUSH", "list"], &items));
    let members = many(2500, |i| vec![i.to_string(), format!("m-{i:04}")]);
    redis.run(&with_args(&["ZADD", "zset"], &members));
    redis.run(&with_args(&["SADD", "set"], &items));
    for i in 0..2500 {
        redis.run(&["XADD", "stream", &format!("{}-0", i + 1), "n", &i.to_string()]);
    }
    let state = redis.state();

    // past the 1000 entries one command reads
    let list = details(&state, "list", 2200);
    let list_items = list.value.as_array().unwrap();
    assert!(list.truncated && list_items.len() == 2200);
    assert_eq!((&list_items[0], &list_items[1999], &list_items[2199]), (&json!("item-0000"), &json!("item-1999"), &json!("item-2199")));
    let zset = details(&state, "zset", 2200);
    assert!(zset.truncated && zset.value.as_array().unwrap().len() == 2200);
    assert_eq!(zset.value[1500], json!(["m-1500", 1500.0]));
    let stream = details(&state, "stream", 2200);
    let entries = stream.value.as_array().unwrap();
    assert!(stream.truncated && entries.len() == 2200);
    assert_eq!((&entries[1000]["id"], &entries[2199]["id"]), (&json!("1001-0"), &json!("2200-0")), "the pages follow each other");
    let set = details(&state, "set", 300);
    assert!(set.truncated && set.value.as_array().unwrap().len() == 300);

    let whole = details(&state, "list", 5000);
    assert!(!whole.truncated && whole.value.as_array().unwrap().len() == 2500);
}

#[test]
#[ignore = "needs a Redis server: cargo xtask e2e"]
fn an_export_reads_a_key_whole_and_nothing_once_it_is_gone() {
    let mut redis = TestRedis::claim();
    for i in 0..2500 {
        redis.run(&["XADD", "stream", &format!("{}-{}", 7, i), "n", &i.to_string()]);
    }
    redis.run(&["SET", "s", "v", "EX", "500"]);
    let state = redis.state();

    block_on(async {
        let mut session = Session::open(&state, None).await.unwrap();
        let exported = read_for_export(&mut session, &key("stream")).await.unwrap().unwrap();
        let entries = exported.value.as_array().unwrap();
        assert!(!exported.truncated && entries.len() == 2500, "past the pages of 1000 entries");
        assert_eq!((&entries[0]["id"], &entries[2499]["id"]), (&json!("7-0"), &json!("7-2499")));
        assert_eq!(exported.ttl, -1);

        let string = read_for_export(&mut session, &key("s")).await.unwrap().unwrap();
        assert!(string.ttl > 0 && string.ttl <= 500);
        assert!(read_for_export(&mut session, &key("gone")).await.unwrap().is_none());
    });
}

#[test]
#[ignore = "needs a Redis server: cargo xtask e2e"]
fn a_string_past_the_limit_is_cut_and_marked() {
    let mut redis = TestRedis::claim();
    let big = "x".repeat(MAX_STRING_BYTES + 10);
    redis.run(&["SET", "big", &big]);
    let state = redis.state();

    let read = details(&state, "big", 100);
    assert!(read.truncated);
    assert_eq!(read.value.as_str().unwrap().len(), MAX_STRING_BYTES);
}

#[test]
#[ignore = "needs a Redis server: cargo xtask e2e"]
fn a_hash_page_is_the_one_holding_the_field_asked_for_in_either_order() {
    let mut redis = TestRedis::claim();
    let fields = many(30, |i| vec![format!("f{}", i + 1), format!("v{}", i + 1)]);
    redis.run(&with_args(&["HSET", "h"], &fields));
    let state = redis.state();
    let page =
        |focus: Option<&str>, descending: bool| block_on(get_key_details(&state, None, &key("h"), 100, 10, focus, descending)).unwrap();

    let first = page(None, false);
    assert_eq!(first.page_start, 0);
    assert_eq!(first.value.as_object().unwrap().keys().collect::<Vec<_>>(), ["f1", "f2", "f3", "f4", "f5", "f6", "f7", "f8", "f9", "f10"]);
    assert_eq!(first.field_count, 30);

    let around = page(Some("f25"), false);
    assert_eq!(around.page_start, 20);
    assert_eq!(around.value.as_object().unwrap().keys().next().map(String::as_str), Some("f21"));
    assert!(around.value.get("f25").is_some());

    let descending = page(Some("f25"), true);
    assert_eq!(descending.page_start, 0, "f25 is the sixth from the end");
    assert_eq!(descending.value.as_object().unwrap().keys().next().map(String::as_str), Some("f30"));

    let names = ["f2".to_string(), "f29".to_string(), "nope".to_string()];
    let HashValues { values, oversized, .. } = block_on(hash_field_page(&state, None, &key("h"), &names)).unwrap();
    assert_eq!(serde_json::Value::Object(values), json!({"f2": "v2", "f29": "v29"}), "a field that is gone is left out");
    assert!(oversized.is_empty());
}

#[test]
#[ignore = "needs a Redis server: cargo xtask e2e"]
fn a_field_too_big_for_the_page_is_given_by_its_size_and_read_on_demand() {
    let mut redis = TestRedis::claim();
    let big = "y".repeat(MAX_FIELD_BYTES + 1);
    redis.run(&["HSET", "h", "small", "s", "big", &big]);
    let state = redis.state();

    let read = details(&state, "h", 100);
    assert_eq!(read.value, json!({"small": "s"}));
    assert_eq!(read.oversized.get("big"), Some(&(MAX_FIELD_BYTES + 1)));
    let full = block_on(hash_full_values(&state, None, &key("h"), &["big".into(), "gone".into()])).unwrap();
    assert_eq!(full[0].as_ref().map(|(text, _)| text.len()), Some(MAX_FIELD_BYTES + 1));
    assert_eq!(full[1], None);
}

#[test]
#[ignore = "needs a Redis server: cargo xtask e2e"]
fn a_huge_hash_names_its_fields_a_batch_at_a_time_and_finds_the_one_asked_for() {
    let mut redis = TestRedis::claim();
    let total = HASH_NAMES_BATCH + 2000;
    let fields = many(total, |i| vec![format!("field-{i:05}"), "v".into()]);
    redis.run(&with_args(&["HSET", "big"], &fields));
    let state = redis.state();

    let first = details(&state, "big", 100);
    let mut names = first.all_field_names.clone().unwrap();
    let cursor = first.names_cursor.expect("names are left to read");
    assert!(names.len() >= HASH_NAMES_BATCH && names.len() < total);
    assert_eq!(first.field_count, total);

    let (more, next) = block_on(hash_more_names(&state, None, &key("big"), cursor, false)).unwrap();
    assert!(more.windows(2).all(|w| w[0] < w[1]), "each batch is sorted");
    let mut cursor = next;
    names.extend(more);
    while let Some(at) = cursor {
        let (more, next) = block_on(hash_more_names(&state, None, &key("big"), at, false)).unwrap();
        names.extend(more);
        cursor = next;
    }
    names.sort();
    names.dedup();
    assert_eq!(names.len(), total, "every field, once");

    let unread = (0..total).map(|i| format!("field-{i:05}")).find(|f| !first.all_field_names.as_ref().unwrap().contains(f)).unwrap();
    let focused = block_on(get_key_details(&state, None, &key("big"), 100, 50, Some(&unread), false)).unwrap();
    assert!(focused.all_field_names.unwrap().contains(&unread), "a field past the first batch is added when asked for");
    assert!(focused.value.get(&unread).is_some(), "and its page is the one read");
}
