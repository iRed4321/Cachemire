//! The value-list view: the JSON-path filter bar, and building the rows
//! `AllValuesRow` shows from the fields the search bar's badges leave
//! (`SearchFilter`, in `search_filter/mod.rs`).

use rustc_hash::FxHashSet;
use std::sync::Arc;

use rayon::prelude::*;
use rust_i18n::t;
use serde_json::Value;

use crate::AllValuesRowData;
use cachemire_core::json_path::{self, JsonPath};
use cachemire_core::redis_value_type::RedisValueType;

use super::field::{Field, Fields};
use super::highlight::compact_styled_texts_for;
use super::search_filter::SearchFilter;

// ---------------------------------------------------------------------------
// The filter bar: a path like `records.*.label`, applied to every field's value
// ---------------------------------------------------------------------------

/// The filter bar's text as a path: `None` for an empty box (no filter).
pub(super) fn parse_filter(text: &str) -> Result<Option<JsonPath>, json_path::ParseError> {
    if text.trim().is_empty() { Ok(None) } else { json_path::parse(text).map(Some) }
}

/// What `path` matches in `json`; a path starting with `value` (the name the
/// field/value layout gives the value) that finds nothing is tried again
/// without it.
fn select_matches<'a>(path: &JsonPath, json: &'a Value) -> Vec<&'a Value> {
    let found = path.select(json);
    match path.without_value_alias() {
        Some(alias) if found.is_empty() => alias.select(json),
        _ => found,
    }
}

/// One field's value after `path`, with how many matched: `None` if nothing
/// does or the field isn't JSON. One match becomes the value; several become
/// an array — laid out as a table (see [`super::table::build_table`]) per object/array.
pub(super) fn filter_field(field: &Arc<Field>, path: &JsonPath) -> Option<(Arc<Field>, usize)> {
    let has_value_key = field.json.as_ref().is_some_and(|json| json.get("value").is_some());
    if path.is_identity() || (path.is_value_alias_only() && !has_value_key) {
        return Some((field.clone(), 1));
    }
    let json = field.json.as_ref()?;
    let matches: Vec<&Value> = match path.without_trailing_wildcard() {
        Some(parent) => {
            let parents = if parent.is_identity() { vec![json] } else { select_matches(&parent, json) };
            parents
                .into_iter()
                .flat_map(|parent| match parent {
                    Value::Array(items) => items.iter().collect(),
                    Value::Object(_) => vec![parent],
                    _ => Vec::new(),
                })
                .collect()
        }
        None => select_matches(path, json),
    };
    let count = matches.len();
    let value = match matches.as_slice() {
        [] => return None,
        [single] => (*single).clone(),
        several => Value::Array(several.iter().map(|v| (*v).clone()).collect()),
    };
    Some((Arc::new(Field::from_value(field.name.clone(), value)), count))
}

/// The fields as the filter leaves them (those with no match drop out), and
/// the total number of matches. Copying the matched values is spread across
/// cores.
pub(super) fn filter_fields(fields: &[Arc<Field>], path: Option<&JsonPath>) -> (Fields, usize) {
    let Some(path) = path else { return (fields.to_vec(), fields.len()) };
    let kept: Vec<(Arc<Field>, usize)> = fields.par_iter().filter_map(|field| filter_field(field, path)).collect();
    let matches = kept.iter().map(|(_, n)| n).sum();
    (kept.into_iter().map(|(field, _)| field).collect(), matches)
}

/// What the bar says beside the filter: how much it matched ("" with no filter).
pub(super) fn filter_status(path: Option<&JsonPath>, matches: usize, fields: usize) -> String {
    match (path, fields) {
        (None, _) => String::new(),
        (Some(_), 0) => t!("No match").into_owned(),
        (Some(_), _) => match (matches == 1, fields == 1) {
            (true, true) => t!("1 match in 1 field"),
            (true, false) => t!("1 match in %{fields} fields", fields = fields),
            (false, true) => t!("%{matches} matches in 1 field", matches = matches),
            (false, false) => t!("%{matches} matches in %{fields} fields", matches = matches, fields = fields),
        }
        .into_owned(),
    }
}

