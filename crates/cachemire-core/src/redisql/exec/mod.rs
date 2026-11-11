//! Running a parsed query: each key's rows are loaded, joined on matching field
//! values, filtered by WHERE and shaped by SELECT.

use rustc_hash::FxHashMap;

use serde_json::{Map, Value};

use super::ast::{Join, Query, Select, WhereCond, WhereOp};
use super::{Located, QueryError};
use crate::error::Error;
use crate::key_name::KeyName;
use crate::redis_value_type::RedisValueType;
use crate::session::Session;
use crate::state::AppState;

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

/// A string key longer than this (in bytes) isn't read: the query fails instead.
const MAX_STRING_BYTES: usize = 16 * 1024 * 1024;

/// `key`'s rows, and whether it held more than [`MAX_SOURCE_FIELDS`] (a join
/// against it may then be missing matches that were never read).
async fn load_records(session: &mut Session, key: &str) -> Result<(Vec<Map<String, Value>>, bool), Error> {
    // a key typed in a query is its text
    let name = KeyName::from(key);
    match session.key_type(&name).await? {
        RedisValueType::Missing => Err(QueryError::KeyNotFound(key.to_string()).into()),
        RedisValueType::Hash => {
            let mut records = Vec::new();
            // a scan may give a field twice
            let mut seen = rustc_hash::FxHashSet::default();
            let mut cursor: u64 = 0;
            loop {
                let (next_cursor, fields) = session.hash_scan(&name, cursor, 500).await?;
                let owner = key.to_string();
                for (field, record) in tokio::task::spawn_blocking(move || hash_records(&owner, fields)).await?? {
                    if seen.insert(field) {
                        records.push(record);
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
            let size = session.string_len(&name).await?;
            if size > MAX_STRING_BYTES {
                return Err(QueryError::StringTooBig { key: key.to_string(), size }.into());
            }
            let raw = session.get(&name).await?.unwrap_or_default();
            let owner = key.to_string();
            Ok((tokio::task::spawn_blocking(move || string_records(&owner, &raw)).await??, false))
        }
        other => Err(QueryError::NotQueryable { key: key.to_string(), kind: other.to_string() }.into()),
    }
}

type Record = Map<String, Value>;

/// The rows of a page of a hash's fields, each with its field's name; the first field
/// that isn't a JSON object is the error.
fn hash_records(key: &str, fields: Vec<(String, String)>) -> Result<Vec<(String, Record)>, Error> {
    fields.into_iter().map(|(field, raw)| hash_record(key, &field, &raw).map(|record| (field, record))).collect()
}

/// The row a hash field's value gives: its JSON, which must be an object.
fn hash_record(key: &str, field: &str, raw: &str) -> Result<Map<String, Value>, Error> {
    match serde_json::from_str::<Value>(raw) {
        Ok(Value::Object(map)) => Ok(map),
        Ok(other) => Err(QueryError::NonObjectField { key: key.to_string(), field: field.to_string(), kind: json_kind(&other) }.into()),
        Err(e) => Err(QueryError::FieldJson { key: key.to_string(), field: field.to_string(), source: e }.into()),
    }
}

/// The rows a string key's value gives: the object it holds, or the objects of its
/// array (anything else in it is skipped); none for a blank value.
fn string_records(key: &str, raw: &str) -> Result<Vec<Map<String, Value>>, Error> {
    if raw.trim().is_empty() {
        return Ok(Vec::new());
    }
    match serde_json::from_str::<Value>(raw) {
        Ok(Value::Object(map)) => Ok(vec![map]),
        Ok(Value::Array(items)) => {
            Ok(items.into_iter().filter_map(|item| if let Value::Object(map) = item { Some(map) } else { None }).collect())
        }
        Ok(other) => Err(QueryError::NonObjectValue { key: key.to_string(), kind: json_kind(&other) }.into()),
        Err(e) => Err(QueryError::ValueJson { key: key.to_string(), source: e }.into()),
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
type Row<'a> = Vec<(&'a str, &'a Map<String, Value>)>;

fn row_get<'a>(row: &Row<'a>, alias: &str) -> Option<&'a Map<String, Value>> {
    row.iter().find(|(a, _)| a.eq_ignore_ascii_case(alias)).map(|(_, r)| *r)
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
    let query = query.clone();
    let (rows, truncated) = tokio::task::spawn_blocking(move || run(&query, &datasets)).await.map_err(Error::from)?;
    Ok(QueryOutput { rows, truncated, sources_truncated })
}

/// Joins, filters and shapes the rows of `datasets` (each key's, by lowercased alias)
/// as `query` says: the rows under its limit, and whether more matched.
fn run(query: &Query, datasets: &FxHashMap<String, Vec<Map<String, Value>>>) -> (Vec<Map<String, Value>>, bool) {
    let aliases: Vec<&str> = std::iter::once(query.base.alias.as_str()).chain(query.joins.iter().map(|j| j.alias.as_str())).collect();
    // each WHERE is checked as soon as the record of its alias joins the row
    let mut checks: Vec<Vec<&WhereCond>> = vec![Vec::new(); aliases.len()];
    for condition in &query.wheres {
        match aliases.iter().position(|alias| alias.eq_ignore_ascii_case(&condition.alias)) {
            Some(stage) => checks[stage].push(condition),
            None => return (Vec::new(), false),
        }
    }
    let indexes = query
        .joins
        .iter()
        .map(|join| {
            let mut index: FxHashMap<String, Vec<&Map<String, Value>>> = FxHashMap::default();
            for record in &datasets[&join.alias.to_lowercase()] {
                if let Some(value) = get_ci(record, &join.join_field).and_then(normalize) {
                    index.entry(value).or_default().push(record);
                }
            }
            index
        })
        .collect::<Vec<_>>();
    let mut joiner = Joiner { query, indexes: &indexes, checks: &checks, rows: Vec::new(), matched: 0 };
    for record in &datasets[&query.base.alias.to_lowercase()] {
        joiner.grow(&mut vec![(query.base.alias.as_str(), record)]);
        if joiner.matched > query.limit {
            break;
        }
    }
    (joiner.rows, joiner.matched > query.limit)
}

/// The join, made depth first so it holds no more than the rows kept, and stops one
/// row past the limit.
struct Joiner<'a> {
    query: &'a Query,
    // each join's records by the value of its join field
    indexes: &'a [FxHashMap<String, Vec<&'a Map<String, Value>>>],
    // the WHEREs on each alias, in join order
    checks: &'a [Vec<&'a WhereCond>],
    rows: Vec<Map<String, Value>>,
    matched: usize,
}

