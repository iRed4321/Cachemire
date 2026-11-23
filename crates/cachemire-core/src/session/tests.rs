use super::*;
use redis::Value;

fn bulk(text: &str) -> Value {
    Value::BulkString(text.as_bytes().to_vec())
}

fn array(items: &[&str]) -> Value {
    Value::Array(items.iter().map(|item| bulk(item)).collect())
}

fn entries(kind: &str, reply: Value) -> Vec<(String, String)> {
    entries_of_reply(&RedisValueType::from(kind), reply)
}

fn pairs_of(list: &[(&str, &str)]) -> Vec<(String, String)> {
    list.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect()
}

#[test]
fn each_types_reply_becomes_its_searchable_entries() {
    assert_eq!(entries("string", bulk("{\"a\":1}")), pairs_of(&[("", "{\"a\":1}")]));
    assert_eq!(entries("hash", Value::Array(vec![bulk("0"), array(&["f1", "v1", "f2", "v2"])])), pairs_of(&[("f1", "v1"), ("f2", "v2")]));
    assert_eq!(entries("list", array(&["x", "y"])), pairs_of(&[("[0]", "x"), ("[1]", "y")]));
    assert_eq!(entries("set", Value::Array(vec![bulk("0"), array(&["m"])])), pairs_of(&[("", "m")]));
    assert_eq!(entries("zset", array(&["alice", "1.5", "bob", "2"])), pairs_of(&[("1.5", "alice"), ("2", "bob")]));
    let stream = Value::Array(vec![Value::Array(vec![bulk("1-0"), array(&["k", "v", "n", "2"])])]);
    assert_eq!(entries("stream", stream), pairs_of(&[("1-0", r#"{"k":"v","n":"2"}"#)]));
}

#[test]
fn a_reply_of_the_wrong_shape_gives_no_entries() {
    // the key was replaced by another type between TYPE and the read
    assert!(entries("hash", array(&["a", "b"])).is_empty());
    assert!(entries("list", Value::Int(3)).is_empty());
    assert!(entries("stream", bulk("x")).is_empty());
    assert!(entries("string", Value::Nil).is_empty());
    assert!(entries("ReJSON-RL", bulk("{}")).is_empty());
}

#[test]
fn an_odd_flat_reply_drops_its_unpaired_last_item() {
    let flat = vec![b"a".to_vec(), b"1".to_vec(), b"b".to_vec()];
    assert_eq!(pairs(flat).collect::<Vec<_>>(), pairs_of(&[("a", "1")]));
}

#[test]
fn values_that_arent_utf8_are_read_with_their_bytes_written_out() {
    assert_eq!(entries("string", Value::BulkString(b"ok\xff".to_vec())), pairs_of(&[("", "ok\\xff")]));
}

#[test]
fn only_the_types_with_content_are_read() {
    let key = |kind: &str| TypedKey { name: KeyName::from("k"), key_type: RedisValueType::from(kind) };
    for kind in ["string", "hash", "list", "set", "zset", "stream"] {
        assert!(head_read(&key(kind), 10).is_some(), "{kind}");
    }
    assert!(head_read(&key("none"), 10).is_none());
    assert!(head_read(&key("ReJSON-RL"), 10).is_none());
}

#[test]
fn a_cluster_is_walked_through_one_slot_of_each_primary() {
    let node = |host: &str, port: i64, id: &str| {
        Value::Array(vec![Value::BulkString(host.into()), Value::Int(port), Value::BulkString(id.into())])
    };
    let range =
        |start: i64, end: i64, primary: Value, replica: Value| Value::Array(vec![Value::Int(start), Value::Int(end), primary, replica]);
    let ranges = vec![
        range(0, 5460, node("10.0.0.1", 7001, "a"), node("10.0.0.4", 7004, "d")),
        range(5461, 10922, node("10.0.0.2", 7002, "b"), node("10.0.0.5", 7005, "e")),
        range(10923, 12000, node("10.0.0.3", 7003, "c"), node("10.0.0.6", 7006, "f")),
        range(12001, 16383, node("10.0.0.3", 7003, "c"), node("10.0.0.6", 7006, "f")),
    ];
    assert_eq!(primary_slots(ranges), [0, 5461, 10923], "a primary serving two ranges is walked once");
}
