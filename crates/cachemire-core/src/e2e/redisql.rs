//! Redisql scripts run on real keys: hashes and strings of JSON, joined and filtered.

use super::*;
use crate::redisql::field_names;
use crate::redisql::{QueryError, run_script};
use serde_json::{Value, json};

/// Orders by customer in a hash (a JSON object a field), the customers in a string.
fn seed(redis: &mut TestRedis) {
    redis.run(&[
        "HSET",
        "orders:eu",
        "1",
        r#"{"id":1,"customer":"c1","status":"paid","total":10}"#,
        "2",
        r#"{"id":2,"customer":"c2","status":"open","total":25}"#,
        "3",
        r#"{"id":3,"customer":"c1","status":"paid","total":7}"#,
    ]);
    redis.run(&["SET", "customers", r#"[{"cid":"c1","name":"Ada"},{"cid":"c2","name":"Linus"}]"#]);
}

fn sorted_by_id(mut rows: Vec<serde_json::Map<String, Value>>, column: &str) -> Vec<Value> {
    rows.sort_by_key(|row| row.get(column).and_then(Value::as_i64));
    rows.into_iter().map(Value::Object).collect()
}

#[test]
#[ignore = "needs a Redis server: cargo xtask e2e"]
fn a_script_joins_a_hash_with_a_string_and_filters_with_its_variables() {
    let mut redis = TestRedis::claim();
    seed(&mut redis);
    let state = redis.state();

    let script = "var shop = 'eu';\nFROM KEY 'orders:&shop' AS o JOIN KEY 'customers' AS c ON o.customer = c.cid\nWHERE o.status = 'paid'\nSELECT o.id, c.name, o.total";
    let mut results = block_on(run_script(&state, None, script));
    assert_eq!(results.len(), 1);
    let output = results.remove(0).output.unwrap();
    assert!(!output.truncated && !output.sources_truncated);
    assert_eq!(
        sorted_by_id(output.rows, "o.id"),
        [json!({"o.id": 1, "c.name": "Ada", "o.total": 10}), json!({"o.id": 3, "c.name": "Ada", "o.total": 7})]
    );
}

#[test]
#[ignore = "needs a Redis server: cargo xtask e2e"]
fn each_query_of_a_script_gives_its_rows_until_one_names_a_key_that_isnt_there() {
    let mut redis = TestRedis::claim();
    seed(&mut redis);
    let state = redis.state();

    let script = "FROM KEY 'customers' AS c SELECT c.name;\nFROM KEY 'missing' AS m;\nFROM KEY 'customers'";
    let results = block_on(run_script(&state, None, script));
    assert_eq!(results.len(), 2, "the script stops at the error");
    let names: Vec<Value> = results[0].output.as_ref().unwrap().rows.iter().map(|row| row["c.name"].clone()).collect();
    assert_eq!(names, [json!("Ada"), json!("Linus")]);
    match &results[1].output {
        Err(crate::error::Error::Query(QueryError::KeyNotFound(key))) => assert_eq!(key, "missing"),
        other => panic!("not the missing key's error: {:?}", other.as_ref().err()),
    }
    assert_eq!(&script[results[1].error_span.clone().unwrap()], "'missing'", "it points at the key");
}

#[test]
#[ignore = "needs a Redis server: cargo xtask e2e"]
fn the_field_names_a_key_offers_come_from_its_first_records() {
    let mut redis = TestRedis::claim();
    seed(&mut redis);
    redis.run(&["SET", "single", r#"{"a":1,"b":{"c":2}}"#]);
    redis.run(&["RPUSH", "a-list", "x"]);
    let state = redis.state();

    let names = |key: &str| block_on(field_names(&state, None, key)).unwrap();
    assert_eq!(names("orders:eu"), ["id", "customer", "status", "total"], "the hash's values that are objects, in order");
    assert_eq!(names("customers"), ["cid", "name"]);
    assert_eq!(names("single"), ["a", "b"]);
    assert!(names("a-list").is_empty(), "a list holds no records");
    assert!(names("missing").is_empty());
}

#[test]
#[ignore = "needs a Redis server: cargo xtask e2e"]
fn a_hash_field_that_isnt_json_stops_the_query_naming_the_key_and_field() {
    let mut redis = TestRedis::claim();
    redis.run(&["HSET", "broken", "fine", r#"{"a":1}"#, "bad", "not json"]);
    let state = redis.state();

    let results = block_on(run_script(&state, None, "FROM KEY 'broken'"));
    match &results[0].output {
        Err(crate::error::Error::Query(QueryError::FieldJson { key, field, .. })) => {
            assert_eq!((key.as_str(), field.as_str()), ("broken", "bad"))
        }
        other => panic!("not the field's error: {:?}", other.as_ref().err()),
    }
}
