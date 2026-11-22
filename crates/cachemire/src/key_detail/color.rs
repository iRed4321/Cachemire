//! Syntax-highlight colors (the syntax theme's) for JSON values, shared by every place that turns
//! a `Value` into colored `StyledText` (previews, pretty text, table cells).

use slint::Color;

use crate::theme::paint;

pub(super) fn color_for_kind(kind: &str) -> Option<Color> {
    let paint = paint();
    match kind {
        "key" => Some(paint.key),
        "string" => Some(paint.string),
        "bool" => Some(paint.bool),
        "number" => Some(paint.number),
        "punct" => Some(paint.punct),
        "escape" => Some(paint.escape),
        "too-long" => Some(paint.too_long),
        _ => None, // null / anything else: falls back to StyledText's default-color
    }
}