impl<'a> Joiner<'a> {
    /// Takes `row` (its last record just joined) through the joins left, keeping each
    /// complete row that passes every WHERE until one past the limit matched.
    fn grow(&mut self, row: &mut Row<'a>) {
        let stage = row.len() - 1;
        if !self.checks[stage].iter().all(|condition| passes(row, condition)) {
            return;
        }
        let Some(join) = self.query.joins.get(stage) else {
            self.matched += 1;
            if self.rows.len() < self.query.limit {
                self.rows.push(materialize_row(row, &self.query.selects));
            }
            return;
        };
        let Join { anchor_alias, anchor_field, alias, .. } = join;
        let Some(anchor) = row_get(row, anchor_alias) else { return };
        let Some(value) = get_ci(anchor, anchor_field).and_then(normalize) else { return };
        let indexes = self.indexes;
        let Some(matches) = indexes[stage].get(&value) else { return };
        for &record in matches {
            row.push((alias.as_str(), record));
            self.grow(row);
            row.pop();
            if self.matched > self.query.limit {
                return;
            }
        }
    }
}

/// Whether `row`'s record for the condition's alias meets it.
fn passes(row: &Row, condition: &WhereCond) -> bool {
    let Some(record) = row_get(row, &condition.alias) else { return false };
    match get_ci(record, &condition.field).and_then(normalize) {
        Some(value) => match condition.op {
            WhereOp::Eq => value == condition.values[0],
            WhereOp::In => condition.values.contains(&value),
        },
        None => false,
    }
}

/// The most field names `field_names` lists.
const MAX_FIELD_NAMES: usize = 100;

/// The field names the records of `key` have, from its first records only (a hash's first
/// page of fields, or the object or array a string holds), in the order they appear.
pub async fn field_names(state: &AppState, connection_id: Option<&str>, key: &str) -> Result<Vec<String>, Error> {
    let mut session = Session::open(state, connection_id).await?;
    let name = KeyName::from(key);
    let records: Vec<Map<String, Value>> = match session.key_type(&name).await? {
        RedisValueType::Hash => {
            let (_, fields) = session.hash_scan(&name, 0, 100).await?;
            fields
                .iter()
                .filter_map(|(_, raw)| serde_json::from_str::<Value>(raw).ok())
                .filter_map(|value| if let Value::Object(map) = value { Some(map) } else { None })
                .collect()
        }
        RedisValueType::String if session.string_len(&name).await? > MAX_STRING_BYTES => Vec::new(),
        RedisValueType::String => {
            let raw = session.get(&name).await?.unwrap_or_default();
            tokio::task::spawn_blocking(move || match serde_json::from_str::<Value>(&raw) {
                Ok(Value::Object(map)) => vec![map],
                Ok(Value::Array(items)) => {
                    items.into_iter().filter_map(|item| if let Value::Object(map) = item { Some(map) } else { None }).take(100).collect()
                }
                _ => Vec::new(),
            })
            .await?
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
mod tests;
