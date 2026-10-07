//! Small constructors shared by more than one submodule's tests.

#![cfg(test)]

use std::sync::Arc;

use serde_json::{Value, json};

use super::field::Field;

pub(super) fn field(name: &str, json: Option<Value>) -> Arc<Field> {
    Arc::new(Field::new(name.into(), String::new(), json))
}

pub(super) fn records_field(i: usize) -> Arc<Field> {
    field(
        &format!("field_{i}"),
        Some(json!({
            "field_index": i,
            "generated": true,
            "records": [
                {"record_id": i * 10, "label": "a", "details": {"priority": 1}},
                {"record_id": i * 10 + 1, "label": "b", "details": {"priority": 2}},
            ],
        })),
    )
}
