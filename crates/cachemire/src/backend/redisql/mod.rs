//! Redisql: a small query language that pulls rows out of keys holding JSON (a hash
//! of JSON objects, or a string holding an object/array) and joins them on matching
//! field values. Example and clause rules are in the README.

mod ast;
mod exec;
mod lexer;
mod parser;
mod script;

use rustc_hash::FxHashMap;
use std::ops::Range;

use super::error::AppError;
use super::session::Session;
use super::state::AppState;

pub use exec::{QueryOutput, field_names};
pub use script::{is_declaration, split_statements, variable_ranges};

/// An error, with the byte range of the statement it is about when there is one.
#[derive(Debug)]
pub struct Located {
    pub error: AppError,
    pub span: Option<Range<usize>>,
}

impl Located {
    fn parse(message: impl Into<String>, span: Option<Range<usize>>) -> Self {
        Self { error: AppError::parse(message), span }
    }

    /// Points an error at `span`, for `map_err`.
    fn at(span: &Range<usize>) -> impl FnOnce(AppError) -> Self + '_ {
        move |error| Self { error, span: Some(span.clone()) }
    }

    /// The same error, with its range moved `by` bytes further.
    fn shifted(self, by: usize) -> Self {
        Self { span: self.span.map(|s| s.start + by..s.end + by), ..self }
    }

    #[cfg(test)]
    fn message(&self) -> &str {
        self.error.message()
    }
}

impl std::fmt::Display for Located {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.error.fmt(f)
    }
}

impl From<AppError> for Located {
    fn from(error: AppError) -> Self {
        Self { error, span: None }
    }
}

/// What one statement of a script produced: a query's rows, or the error that ended
/// the script, with where in the script it is when that can be told.
pub struct StatementResult {
    pub output: Result<QueryOutput, AppError>,
    pub error_span: Option<Range<usize>>,
}

/// Parses `text`, one statement, as a query (for the tests).
#[cfg(test)]
fn parse_query(text: &str) -> Result<ast::Query, Located> {
    parser::parse_query(text, &lexer::tokenize(text)?)
}

async fn run_statement(state: &AppState, connection_id: Option<&str>, text: &str, vars: &FxHashMap<String, String>) -> Result<QueryOutput, Located> {
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

