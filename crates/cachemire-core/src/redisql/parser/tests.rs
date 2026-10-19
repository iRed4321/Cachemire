use super::super::ast::{DEFAULT_LIMIT, Query, WhereOp};
use super::super::parse_query;

#[test]
fn parses_a_join_query_whatever_order_the_clauses_come_in() {
    let query = "FROM KEY 'orders:items' AS i\nJOIN KEY 'products' AS p ON i.productId = p.id\nWHERE i.qty IN [1,2,3]\nSELECT i.productId, p.label\nLIMIT 50";
    let parsed = parse_query(query).unwrap();
    assert_eq!(parsed.base.alias, "i");
    assert_eq!(parsed.joins.len(), 1);
    assert_eq!(parsed.joins[0].anchor_alias, "i");
    assert_eq!(parsed.joins[0].anchor_field, "productId");
    assert_eq!(parsed.joins[0].join_field, "id");
    assert_eq!(parsed.wheres[0].op, WhereOp::In);
    assert_eq!(parsed.wheres[0].values, vec!["1", "2", "3"]);
    assert_eq!(parsed.selects.len(), 2);
    assert_eq!(parsed.limit, 50);

    // the same query, clauses in a different order, parses the same way (spans aside)
    let reordered = "SELECT i.productId, p.label\nLIMIT 50\nFROM KEY 'orders:items' AS i\nWHERE i.qty IN [1,2,3]\nJOIN KEY 'products' AS p ON i.productId = p.id";
    let shape = |q: &Query| format!("{} {} {:?} {:?} {:?} {}", q.base.key, q.base.alias, q.joins.iter().map(|j| (&j.key, &j.alias, &j.anchor_alias, &j.anchor_field, &j.join_field)).collect::<Vec<_>>(), q.wheres.iter().map(|w| (&w.alias, &w.field, w.op, &w.values)).collect::<Vec<_>>(), q.selects.iter().map(|s| (&s.alias, &s.field, s.explicit_alias)).collect::<Vec<_>>(), q.limit);
    assert_eq!(shape(&parse_query(reordered).unwrap()), shape(&parsed));
}

#[test]
fn the_compact_one_liner_needs_no_separate_path() {
    // "AS alias" is optional for a single-key query: it defaults to "src"
    let parsed = parse_query("SELECT * FROM KEY 'orders:42'").unwrap();
    assert_eq!(parsed.base.alias, "src");
    assert_eq!(parsed.selects.iter().map(|s| (s.alias.as_str(), s.field.as_str(), s.explicit_alias)).collect::<Vec<_>>(), vec![("*", "*", true)]);
    assert_eq!(parsed.limit, DEFAULT_LIMIT);

    let parsed = parse_query("SELECT status, total FROM KEY 'orders:42' AS o WHERE status = 'paid' LIMIT 10").unwrap();
    assert_eq!(parsed.selects[0].alias, "o");
    let w = &parsed.wheres[0];
    assert_eq!((w.alias.as_str(), w.field.as_str(), w.op, w.values.clone()), ("o", "status", WhereOp::Eq, vec!["paid".to_string()]));
    assert_eq!(parsed.limit, 10);
}

#[test]
fn an_unqualified_field_means_the_from_keys_alias() {
    let parsed = parse_query("FROM KEY 'a' AS x\nWHERE active = true\nSELECT name").unwrap();
    assert_eq!(parsed.wheres[0].alias, "x");
    assert_eq!(parsed.selects[0].alias, "x");
}

#[test]
fn rejects_a_join_to_an_unknown_alias() {
    let err = parse_query("FROM KEY 'a' AS x\nJOIN KEY 'b' AS y ON z.id = y.refId\nSELECT *").unwrap_err();
    assert!(err.message().contains("unknown alias"), "{err}");
}

#[test]
fn rejects_a_duplicate_alias() {
    let err = parse_query("FROM KEY 'a' AS x\nJOIN KEY 'b' AS x ON q.id = x.refId\nSELECT *").unwrap_err();
    assert!(err.message().contains("Duplicate alias"), "{err}");
}

#[test]
fn alias_matching_is_case_insensitive() {
    let parsed = parse_query("FROM KEY 'a' AS Src\nWHERE SRC.id = 1\nSELECT src.id").unwrap();
    assert_eq!(parsed.wheres[0].alias, "SRC");
    assert_eq!(parsed.selects[0].alias, "src");
}

#[test]
fn parse_list_handles_quoted_and_numeric_values() {
    assert_eq!(parse_query("FROM KEY 'k' WHERE x IN ['a', 'b', 42]").unwrap().wheres[0].values, vec!["a", "b", "42"]);
    assert!(parse_query("FROM KEY 'k' WHERE x IN 1,2,3").is_err(), "needs [] or ()");
}

/// The text an error of `query` points at.
fn pointed_at(query: &str) -> &str {
    let error = parse_query(query).unwrap_err();
    &query[error.span.expect("the error says where it is")]
}

