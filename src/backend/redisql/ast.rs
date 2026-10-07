//! A parsed query. The spans (byte ranges in the statement) point an error found
//! later, an unknown alias or a missing key, at the place it is written.

use std::ops::Range;

pub const DEFAULT_LIMIT: i64 = 250;
pub const MAX_LIMIT: i64 = 5000;

#[derive(Debug, Clone, PartialEq)]
pub struct Source {
    pub key: String,
    pub key_span: Range<usize>,
    pub alias: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Join {
    pub key: String,
    pub key_span: Range<usize>,
    pub alias: String,
    pub alias_span: Range<usize>,
    /// the alias already joined that this one links to, and the field compared on each side
    pub anchor_alias: String,
    pub anchor_span: Range<usize>,
    pub anchor_field: String,
    pub join_field: String,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WhereOp {
    Eq,
    In,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WhereCond {
    pub alias: String,
    pub alias_span: Range<usize>,
    pub field: String,
    pub op: WhereOp,
    pub values: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Select {
    pub alias: String,
    pub alias_span: Range<usize>,
    pub field: String,
    /// the alias was written (`alias.field`), not filled in from the FROM key's own:
    /// decides whether a single-source query's columns are still prefixed
    pub explicit_alias: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Query {
    pub base: Source,
    pub joins: Vec<Join>,
    pub wheres: Vec<WhereCond>,
    pub selects: Vec<Select>,
    pub limit: i64,
}
