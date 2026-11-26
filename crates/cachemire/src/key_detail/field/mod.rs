//! The loaded key's fields: `Field`/`Fields` are the parsed model everything
//! else in `key_detail` reads from. The search bar applied to them lives in
//! `search_filter` (`SearchFilter`/`SearchTerm`).

use parking_lot::Mutex;
use rustc_hash::{FxHashMap, FxHashSet};
use std::borrow::Cow;
use std::sync::{Arc, OnceLock};

use crate::t;
use rayon::prelude::*;
use serde::Serialize;
use serde_json::Value;

use super::fold::{fold, unescaped_json};
use super::formats::{ShowFormat, structured_value};
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
    // (`raw` is then empty, until it's read)
    pub(super) too_long: Option<usize>,
    // the value isn't UTF-8 text: `raw` writes its other bytes as `\xNN`, and it is never edited
    pub(super) binary: bool,
    // `name` and `raw` as the search reads them (`fold`), case kept then not, made on first
    // use and kept for every later badge (`None`: the text itself); `raw` can be a large blob
    folded: [[OnceLock<Option<String>>; 2]; 2],
    // its JSON's `\uXXXX` escapes written out, then folded, case kept then not (`None`: no escape)
    unescaped: [OnceLock<Option<String>>; 2],
    // the object or array a YAML or XML value stands for, read on first use by the table
    structured: OnceLock<Option<Value>>,
}

impl Field {
    pub(super) fn new(name: String, raw: String, json: Option<Value>) -> Field {
        Field {
            name,
            raw,
            json,
            too_long: None,
            binary: false,
            folded: Default::default(),
            unescaped: Default::default(),
            structured: OnceLock::new(),
        }
    }

    /// A hash field whose value, `size` bytes long, was left out for its size.
    pub(super) fn too_long(name: String, size: usize) -> Field {
        Field { too_long: Some(size), ..Field::new(name, String::new(), None) }
    }

    /// A hash field read as `raw`, parsed when it is JSON.
    pub(super) fn hash_value(name: String, raw: String, binary: bool) -> Field {
        let json = serde_json::from_str::<Value>(&raw).ok();
        Field { binary, ..Field::new(name, raw, json) }
    }

    /// Whether the field can't be written: its value or its name isn't UTF-8 text.
    pub(super) fn is_binary(&self) -> bool {
        self.binary || !cachemire_core::key_name::is_text_field(&self.name)
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

    /// What the table reads the field's columns from: its JSON, else the object or array
    /// its YAML or XML text stands for.
    pub(super) fn table_value(&self) -> Option<&Value> {
        self.json.as_ref().or_else(|| self.structured.get_or_init(|| structured_value(&self.raw)).as_ref())
    }

    /// The field's name as a search matching case or not reads it.
    pub(super) fn searched_name(&self, case_sensitive: bool) -> &str {
        folded_once(&self.folded[0][usize::from(case_sensitive)], &self.name, case_sensitive)
    }

    /// The field's raw value as a search matching case or not reads it.
    pub(super) fn searched_raw(&self, case_sensitive: bool) -> &str {
        folded_once(&self.folded[1][usize::from(case_sensitive)], &self.raw, case_sensitive)
    }

    /// The field's JSON value with its `\uXXXX` escapes written out, as a search reads it:
    /// `None` when its raw text holds none.
    pub(super) fn searched_unescaped(&self, case_sensitive: bool) -> Option<&str> {
        self.unescaped[usize::from(case_sensitive)]
            .get_or_init(|| {
                let text = unescaped_json(&self.raw, self.json.as_ref()?)?;
                Some(fold(&text, case_sensitive).into_owned())
            })
            .as_deref()
    }
}

/// `text` folded, kept in `cell` the first time when that differs from `text`.
fn folded_once<'a>(cell: &'a OnceLock<Option<String>>, text: &'a str, case_sensitive: bool) -> &'a str {
    cell.get_or_init(|| match fold(text, case_sensitive) {
        Cow::Borrowed(_) => None,
        Cow::Owned(folded) => Some(folded),
    })
    .as_deref()
    .unwrap_or(text)
}

/// The loaded key's fields in display order. Each sits behind an `Arc` so a
/// background task can hold one — or a snapshot of the whole list — without
/// copying its data or keeping the lock.
pub(super) type Fields = Vec<Arc<Field>>;

/// A page of hash fields in the order of `names`: each read value JSON-parsed when
/// possible (spread across cores, names/strings moved rather than cloned), each one
/// in `oversized` by its size; a name in neither is gone from the server.
pub(super) fn build_fields(
    details_value: Value,
    mut oversized: FxHashMap<String, usize>,
    binary: &FxHashSet<String>,
    names: &[String],
) -> Fields {
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
                let binary = binary.contains(&name);
                Arc::new(Field::hash_value(name, raw, binary))
            }
            Err(size) => Arc::new(Field::too_long(name, size)),
        })
        .collect()
}

/// The single "(value)" field standing for a non-hash key's whole value.
pub(super) fn single_value_field(value: Value, binary: bool) -> Field {
    let name = t!("(value)").into_owned();
    Field { binary, ..single_field(name, value) }
}

fn single_field(name: String, value: Value) -> Field {
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

/// Open-row renderings by field name, each format's apart, so a lookup by `&str`
/// allocates nothing. `Arc` per entry so serving one is a cheap clone.
#[derive(Default)]
pub(super) struct PrettyRenderings([FxHashMap<String, Arc<Vec<ColoredParagraph>>>; ShowFormat::COUNT]);

impl PrettyRenderings {
    pub(super) fn get(&self, name: &str, format: ShowFormat) -> Option<&Arc<Vec<ColoredParagraph>>> {
        self.0[format.index()].get(name)
    }

    pub(super) fn contains(&self, name: &str, format: ShowFormat) -> bool {
        self.0[format.index()].contains_key(name)
    }

    pub(super) fn insert(&mut self, name: String, format: ShowFormat, paragraphs: Arc<Vec<ColoredParagraph>>) {
        self.0[format.index()].insert(name, paragraphs);
    }

    /// Makes the rendering of `name` in `format` with `render`, unless there is one already.
    pub(super) fn get_or_insert_with(&mut self, name: &str, format: ShowFormat, render: impl FnOnce() -> Vec<ColoredParagraph>) {
        if !self.contains(name, format) {
            self.insert(name.to_string(), format, Arc::new(render()));
        }
    }

    pub(super) fn clear(&mut self) {
        self.0.iter_mut().for_each(FxHashMap::clear);
    }

    /// Drops the renderings of the fields named in `gone`.
    pub(super) fn forget(&mut self, gone: &rustc_hash::FxHashSet<String>) {
        self.0.iter_mut().for_each(|renderings| renderings.retain(|name, _| !gone.contains(name)));
    }
}

/// The renderings, shared with the background tasks that fill them.
pub(super) type PrettyCache = Arc<Mutex<PrettyRenderings>>;

#[cfg(test)]
mod tests;