#[test]
fn the_limit_is_kept_between_1_and_the_maximum() {
    use super::super::ast::MAX_LIMIT;
    let limit = |text: &str| parse_query(&format!("FROM KEY 'k' LIMIT {text}")).map(|q| q.limit);
    assert_eq!(limit("0").unwrap(), 1);
    assert_eq!(limit("-5").unwrap(), 1);
    assert_eq!(limit("99999999").unwrap(), MAX_LIMIT);
    assert!(limit("1.5").is_err());
    assert!(limit("ten").is_err());
    assert!(limit("").is_err());
    assert_eq!(parse_query("FROM KEY 'k'").unwrap().limit, DEFAULT_LIMIT);
}

#[test]
fn where_conditions_chain_with_and_and_keep_their_operator() {
    let query = parse_query("FROM KEY 'k' AS k WHERE k.status = 'paid' AND region IN (eu, 'us') AND k.n = -3").unwrap();
    let conditions: Vec<(&str, &str, WhereOp, Vec<String>)> =
        query.wheres.iter().map(|w| (w.alias.as_str(), w.field.as_str(), w.op, w.values.clone())).collect();
    assert_eq!(
        conditions,
        [
            ("k", "status", WhereOp::Eq, vec!["paid".to_string()]),
            ("k", "region", WhereOp::In, vec!["eu".to_string(), "us".to_string()]),
            ("k", "n", WhereOp::Eq, vec!["-3".to_string()]),
        ]
    );
}

#[test]
fn keys_with_colons_dashes_and_dates_are_one_value() {
    let query = parse_query("FROM KEY 'k' WHERE day = 2024-01-31 AND ref = user:42").unwrap();
    assert_eq!(query.wheres[0].values, ["2024-01-31"]);
    assert_eq!(query.wheres[1].values, ["user:42"]);
}

#[test]
fn select_takes_stars_per_alias_and_plain_fields() {
    let query = parse_query("FROM KEY 'a' AS a JOIN KEY 'b' AS b ON a.id = b.aid SELECT a.*, b.name, total").unwrap();
    let selects: Vec<(&str, &str, bool)> = query.selects.iter().map(|s| (s.alias.as_str(), s.field.as_str(), s.explicit_alias)).collect();
    assert_eq!(selects, [("a", "*", true), ("b", "name", true), ("a", "total", false)]);
}

#[test]
fn a_join_may_name_its_own_alias_on_either_side() {
    let left = parse_query("FROM KEY 'a' AS a JOIN KEY 'b' AS b ON b.aid = a.id").unwrap();
    let right = parse_query("FROM KEY 'a' AS a JOIN KEY 'b' AS b ON a.id = b.aid").unwrap();
    for query in [left, right] {
        let join = &query.joins[0];
        assert_eq!((join.anchor_alias.as_str(), join.anchor_field.as_str(), join.join_field.as_str()), ("a", "id", "aid"));
    }
    let unqualified = parse_query("FROM KEY 'a' AS a JOIN KEY 'b' AS b ON a.id = aid").unwrap();
    assert_eq!(unqualified.joins[0].join_field, "aid", "a side with no alias is the joined key's");
}

#[test]
fn errors_point_at_what_is_wrong() {
    assert_eq!(pointed_at("FROM KEY 'a' AS a JOIN KEY 'b' AS b ON a.id = a.ref"), "a.id = a.ref");
    assert_eq!(pointed_at("FROM KEY 'a' AS a JOIN KEY 'b' AS b ON a.id = c.ref"), "a.id = c.ref");
    assert_eq!(pointed_at("FROM KEY 'a' AS a WHERE z.id = 1"), "z");
    assert_eq!(pointed_at("FROM KEY 'a' AS a SELECT z.name"), "z");
    assert_eq!(pointed_at("FROM KEY 'a' FROM KEY 'b'"), "FROM");
    assert_eq!(pointed_at("FROM KEY 'a' WHERE x = 1 SELECT x WHERE y = 2"), "WHERE");
    assert_eq!(pointed_at("FROM KEY 'a' SELECT x select y"), "select");
    assert_eq!(pointed_at("FROM KEY 'a' LIMIT 5 LIMIT 10"), "LIMIT");
    assert_eq!(pointed_at("FROM KEY 'a' ORDER BY x"), "ORDER");
    assert_eq!(pointed_at("FROM KEY 'a' WHERE x IN []"), "[");
    assert_eq!(pointed_at("FROM KEY 'a' AS a extra"), "extra");
}

#[test]
fn incomplete_queries_are_refused() {
    for query in [
        "",
        "SELECT *",
        "FROM 'k'",
        "FROM KEY k",
        "FROM KEY 'k' AS",
        "FROM KEY 'a' JOIN KEY 'b' ON a.id = b.id",
        "FROM KEY 'a' AS a JOIN KEY 'b' AS b a.id = b.id",
        "FROM KEY 'a' WHERE x",
        "FROM KEY 'a' WHERE x = ''",
        "FROM KEY 'a' WHERE x IN [1, 2",
        "FROM KEY 'a' SELECT a.",
    ] {
        assert!(parse_query(query).is_err(), "{query:?} was accepted");
    }
}