/// The key's size and TTL, as the panel's header shows them. `loaded` is how many of
/// a hash's field names are read when not all of them are (they're then sorted only
/// among themselves); a cut string says where it was cut.
pub(super) fn meta_text(key_type: &RedisValueType, ttl: i64, count: usize, truncated: bool, loaded: Option<usize>) -> String {
    let ttl_part = if ttl < 0 { t!("no ttl").into_owned() } else { t!("ttl %{ttl}s", ttl = ttl).into_owned() };
    // a "+" says the list was cut short
    let suffix = if truncated { "+" } else { "" };
    let size = match key_type {
        RedisValueType::Hash => Some(match loaded {
            Some(loaded) => t!("%{count} fields · %{loaded} loaded, sorted among themselves", count = count, loaded = loaded),
            None => t!("%{count}%{suffix} fields", count = count, suffix = suffix),
        }),
        RedisValueType::String if truncated => Some(t!(
            "value cut at %{size}",
            size = cachemire_core::server_info::format_bytes(cachemire_core::explorer::MAX_STRING_BYTES as i64)
        )),
        RedisValueType::List | RedisValueType::Set | RedisValueType::Zset => {
            Some(t!("%{count}%{suffix} items", count = count, suffix = suffix))
        }
        RedisValueType::Stream => Some(t!("%{count}%{suffix} entries", count = count, suffix = suffix)),
        _ => None,
    };
    match size {
        Some(size) => format!("{size} · {ttl_part}"),
        None => ttl_part,
    }
}

// field column width in chars: widest field name, clamped so a narrow column
// stays legible and one huge name can't blow out the table
const MIN_COL_CHARS: i32 = 4;
const MAX_COL_CHARS: i32 = 200;

/// One row per field/value pair. Only the collapsed preview is built here —
/// the pretty rendering is built on demand via `request-pretty-value`, since
/// most rows are never expanded. `expanded` names which should open right away.
pub(super) fn build_all_values_table(
    fields: &[Arc<Field>],
    filter: &SearchFilter,
    expanded: &FxHashSet<String>,
) -> (i32, Vec<AllValuesRowData>) {
    // spread across cores too: a big hash means a big scan, and each field's
    // `filter.matches` is now independent (memoized lowercasing lives on the
    // `Field`/`SearchFilter` instances, not in any shared mutable state)
    let matching: Vec<&Field> = fields.par_iter().map(|f| &**f).filter(|f| filter.matches(f)).collect();

    let field_chars = matching
        .iter()
        .map(|f| f.name.chars().count() as i32)
        .max()
        .unwrap_or(0)
        .max("Field".len() as i32)
        .clamp(MIN_COL_CHARS, MAX_COL_CHARS);

    // the badges whose match highlights inside the value text, computed once
    // rather than per field
    let needles = filter.highlight_needles();

    // coloring each preview is the expensive part here and is independent per
    // field, so spread it across cores; rayon keeps `matching`'s sequence
    let rows = matching
        .into_par_iter()
        .map(|f| {
            let (value_compact, value_compact_escaped) = compact_styled_texts_for(f.value(), &needles, filter.case_sensitive);
            let too_long = f.too_long.map(|size| cachemire_core::server_info::format_bytes(size as i64)).unwrap_or_default();
            // a value left out for its size has nothing to open: its row never starts open
            let initial_expanded = f.too_long.is_none() && expanded.contains(&f.name);
            AllValuesRowData {
                field: f.name.as_str().into(),
                value_compact,
                value_compact_escaped,
                too_long: too_long.into(),
                initial_expanded,
            }
        })
        .collect();

    (field_chars, rows)
}

#[cfg(test)]
mod tests;
