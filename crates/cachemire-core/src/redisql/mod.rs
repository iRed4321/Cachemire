//! Redisql: a small query language that pulls rows out of keys holding JSON (a hash
//! of JSON objects, or a string holding an object/array) and joins them on matching
//! field values. Example and clause rules are in the README.

mod ast;
pub mod complete;
mod exec;
pub mod history;
mod lexer;
mod parser;
mod script;

use rustc_hash::FxHashMap;
use std::ops::Range;

use crate::error::{Error, ErrorKind};
use crate::session::Session;
use crate::state::AppState;

pub use exec::{QueryOutput, field_names};
pub use script::{is_declaration, split_statements, variable_ranges};

/// Why a query can't be understood, or can't run on what its keys hold.
#[derive(Debug, thiserror::Error)]
pub enum QueryError {
    #[error("Unterminated quote: {0}")]
    UnterminatedQuote(String),
    #[error("Unexpected text: {0}")]
    UnexpectedText(String),
    #[error("Unknown variable: &{0}")]
    UnknownVariable(String),
    #[error("Invalid FROM clause. Example: FROM KEY 'my:key' AS src")]
    InvalidFrom,
    #[error("Invalid JOIN clause. Example: JOIN KEY 'my:key' AS b ON a.id = b.refId")]
    InvalidJoin,
    #[error("A JOIN's ON must link two different aliases.")]
    JoinOnItself,
    #[error("The ON clause must name the JOIN's own alias ({0}).")]
    OnWithoutOwnAlias(String),
    #[error("Invalid WHERE condition: {0}")]
    InvalidWhere(String),
    #[error("IN needs a list in [] or (): {0}")]
    InWithoutList(String),
    #[error("IN's list is empty.")]
    EmptyInList,
    #[error("Invalid SELECT clause.")]
    InvalidSelect,
    #[error("LIMIT must be a whole number.")]
    InvalidLimit,
    #[error("The query is empty.")]
    EmptyQuery,
    #[error("{0} can only appear once.")]
    RepeatedClause(String),
    #[error("Unrecognized clause: {0}")]
    UnknownClause(String),
    #[error("Missing FROM clause.")]
    MissingFrom,
    #[error("Duplicate alias: {0}")]
    DuplicateAlias(String),
    #[error("JOIN {alias} references an unknown alias: {other}")]
    UnknownJoinAlias { alias: String, other: String },
    #[error("Unknown alias in WHERE: {0}")]
    UnknownWhereAlias(String),
    #[error("Unknown alias in SELECT: {0}")]
    UnknownSelectAlias(String),
    #[error("Invalid variable declaration. Example: var companyId = 255;")]
    InvalidDeclaration,
    #[error("Key not found: {0}")]
    KeyNotFound(String),
    #[error("{key} is a {kind}: only a hash of JSON objects, or a string holding one, can be queried.")]
    NotQueryable { key: String, kind: String },
    #[error("A non-object value was found in {key} (field {field}): {kind}")]
    NonObjectField { key: String, field: String, kind: &'static str },
    #[error("Invalid JSON in {key} (field {field}): {source}")]
    FieldJson { key: String, field: String, source: serde_json::Error },
    #[error("{key} holds JSON, but not an object or an array of objects: {kind}")]
    NonObjectValue { key: String, kind: &'static str },
    #[error("Invalid JSON in {key}: {source}")]
    ValueJson { key: String, source: serde_json::Error },
}

impl QueryError {
    pub fn kind(&self) -> ErrorKind {
        match self {
            QueryError::KeyNotFound(_) => ErrorKind::NotFound,
            QueryError::NotQueryable { .. }
            | QueryError::NonObjectField { .. }
            | QueryError::FieldJson { .. }
            | QueryError::NonObjectValue { .. }
            | QueryError::ValueJson { .. } => ErrorKind::Invalid,
            _ => ErrorKind::Parse,
        }
    }
}

/// An error, with the byte range of the statement it is about when there is one.
#[derive(Debug)]
pub struct Located {
    pub error: Error,
    pub span: Option<Range<usize>>,
}

impl Located {
    fn parse(error: QueryError, span: Option<Range<usize>>) -> Self {
        Self { error: error.into(), span }
    }

    /// Points an error at `span`, for `map_err`.
    fn at(span: &Range<usize>) -> impl FnOnce(Error) -> Self + '_ {
        move |error| Self { error, span: Some(span.clone()) }
    }

    /// The same error, with its range moved `by` bytes further.
    fn shifted(self, by: usize) -> Self {
        Self { span: self.span.map(|s| s.start + by..s.end + by), ..self }
    }

    #[cfg(test)]
    fn message(&self) -> String {
        self.error.to_string()
    }
}

impl std::fmt::Display for Located {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.error.fmt(f)
    }
}

impl From<Error> for Located {
    fn from(error: Error) -> Self {
        Self { error, span: None }
    }
}

/// What one statement of a script produced: a query's rows, or the error that ended
/// the script, with where in the script it is when that can be told.
pub struct StatementResult {
    pub output: Result<QueryOutput, Error>,
    pub error_span: Option<Range<usize>>,
}

/// Parses `text`, one statement, as a query (for the tests).
#[cfg(test)]
fn parse_query(text: &str) -> Result<ast::Query, Located> {
    parser::parse_query(text, &lexer::tokenize(text)?)
}

async fn run_statement(
    state: &AppState,
    connection_id: Option<&str>,
    text: &str,
    vars: &FxHashMap<String, String>,
) -> Result<QueryOutput, Located> {
    let mut tokens = lexer::tokenize(text)?;
    script::substitute_variables(&mut tokens, vars)?;
    let query = parser::parse_query(text, &tokens)?;
    let mut session = Session::open(state, connection_id).await?;
    exec::execute(&mut session, &query).await
}

/// Runs the statements of `script` in order against the active (or `connection_id`) connection:
/// a `var` declaration sets a variable for those after it, a query gives a result. It stops at
/// the first error, which is the last result.
pub async fn run_script(state: &AppState, connection_id: Option<&str>, script: &str) -> Vec<StatementResult> {
    let mut results = Vec::new();
    let mut vars = FxHashMap::default();
    for statement in split_statements(script) {
        let text = &script[statement.range.clone()];
        let outcome = if is_declaration(text) {
            match lexer::tokenize(text).and_then(|tokens| parser::parse_declaration(text, &tokens)) {
                Ok((name, value)) => {
                    vars.insert(name, value);
                    continue;
                }
                Err(e) => Err(e),
            }
        } else {
            run_statement(state, connection_id, text, &vars).await
        };
        let failed = outcome.is_err();
        results.push(match outcome.map_err(|e| e.shifted(statement.range.start)) {
            Ok(output) => StatementResult { output: Ok(output), error_span: None },
            Err(e) => StatementResult { output: Err(e.error), error_span: e.span },
        });
        if failed {
            break;
        }
    }
    results
}

#[cfg(test)]
mod tests;
