//! The value-list view: the JSON-path filter bar, and building the rows
//! `AllValuesRow` shows from the fields the search bar's badges leave
//! (`SearchFilter`, in `search_filter.rs`).

use rustc_hash::FxHashSet;
use std::sync::Arc;

use rayon::prelude::*;
use rust_i18n::t;
use serde_json::Value;

use crate::AllValuesRowData;
use crate::backend::redis_value_type::RedisValueType;
use crate::backend::error::AppError;
use crate::json_path::{self, JsonPath};

use super::field::{Field, Fields};
use super::highlight::compact_styled_texts_for;
use super::search_filter::SearchFilter;

// ---------------------------------------------------------------------------
// The filter bar: a path like `records.*.label`, applied to every field's value
// ---------------------------------------------------------------------------

/// The filter bar's text as a path: `None` for an empty box (no filter).
pub(super) fn parse_filter(text: &str) -> Result<Option<JsonPath>, AppError> {
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
        RedisValueType::String if truncated => {
            Some(t!("value cut at %{size}", size = crate::backend::server_info::format_bytes(crate::backend::explorer::MAX_STRING_BYTES as i64)))
        }
        RedisValueType::List | RedisValueType::Set | RedisValueType::Zset => Some(t!("%{count}%{suffix} items", count = count, suffix = suffix)),
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
pub(super) fn build_all_values_table(fields: &[Arc<Field>], filter: &SearchFilter, expanded: &FxHashSet<String>) -> (i32, Vec<AllValuesRowData>) {
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
            AllValuesRowData { field: f.name.as_str().into(), value_compact, value_compact_escaped, initial_expanded: expanded.contains(&f.name) }
        })
        .collect();

    (field_chars, rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::test_support::{field, records_field};
    use serde_json::json;

    #[test]
    fn a_row_starts_open_only_when_its_field_is_in_the_expanded_set() {
        let fields = vec![field("a", None), field("b", None), field("c", None)];
        let expanded = FxHashSet::from_iter(["b".to_string(), "gone".to_string()]);
        let (_, rows) = build_all_values_table(&fields, &SearchFilter::default(), &expanded);
        let opened: Vec<&str> = rows.iter().filter(|r| r.initial_expanded).map(|r| r.field.as_str()).collect();
        assert_eq!(opened, vec!["b"]);
    }

    #[test]
    fn filtering_replaces_each_fields_value_with_what_the_path_matches() {
        let fields: Fields = (0..3).map(records_field).chain([field("text", None), field("nomatch", Some(json!({"x": 1})))]).collect();

        // one match: the field's value becomes it; fields without a match drop out
        let path = parse_filter("records").unwrap();
        let (kept, matches) = filter_fields(&fields, path.as_ref());
        assert_eq!((kept.len(), matches), (3, 3));
        assert!(matches!(&kept[0].json, Some(Value::Array(items)) if items.len() == 2));
        assert_eq!(filter_status(path.as_ref(), matches, kept.len()), "3 matches in 3 fields");

        // several matches: an array of them
        let path = parse_filter("records.*.label").unwrap();
        let (kept, matches) = filter_fields(&fields, path.as_ref());
        assert_eq!((kept.len(), matches), (3, 6));
        assert_eq!(kept[0].json, Some(json!(["a", "b"])));

        // a matched string is plain text, like a non-JSON field
        let path = parse_filter("records[0].label").unwrap();
        let (kept, _) = filter_fields(&fields, path.as_ref());
        assert_eq!((kept[0].raw.as_str(), kept[0].json.is_none()), ("a", true));

        // no path: everything as it was; `$`: the values themselves, text included
        assert_eq!(filter_fields(&fields, None).0.len(), 5);
        assert_eq!(filter_fields(&fields, parse_filter("$").unwrap().as_ref()).0.len(), 5);
        assert_eq!(filter_status(None, 5, 5), "");
        assert_eq!(filter_status(parse_filter("zzz").unwrap().as_ref(), 0, 0), "No match");
        assert_eq!(filter_status(parse_filter("a").unwrap().as_ref(), 1, 1), "1 match in 1 field");
        assert!(parse_filter("  ").unwrap().is_none());
        assert!(parse_filter("a[").is_err());
    }

    #[test]
    fn a_string_key_holding_json_can_be_filtered_like_a_hash_field() {
        use super::super::field::single_value_field;
        let field = single_value_field(Value::String(r#"{"a": [{"b": 1}, {"b": 2}]}"#.to_string()));
        assert!(field.json.is_some());
        assert_eq!(field.name, "(value)");
        let plain = single_value_field(Value::String("just text".to_string()));
        assert!(plain.json.is_none());
        let number_text = single_value_field(Value::String("123".to_string()));
        assert!(number_text.json.is_none(), "a bare number stays text");
        let (kept, matches) = filter_fields(&[Arc::new(field)], parse_filter("a.*.b").unwrap().as_ref());
        assert_eq!((kept[0].json.clone(), matches), (Some(json!([1, 2])), 2));
    }
}
