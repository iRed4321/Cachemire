use super::*;

fn obj(pairs: &[(&str, Value)]) -> Map<String, Value> {
    pairs.iter().map(|(k, v)| (k.to_string(), v.clone())).collect()
}

fn row(pairs: &[(&str, Map<String, Value>)]) -> Row {
    pairs.iter().map(|(a, r)| (a.to_string(), r.clone())).collect()
}

#[test]
fn select_star_preserves_key_order_for_one_source() {
    let r = row(&[("src", obj(&[("id", Value::from(1)), ("label", Value::from("Widget"))]))]);
    let materialized = materialize_row(&r, &[Select { alias: "*".to_string(), field: "*".to_string(), alias_span: 0..0, explicit_alias: true }]);
    assert_eq!(materialized.keys().collect::<Vec<_>>(), vec!["id", "label"]);
}

#[test]
fn columns_are_prefixed_once_more_than_one_alias_is_involved() {
    let r = row(&[("o", obj(&[("id", Value::from(1))])), ("p", obj(&[("label", Value::from("Widget"))]))]);
    let selects = vec![
        Select { alias: "o".to_string(), field: "id".to_string(), alias_span: 0..0, explicit_alias: true },
        Select { alias: "p".to_string(), field: "label".to_string(), alias_span: 0..0, explicit_alias: true },
    ];
    let materialized = materialize_row(&r, &selects);
    assert_eq!(materialized.get("o.id"), Some(&Value::from(1)));
    assert_eq!(materialized.get("p.label"), Some(&Value::from("Widget")));
}

#[test]
fn a_missing_alias_in_select_becomes_a_null_column() {
    let r = row(&[("o", obj(&[("id", Value::from(1))]))]);
    let selects = vec![Select { alias: "p".to_string(), field: "label".to_string(), alias_span: 0..0, explicit_alias: true }];
    assert_eq!(materialize_row(&r, &selects).get("p.label"), Some(&Value::Null));
}

/// `query` run on `keys` (each key's records, by key name), as the column-named rows it gives.
fn query(text: &str, keys: &[(&str, Value)]) -> (Vec<Value>, bool) {
    let query = super::super::parse_query(text).unwrap();
    let records = |key: &str| keys.iter().find(|(k, _)| *k == key).map(|(_, v)| string_records(key, &v.to_string()).unwrap()).unwrap_or_default();
    let mut datasets = FxHashMap::default();
    datasets.insert(query.base.alias.to_lowercase(), records(&query.base.key));
    for join in &query.joins {
        datasets.insert(join.alias.to_lowercase(), records(&join.key));
    }
    let (rows, truncated) = run(&query, &datasets);
    (rows.into_iter().map(Value::Object).collect(), truncated)
}

fn orders() -> Value {
    serde_json::json!([
        {"id": 1, "customer": "c1", "status": "paid", "total": 10},
        {"id": 2, "customer": "c2", "status": "open", "total": 25},
        {"id": 3, "customer": "c1", "status": "paid", "total": 7},
        {"id": 4, "customer": null, "status": "paid", "total": 1},
    ])
}

fn customers() -> Value {
    serde_json::json!([{"cid": "c1", "name": "Ada"}, {"cid": "c2", "name": "Linus"}, {"cid": "c3", "name": "Nobody"}])
}

#[test]
fn a_join_pairs_each_row_with_every_match_and_drops_the_rest() {
    let (rows, _) = query("FROM KEY 'orders' AS o JOIN KEY 'customers' AS c ON o.customer = c.cid SELECT o.id, c.name", &[("orders", orders()), ("customers", customers())]);
    assert_eq!(
        rows,
        [
            serde_json::json!({"o.id": 1, "c.name": "Ada"}),
            serde_json::json!({"o.id": 2, "c.name": "Linus"}),
            serde_json::json!({"o.id": 3, "c.name": "Ada"}),
        ],
        "the order with no customer matches nothing, the customer with no order is left out"
    );
}

#[test]
fn a_join_matches_a_number_with_its_text() {
    let left = serde_json::json!([{"ref": 42}, {"ref": "7"}, {"ref": true}]);
    let right = serde_json::json!([{"id": "42", "v": "a"}, {"id": 7, "v": "b"}, {"id": "true", "v": "c"}]);
    let (rows, _) = query("FROM KEY 'l' AS l JOIN KEY 'r' AS r ON l.ref = r.id SELECT r.v", &[("l", left), ("r", right)]);
    assert_eq!(rows, [serde_json::json!({"r.v": "a"}), serde_json::json!({"r.v": "b"}), serde_json::json!({"r.v": "c"})]);
}

