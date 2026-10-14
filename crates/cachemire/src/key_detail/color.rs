//! Syntax-highlight colors for JSON values, shared by every place that turns
//! a `Value` into colored `StyledText` (previews, pretty text, table cells).

use slint::Color;

use crate::theme_colors::{JSON_BOOL, JSON_ESCAPE, JSON_KEY, JSON_NUMBER, JSON_PUNCT, JSON_STRING};

pub(super) fn color_for_kind(kind: &str) -> Option<Color> {
    match kind {
        "key" => Some(JSON_KEY),
        "string" => Some(JSON_STRING),
        "bool" => Some(JSON_BOOL),
        "number" => Some(JSON_NUMBER),
        "punct" => Some(JSON_PUNCT),
        "escape" => Some(JSON_ESCAPE),
        _ => None, // null / anything else: falls back to StyledText's default-color
    }
}
