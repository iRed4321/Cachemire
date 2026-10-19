use serde_json::json;

use super::*;

fn select(query: &str, value: &Value) -> Vec<Value> {
    parse(query).unwrap().select(value).into_iter().cloned().collect()
}

fn sample() -> Value {
    json!({
        "bar": [{"foo": 1, "x": "a"}, {"foo": 2, "x": "b"}, {"other": true}],
        "name": "n",
        "nested": {"deep": {"foo": 9}},
    })
}

#[test]
fn keys_wildcards_and_indexes() {
    let v = sample();
    assert_eq!(select("name", &v), vec![json!("n")]);
    assert_eq!(select("bar.*.foo", &v), vec![json!(1), json!(2)], "items without the key are skipped");
    assert_eq!(select("bar[*].x", &v), vec![json!("a"), json!("b")]);
    assert_eq!(select("bar[1].foo", &v), vec![json!(2)]);
    assert_eq!(select("bar.1.foo", &v), vec![json!(2)], "a numeric key indexes an array");
    assert_eq!(select("bar", &v).len(), 1);
    assert_eq!(select("nested.*", &v), vec![json!({"foo": 9})]);
    assert!(select("bar.7.foo", &v).is_empty());
    assert!(select("missing.foo", &v).is_empty());
    assert!(select("name.foo", &v).is_empty(), "a string has no keys");
}

#[test]
fn root_marker_quoted_keys_and_descent() {
    let v = sample();
    assert_eq!(select("$", &v), vec![v.clone()]);
    assert_eq!(select("$.name", &v), vec![json!("n")]);
    assert_eq!(select("$['name']", &v), vec![json!("n")]);
    assert_eq!(select(".name", &v), vec![json!("n")]);
    assert_eq!(select("..foo", &v), vec![json!(1), json!(2), json!(9)], "at any depth, in document order");
    assert_eq!(select("nested..foo", &v), vec![json!(9)]);
    assert_eq!(select("bar..*", &v).len(), 3 + 2 + 2 + 1, "every value below bar, and bar's own items");
    assert!(parse("$").unwrap().is_identity());
    assert!(!parse("a").unwrap().is_identity());
}

#[test]
fn value_alias_and_trailing_wildcard_helpers() {
    assert!(parse("value").unwrap().is_value_alias_only());
    assert!(!parse("value.a").unwrap().is_value_alias_only());
    assert!(parse("value.a").unwrap().without_value_alias().unwrap() == parse("a").unwrap());
    assert!(parse("value").unwrap().without_value_alias().unwrap().is_identity());
    assert!(parse("a.value").unwrap().without_value_alias().is_none());
    assert!(parse("a.*").unwrap().without_trailing_wildcard().unwrap() == parse("a").unwrap());
    assert!(parse("*").unwrap().without_trailing_wildcard().unwrap().is_identity());
    assert!(parse("a.*.b").unwrap().without_trailing_wildcard().is_none());
    assert!(parse("a..*").unwrap().without_trailing_wildcard().is_none(), "a descent isn't a plain trailing *");
}

#[test]
fn spaces_are_forgiven_and_mistakes_are_explained() {
    let v = sample();
    assert_eq!(select("  bar.*.foo  ", &v), vec![json!(1), json!(2)]);
    assert_eq!(select("bar[ 0 ].foo", &v), vec![json!(1)]);
    for bad in ["bar.", "bar..", ".", "bar[", "bar[x]", "bar[1", "bar['x]", "$name", "bar]", "bar.*.[0"] {
        let error = parse(bad).unwrap_err();
        assert!(!error.to_string().is_empty(), "{bad}");
    }
    assert!(parse("bar.").unwrap_err().to_string().contains("expected a key"));
    assert!(parse("bar[x]").unwrap_err().to_string().contains("after '['"));
}
