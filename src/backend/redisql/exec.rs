//! Running a parsed query: each key's rows are loaded, joined on matching field
//! values, filtered by WHERE and shaped by SELECT.

use rustc_hash::FxHashMap;

use rust_i18n::t;
use serde_json::{Map, Value};

use super::Located;
use super::ast::{Query, Select, WhereOp};
use crate::backend::error::AppError;
use crate::backend::key_name::KeyName;
use crate::backend::redis_value_type::RedisValueType;
use crate::backend::session::Session;
use crate::backend::state::AppState;

// ---------------------------------------------------------------------------
// Value helpers
// ---------------------------------------------------------------------------

/// A field, found whatever its case (Redis field/JSON key names alike): what
/// matters is the value, not the exact spelling someone typed.
fn get_ci<'a>(record: &'a Map<String, Value>, field: &str) -> Option<&'a Value> {
    record.get(field).or_else(|| record.iter().find(|(k, _)| k.eq_ignore_ascii_case(field)).map(|(_, v)| v))
}

/// A value as the plain string a join or a WHERE compares — `null`/absent
/// never matches anything, so callers see it as `None`.
fn normalize(value: &Value) -> Option<String> {
    match value {
        Value::Null => None,
        Value::String(s) => Some(s.clone()),
        Value::Bool(b) => Some(b.to_string()),
        Value::Number(n) => Some(n.to_string()),
        other => Some(other.to_string()),
    }
}

// ---------------------------------------------------------------------------
// Loading a key's rows
// ---------------------------------------------------------------------------

/// A hash referenced by FROM/JOIN is read at most this many fields, bounding time
/// and memory; the query runs on what was read and `load_records` says so.
const MAX_SOURCE_FIELDS: usize = 20_000;

/// `key`'s rows, and whether it held more than [`MAX_SOURCE_FIELDS`] (a join
/// against it may then be missing matches that were never read).
async fn load_records(session: &mut Session, key: &str) -> Result<(Vec<Map<String, Value>>, bool), AppError> {
    // a key typed in a query is its text
    let name = KeyName::from(key);
    match session.key_type(&name).await? {
        RedisValueType::Missing => Err(AppError::not_found(t!("Key not found: %{key}", key = key))),
        RedisValueType::Hash => {
            let mut records = Vec::new();
            let mut cursor: u64 = 0;
            loop {
                let (next_cursor, fields) = session.hash_scan(&name, cursor, 500).await?;
                for (field, raw) in &fields {
                    match serde_json::from_str::<Value>(raw) {
                        Ok(Value::Object(map)) => records.push(map),
                        Ok(other) => return Err(AppError::invalid(t!("A non-object value was found in %{key} (field %{field}): %{kind}", key = key, field = field.as_str(), kind = json_kind(&other)))),
                        Err(e) => return Err(AppError::invalid(t!("Invalid JSON in %{key} (field %{field}): %{error}", key = key, field = field.as_str(), error = e.to_string()))),
                    }
                }
                cursor = next_cursor;
                if cursor == 0 || records.len() >= MAX_SOURCE_FIELDS {
                    break;
                }
            }
            // the cursor not having come back to 0 is what "more was left" means
            Ok((records, cursor != 0))
        }
        RedisValueType::String => {
            let raw = session.get(&name).await?.unwrap_or_default();
            if raw.trim().is_empty() {
                return Ok((Vec::new(), false));
            }
            match serde_json::from_str::<Value>(&raw) {
                Ok(Value::Object(map)) => Ok((vec![map], false)),
                Ok(Value::Array(items)) => Ok((items.into_iter().filter_map(|item| if let Value::Object(map) = item { Some(map) } else { None }).collect(), false)),
                Ok(other) => Err(AppError::invalid(t!("%{key} holds JSON, but not an object or an array of objects: %{kind}", key = key, kind = json_kind(&other)))),
                Err(e) => Err(AppError::invalid(t!("Invalid JSON in %{key}: %{error}", key = key, error = e.to_string()))),
            }
        }
        other => Err(AppError::invalid(t!("%{key} is a %{kind}: only a hash of JSON objects, or a string holding one, can be queried.", key = key, kind = other.to_string()))),
    }
}

fn json_kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

// ---------------------------------------------------------------------------
// Execution
// ---------------------------------------------------------------------------

/// One matched row while joining: the record found so far for each alias
/// mentioned, in the order they were joined in. A handful of aliases at
/// most, so a linear scan by name is simpler (and just as fast) as a map.
type Row = Vec<(String, Map<String, Value>)>;

fn row_get<'a>(row: &'a Row, alias: &str) -> Option<&'a Map<String, Value>> {
    row.iter().find(|(a, _)| a.eq_ignore_ascii_case(alias)).map(|(_, r)| r)
}

/// `record`'s fields, each keyed as `alias.field` when `prefix` (several
/// aliases are in play) or plainly otherwise.
fn flatten(alias: &str, record: &Map<String, Value>, prefix: bool, out: &mut Map<String, Value>) {
    for (field, value) in record {
        let key = if prefix { format!("{alias}.{field}") } else { field.clone() };
        out.insert(key, value.clone());
    }
}

