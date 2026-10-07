//! The loaded key's fields: `Field`/`Fields` are the parsed model everything
//! else in `key_detail` reads from. The search bar applied to them lives in
//! `search_filter` (`SearchFilter`/`SearchTerm`).

use rustc_hash::FxHashMap;
use std::sync::{Arc, Mutex, OnceLock};

use rayon::prelude::*;
use rust_i18n::t;
use serde::Serialize;
use serde_json::Value;

use crate::backend::LockExt;

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
    // lowercased on first use by a case-insensitive search, then kept for
    // every later keystroke/badge — `raw` especially can be a large JSON
    // blob, not worth re-lowering on every pass over the fields
    lower_name: OnceLock<String>,
    lower_raw: OnceLock<String>,
}

impl Field {
    pub(super) fn new(name: String, raw: String, json: Option<Value>) -> Field {
        Field { name, raw, json, lower_name: OnceLock::new(), lower_raw: OnceLock::new() }
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

pub(super) fn find_field(current: &Mutex<Fields>, name: &str) -> Option<Arc<Field>> {
    current.lock_recover().iter().find(|f| f.name == name).cloned()
}

/// JSON-parses each hash field's raw string when possible; non-JSON keeps
/// only its text. Parses spread across cores, and the map is consumed so
/// names/strings move into `Field` instead of cloning — matters at scale.
pub(super) fn build_fields(details_value: Value) -> Fields {
    match details_value {
        Value::Object(map) => map
            .into_iter()
            .collect::<Vec<_>>()
            .into_par_iter()
            .map(|(name, raw)| {
                let raw = match raw {
                    Value::String(s) => s,
                    other => other.to_string(),
                };
                let json = serde_json::from_str::<Value>(&raw).ok();
                Arc::new(Field::new(name, raw, json))
            })
            .collect(),
        _ => Vec::new(),
    }
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

/// Open-row renderings by field and by whether escaped; shared with the background
/// tasks that fill it. `Arc` per entry so serving one is a cheap clone.
pub(super) type PrettyCache = Arc<Mutex<FxHashMap<(String, bool), Arc<Vec<ColoredParagraph>>>>>;
