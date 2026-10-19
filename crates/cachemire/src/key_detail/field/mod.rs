//! The loaded key's fields: `Field`/`Fields` are the parsed model everything
//! else in `key_detail` reads from. The search bar applied to them lives in
//! `search_filter` (`SearchFilter`/`SearchTerm`).

use rustc_hash::FxHashMap;
use std::sync::{Arc, Mutex, OnceLock};

use rayon::prelude::*;
use rust_i18n::t;
use serde::Serialize;
use serde_json::Value;


use super::pretty_text::ColoredParagraph;

/// A field's value as shown and exported: the parsed JSON when its raw text
/// was JSON, otherwise the text itself (displayed as a JSON string). A text
/// field never keeps a second, parsed copy of its string.
#[derive(Clone, Copy)]
pub(super) enum FieldValue<'a> {
    Json(&'a Value),
    Text(&'a str),
}

impl Serialize for FieldValue<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            FieldValue::Json(value) => value.serialize(serializer),
            FieldValue::Text(text) => serializer.serialize_str(text),
        }
    }
}

/// One field of the loaded key: the original raw Redis text, which the value
/// filter matches against, plus its parsed JSON when it is JSON.
pub(super) struct Field {
    pub(super) name: String,
    pub(super) raw: String,
    pub(super) json: Option<Value>,
    // the value's size in bytes when it was too big to be read with the others
    // (`raw` is then empty, until an export reads it)
    pub(super) too_long: Option<usize>,
    // lowercased on first use by a case-insensitive search, then kept for
    // every later keystroke/badge — `raw` especially can be a large JSON
    // blob, not worth re-lowering on every pass over the fields
    lower_name: OnceLock<String>,
    lower_raw: OnceLock<String>,
}

impl Field {
    pub(super) fn new(name: String, raw: String, json: Option<Value>) -> Field {
        Field { name, raw, json, too_long: None, lower_name: OnceLock::new(), lower_raw: OnceLock::new() }
    }

    /// A hash field whose value, `size` bytes long, was left out for its size.
    pub(super) fn too_long(name: String, size: usize) -> Field {
        Field { too_long: Some(size), ..Field::new(name, String::new(), None) }
    }

    /// A field holding `value`: a string stays plain text (shown as a JSON
    /// string), anything else is kept as JSON along with its compact text.
    pub(super) fn from_value(name: String, value: Value) -> Field {
        match value {
            Value::String(raw) => Field::new(name, raw, None),
            other => {
                let raw = serde_json::to_string(&other).unwrap_or_default();
                Field::new(name, raw, Some(other))
            }
        }
    }

    pub(super) fn value(&self) -> FieldValue<'_> {
        match &self.json {
            Some(json) => FieldValue::Json(json),
            None => FieldValue::Text(&self.raw),
        }
    }

    pub(super) fn lower_name(&self) -> &str {
        self.lower_name.get_or_init(|| self.name.to_lowercase())
    }

    pub(super) fn lower_raw(&self) -> &str {
        self.lower_raw.get_or_init(|| self.raw.to_lowercase())
    }
}

/// The loaded key's fields in display order. Each sits behind an `Arc` so a
/// background task can hold one — or a snapshot of the whole list — without
/// copying its data or keeping the lock.
pub(super) type Fields = Vec<Arc<Field>>;

/// A page of hash fields in the order of `names`: each read value JSON-parsed when
/// possible (spread across cores, names/strings moved rather than cloned), each one
/// in `oversized` by its size; a name in neither is gone from the server.
pub(super) fn build_fields(details_value: Value, mut oversized: FxHashMap<String, usize>, names: &[String]) -> Fields {
    let Value::Object(map) = details_value else { return Vec::new() };
    let mut values: FxHashMap<String, Value> = map.into_iter().collect();
    names
        .iter()
        .filter_map(|name| match values.remove_entry(name) {
            Some((name, value)) => Some((name, Ok(value))),
            None => oversized.remove_entry(name).map(|(name, size)| (name, Err(size))),
        })
        .collect::<Vec<_>>()
        .into_par_iter()
        .map(|(name, value)| match value {
            Ok(value) => {
                let raw = match value {
                    Value::String(s) => s,
                    other => other.to_string(),
                };
                let json = serde_json::from_str::<Value>(&raw).ok();
                Arc::new(Field::new(name, raw, json))
            }
            Err(size) => Arc::new(Field::too_long(name, size)),
        })
        .collect()
}

/// The single "(value)" field standing for a non-hash key's whole value.
pub(super) fn single_value_field(value: Value) -> Field {
    let name = t!("(value)").into_owned();
    match value {
        // a string key holding a JSON object or array is shown, and can be
        // filtered, as that JSON — like a hash field's value is
        Value::String(raw) => match serde_json::from_str::<Value>(&raw) {
            Ok(json @ (Value::Object(_) | Value::Array(_))) => Field::new(name, raw, Some(json)),
            _ => Field::new(name, raw, None),
        },
        other => Field::from_value(name, other),
    }
}

/// Open-row renderings by field name, plain and escaped apart, so a lookup by `&str`
/// allocates nothing. `Arc` per entry so serving one is a cheap clone.
#[derive(Default)]
pub(super) struct PrettyRenderings([FxHashMap<String, Arc<Vec<ColoredParagraph>>>; 2]);

impl PrettyRenderings {
    pub(super) fn get(&self, name: &str, escaped: bool) -> Option<&Arc<Vec<ColoredParagraph>>> {
        self.0[usize::from(escaped)].get(name)
    }

    pub(super) fn contains(&self, name: &str, escaped: bool) -> bool {
        self.0[usize::from(escaped)].contains_key(name)
    }

    pub(super) fn insert(&mut self, name: String, escaped: bool, paragraphs: Arc<Vec<ColoredParagraph>>) {
        self.0[usize::from(escaped)].insert(name, paragraphs);
    }

    /// Makes the plain rendering of `name` with `render`, unless there is one already.
    pub(super) fn plain_or_insert_with(&mut self, name: &str, render: impl FnOnce() -> Vec<ColoredParagraph>) {
        if !self.contains(name, false) {
            self.insert(name.to_string(), false, Arc::new(render()));
        }
    }

    pub(super) fn clear(&mut self) {
        self.0.iter_mut().for_each(FxHashMap::clear);
    }
}

/// The renderings, shared with the background tasks that fill them.
pub(super) type PrettyCache = Arc<Mutex<PrettyRenderings>>;

#[cfg(test)]
mod tests;