#[test]
fn a_second_join_can_link_to_the_first_joined_key() {
    let items = serde_json::json!([{"order": 1, "sku": "s1"}, {"order": 3, "sku": "s2"}]);
    let products = serde_json::json!([{"sku": "s1", "label": "Pen"}, {"sku": "s2", "label": "Ink"}]);
    let (rows, _) = query(
        "FROM KEY 'orders' AS o JOIN KEY 'items' AS i ON o.id = i.order JOIN KEY 'products' AS p ON i.sku = p.sku SELECT o.id, p.label",
        &[("orders", orders()), ("items", items), ("products", products)],
    );
    assert_eq!(rows, [serde_json::json!({"o.id": 1, "p.label": "Pen"}), serde_json::json!({"o.id": 3, "p.label": "Ink"})]);
}

#[test]
fn where_keeps_the_rows_every_condition_holds_for() {
    let (rows, _) = query("FROM KEY 'orders' WHERE status = paid AND customer IN ['c1', 'c9'] SELECT id", &[("orders", orders())]);
    assert_eq!(rows, [serde_json::json!({"id": 1}), serde_json::json!({"id": 3})]);
    let (rows, _) = query("FROM KEY 'orders' WHERE STATUS = 'open' SELECT id", &[("orders", orders())]);
    assert_eq!(rows, [serde_json::json!({"id": 2})], "field names match whatever their case");
    let (rows, _) = query("FROM KEY 'orders' WHERE customer = null", &[("orders", orders())]);
    assert!(rows.is_empty(), "a null value matches nothing");
}

#[test]
fn where_on_a_joined_key_filters_after_the_join() {
    let (rows, _) = query("FROM KEY 'orders' AS o JOIN KEY 'customers' AS c ON o.customer = c.cid WHERE c.name = Ada SELECT o.total", &[("orders", orders()), ("customers", customers())]);
    assert_eq!(rows, [serde_json::json!({"o.total": 10}), serde_json::json!({"o.total": 7})]);
}

#[test]
fn the_limit_cuts_the_rows_and_says_more_matched() {
    assert_eq!(query("FROM KEY 'orders' LIMIT 2", &[("orders", orders())]).0.len(), 2);
    assert!(query("FROM KEY 'orders' LIMIT 2", &[("orders", orders())]).1);
    assert!(!query("FROM KEY 'orders' LIMIT 4", &[("orders", orders())]).1, "exactly the limit: nothing left out");
}

#[test]
fn select_star_of_a_join_prefixes_every_column() {
    let (rows, _) = query("FROM KEY 'orders' AS o JOIN KEY 'customers' AS c ON o.customer = c.cid WHERE o.id = 2", &[("orders", orders()), ("customers", customers())]);
    assert_eq!(rows, [serde_json::json!({"o.id": 2, "o.customer": "c2", "o.status": "open", "o.total": 25, "c.cid": "c2", "c.name": "Linus"})]);
}

#[test]
fn a_strings_rows_are_its_object_or_the_objects_of_its_array() {
    assert_eq!(string_records("k", r#"{"a": 1}"#).unwrap().len(), 1);
    assert_eq!(string_records("k", r#"[{"a": 1}, 2, "x", {"b": 2}]"#).unwrap().len(), 2, "what isn't an object is skipped");
    assert!(string_records("k", "  ").unwrap().is_empty());
    let error = string_records("k", "42").unwrap_err();
    assert!(error.to_string().contains("number"), "{error}");
    assert!(string_records("k", "{oops").is_err());
}

#[test]
fn a_hash_field_that_isnt_a_json_object_is_reported_by_name() {
    assert!(hash_record("users", "u1", r#"{"name": "Ada"}"#).is_ok());
    let error = hash_record("users", "u2", "[1]").unwrap_err().to_string();
    assert!(error.contains("users") && error.contains("u2") && error.contains("array"), "{error}");
    let error = hash_record("users", "u3", "not json").unwrap_err().to_string();
    assert!(error.contains("u3"), "{error}");
}
