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
fn values_that_arent_utf8_are_read_lossily() {
    assert_eq!(entries("string", Value::BulkString(b"ok\xff".to_vec())), pairs_of(&[("", "ok\u{fffd}")]));
}

#[test]
fn only_the_types_with_content_are_read() {
    let mut pipe = redis::pipe();
    let key = |kind: &str| TypedKey { name: KeyName::from("k"), key_type: RedisValueType::from(kind) };
    for kind in ["string", "hash", "list", "set", "zset", "stream"] {
        assert!(push_head_read(&mut pipe, &key(kind), 10), "{kind}");
    }
    assert!(!push_head_read(&mut pipe, &key("none"), 10));
    assert!(!push_head_read(&mut pipe, &key("ReJSON-RL"), 10));
}