fn materialize_row(row: &Row, selects: &[Select]) -> Map<String, Value> {
    let mut out = Map::new();
    let multi_source = row.len() > 1;
    if selects.is_empty() || (selects.len() == 1 && selects[0].alias == "*") {
        for (alias, record) in row {
            flatten(alias, record, multi_source, &mut out);
        }
        return out;
    }
    for select in selects {
        let prefixed = select.explicit_alias || multi_source;
        let Some(record) = row_get(row, &select.alias) else {
            out.insert(if prefixed { format!("{}.{}", select.alias, select.field) } else { select.field.clone() }, Value::Null);
            continue;
        };
        if select.field == "*" {
            flatten(&select.alias, record, prefixed, &mut out);
            continue;
        }
        let column = if prefixed { format!("{}.{}", select.alias, select.field) } else { select.field.clone() };
        out.insert(column, get_ci(record, &select.field).cloned().unwrap_or(Value::Null));
    }
    out
}

/// What a query produced: its rows (already limited), and whether more
/// matched than fit under the limit.
pub struct QueryOutput {
    pub rows: Vec<Map<String, Value>>,
    pub truncated: bool,
    /// a FROM/JOIN key held more than [`MAX_SOURCE_FIELDS`]: a join against it
    /// may be missing matches that were past the cutoff and never read.
    pub sources_truncated: bool,
}

/// Runs `query`; an error about one of its keys points at where that key is written.
pub async fn execute(session: &mut Session, query: &Query) -> Result<QueryOutput, Located> {
    let mut datasets: FxHashMap<String, Vec<Map<String, Value>>> = FxHashMap::default();
    let mut sources_truncated = false;
    let (base_records, base_cut) = load_records(session, &query.base.key).await.map_err(Located::at(&query.base.key_span))?;
    sources_truncated |= base_cut;
    datasets.insert(query.base.alias.to_lowercase(), base_records);
    for join in &query.joins {
        // `parse_query` already rejected a duplicate alias, so each join's is new
        let (records, cut) = load_records(session, &join.key).await.map_err(Located::at(&join.key_span))?;
        sources_truncated |= cut;
        datasets.insert(join.alias.to_lowercase(), records);
    }

    let mut rows: Vec<Row> = datasets[&query.base.alias.to_lowercase()].iter().map(|record| vec![(query.base.alias.clone(), record.clone())]).collect();

    for join in &query.joins {
        let right = &datasets[&join.alias.to_lowercase()];
        let mut index: FxHashMap<String, Vec<&Map<String, Value>>> = FxHashMap::default();
        for record in right {
            if let Some(value) = get_ci(record, &join.join_field).and_then(normalize) {
                index.entry(value).or_default().push(record);
            }
        }
        let mut joined = Vec::new();
        for row in &rows {
            let Some(anchor) = row_get(row, &join.anchor_alias) else { continue };
            let Some(anchor_value) = get_ci(anchor, &join.anchor_field).and_then(normalize) else { continue };
            if let Some(matches) = index.get(&anchor_value) {
                for m in matches {
                    let mut next = row.clone();
                    next.push((join.alias.clone(), (*m).clone()));
                    joined.push(next);
                }
            }
        }
        rows = joined;
    }

    if !query.wheres.is_empty() {
        rows.retain(|row| {
            query.wheres.iter().all(|w| {
                let Some(record) = row_get(row, &w.alias) else { return false };
                match get_ci(record, &w.field).and_then(normalize) {
                    Some(value) => match w.op {
                        WhereOp::Eq => value == w.values[0],
                        WhereOp::In => w.values.contains(&value),
                    },
                    None => false,
                }
            })
        });
    }

    let truncated = (rows.len() as i64) > query.limit;
    let rows = rows.into_iter().take(query.limit as usize).map(|row| materialize_row(&row, &query.selects)).collect();
    Ok(QueryOutput { rows, truncated, sources_truncated })
}

/// The most field names `field_names` lists.
const MAX_FIELD_NAMES: usize = 100;

/// The field names the records of `key` have, from its first records only (a hash's first
/// page of fields, or the object or array a string holds), in the order they appear.
pub async fn field_names(state: &AppState, connection_id: Option<&str>, key: &str) -> Result<Vec<String>, AppError> {
    let mut session = Session::open(state, connection_id).await?;
    let name = KeyName::from(key);
    let records: Vec<Map<String, Value>> = match session.key_type(&name).await? {
        RedisValueType::Hash => {
            let (_, fields) = session.hash_scan(&name, 0, 100).await?;
            fields.iter().filter_map(|(_, raw)| serde_json::from_str::<Value>(raw).ok()).filter_map(|value| if let Value::Object(map) = value { Some(map) } else { None }).collect()
        }
        RedisValueType::String => {
            let raw = session.get(&name).await?.unwrap_or_default();
            match serde_json::from_str::<Value>(&raw) {
                Ok(Value::Object(map)) => vec![map],
                Ok(Value::Array(items)) => items.into_iter().filter_map(|item| if let Value::Object(map) = item { Some(map) } else { None }).take(100).collect(),
                _ => Vec::new(),
            }
        }
        _ => Vec::new(),
    };
    let mut names: Vec<String> = Vec::new();
    for record in &records {
        for name in record.keys() {
            if !names.contains(name) && names.len() < MAX_FIELD_NAMES {
                names.push(name.clone());
            }
        }
    }
    Ok(names)
}

#[cfg(test)]
mod tests {
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
}
