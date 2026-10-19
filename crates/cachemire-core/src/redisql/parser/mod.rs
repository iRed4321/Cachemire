//! Recursive descent over a statement's tokens: a query's clauses, in any order,
//! or a `var` declaration. Each error points at the token it is about.

use std::ops::Range;


use super::{Located, QueryError};
use super::ast::{DEFAULT_LIMIT, Join, MAX_LIMIT, Query, Select, Source, WhereCond, WhereOp};
use super::lexer::{Tok, Token};

const CLAUSES: [&str; 5] = ["from", "join", "where", "select", "limit"];
/// The clauses a query may hold only one of (JOIN may come back).
const SINGLE_CLAUSES: [&str; 4] = ["from", "where", "select", "limit"];

struct Parser<'a> {
    src: &'a str,
    tokens: &'a [Token],
    pos: usize,
}

/// A name written `alias.field`, or just `field` (its alias is then left for the caller).
struct Ref {
    alias: Option<(String, Range<usize>)>,
    field: String,
    span: Range<usize>,
}

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<&'a Token> {
        self.tokens.get(self.pos)
    }

    fn next(&mut self) -> Option<&'a Token> {
        let token = self.tokens.get(self.pos);
        self.pos += token.is_some() as usize;
        token
    }

    /// Where an error about the token at hand goes: that token, or the last one
    /// at the end of the statement.
    fn here(&self) -> Option<Range<usize>> {
        self.peek().or(self.tokens.last()).map(|t| t.span.clone())
    }

    fn at_keyword(&self, word: &str) -> bool {
        self.peek().is_some_and(|t| t.is_keyword(word))
    }

    fn at_clause_end(&self) -> bool {
        self.peek().is_none_or(|t| CLAUSES.iter().any(|c| t.is_keyword(c)))
    }

    fn eat_keyword(&mut self, word: &str) -> bool {
        let found = self.at_keyword(word);
        self.pos += found as usize;
        found
    }

    fn eat(&mut self, tok: &Tok) -> bool {
        let found = self.peek().is_some_and(|t| &t.tok == tok);
        self.pos += found as usize;
        found
    }

    fn error(&self, error: QueryError) -> Located {
        Located::parse(error, self.here())
    }

    fn string(&mut self) -> Option<(String, Range<usize>)> {
        match self.peek() {
            Some(Token { tok: Tok::Str(s), span }) => {
                self.pos += 1;
                Some((s.clone(), span.clone()))
            }
            _ => None,
        }
    }

    fn name(&mut self) -> Option<(String, Range<usize>)> {
        match self.peek() {
            Some(Token { tok: Tok::Word(w), span }) => {
                self.pos += 1;
                Some((w.clone(), span.clone()))
            }
            _ => None,
        }
    }

    fn reference(&mut self) -> Option<Ref> {
        let (first, first_span) = self.name()?;
        if !self.eat(&Tok::Dot) {
            return Some(Ref { alias: None, field: first, span: first_span });
        }
        let (field, field_span) = self.name()?;
        Some(Ref { alias: Some((first, first_span.clone())), field, span: first_span.start..field_span.end })
    }

    /// A literal: a string, a number or a bare word (`true`).
    fn value(&mut self) -> Option<String> {
        match &self.peek()?.tok {
            Tok::Str(s) | Tok::Number(s) | Tok::Word(s) => {
                let value = s.clone();
                self.pos += 1;
                Some(value)
            }
            _ => None,
        }
    }

    /// The text from token `start` to the clause's end, to quote in a message.
    fn text_from(&self, start: usize) -> &'a str {
        let from = self.tokens.get(start).map_or(self.src.len(), |t| t.span.start);
        let mut end = self.pos;
        while end < self.tokens.len() && !CLAUSES.iter().any(|c| self.tokens[end].is_keyword(c)) {
            end += 1;
        }
        let to = self.tokens.get(end.saturating_sub(1)).map_or(from, |t| t.span.end).max(from);
        self.src[from..to].trim()
    }

    fn from(&mut self) -> Result<Source, Located> {
        let bad = |p: &Self| p.error(QueryError::InvalidFrom);
        if !self.eat_keyword("key") {
            return Err(bad(self));
        }
        let (key, key_span) = self.string().ok_or_else(|| bad(self))?;
        // `AS alias` is optional for a single-key query: it defaults to "src"
        if !self.eat_keyword("as") {
            return if self.at_clause_end() { Ok(Source { key, key_span, alias: "src".to_string() }) } else { Err(bad(self)) };
        }
        let (alias, _) = self.name().ok_or_else(|| bad(self))?;
        Ok(Source { key, key_span, alias })
    }

    fn join(&mut self) -> Result<Join, Located> {
        let bad = |p: &Self| p.error(QueryError::InvalidJoin);
        if !self.eat_keyword("key") {
            return Err(bad(self));
        }
        let (key, key_span) = self.string().ok_or_else(|| bad(self))?;
        if !self.eat_keyword("as") {
            return Err(bad(self));
        }
        let (alias, alias_span) = self.name().ok_or_else(|| bad(self))?;
        if !self.eat_keyword("on") {
            return Err(bad(self));
        }
        let left = self.reference().ok_or_else(|| bad(self))?;
        if !self.eat(&Tok::Eq) {
            return Err(bad(self));
        }
        let right = self.reference().ok_or_else(|| bad(self))?;
        let on_span = left.span.start..right.span.end;
        // a side with no `alias.` is the joined key's own
        let side = |r: Ref| (r.alias.unwrap_or_else(|| (alias.clone(), r.span.clone())), r.field);
        let ((left_alias, left_span), left_field) = side(left);
        let ((right_alias, right_span), right_field) = side(right);
        if left_alias.eq_ignore_ascii_case(&right_alias) {
            return Err(Located::parse(QueryError::JoinOnItself, Some(on_span)));
        }
        let (anchor_alias, anchor_span, anchor_field, join_field) = if alias.eq_ignore_ascii_case(&right_alias) {
            (left_alias, left_span, left_field, right_field)
        } else if alias.eq_ignore_ascii_case(&left_alias) {
            (right_alias, right_span, right_field, left_field)
        } else {
            return Err(Located::parse(QueryError::OnWithoutOwnAlias(alias.to_string()), Some(on_span)));
        };
        Ok(Join { key, key_span, alias, alias_span, anchor_alias, anchor_span, anchor_field, join_field })
    }

    fn condition(&mut self) -> Result<WhereCond, Located> {
        let start = self.pos;
        let bad = |p: &Self| Located::parse(QueryError::InvalidWhere(p.text_from(start).to_string()), p.here());
        let r = self.reference().ok_or_else(|| bad(self))?;
        let (alias, alias_span) = r.alias.unwrap_or((String::new(), r.span.clone()));
        let (op, values) = if self.eat(&Tok::Eq) {
            (WhereOp::Eq, vec![self.value().ok_or_else(|| bad(self))?])
        } else if self.eat_keyword("in") {
            (WhereOp::In, self.list()?)
        } else {
            return Err(bad(self));
        };
        if values.iter().any(|v| v.trim().is_empty()) {
            return Err(Located::parse(QueryError::InvalidWhere(self.text_from(start).to_string()), Some(r.span)));
        }
        Ok(WhereCond { alias, alias_span, field: r.field, op, values })
    }

    /// `[a, 'b', 3]` or `(a, 'b', 3)`.
    fn list(&mut self) -> Result<Vec<String>, Located> {
        let start = self.pos;
        let bad = |p: &Self| Located::parse(QueryError::InWithoutList(p.text_from(start).to_string()), p.here());
        let close = match self.peek().map(|t| &t.tok) {
            Some(Tok::Open('[')) => ']',
            Some(Tok::Open('(')) => ')',
            _ => return Err(bad(self)),
        };
        let open_span = self.next().map(|t| t.span.clone());
        let mut values = Vec::new();
        loop {
            if self.eat(&Tok::Close(close)) {
                break;
            }
            if self.eat(&Tok::Comma) {
                continue;
            }
            values.push(self.value().ok_or_else(|| bad(self))?);
        }
        if values.is_empty() {
            return Err(Located::parse(QueryError::EmptyInList, open_span));
        }
        Ok(values)
    }

    fn select_item(&mut self) -> Result<Select, Located> {
        let star = |alias: String, alias_span| Select { alias, alias_span, field: "*".to_string(), explicit_alias: true };
        if let Some(t) = self.peek().filter(|t| t.tok == Tok::Star) {
            self.pos += 1;
            return Ok(star("*".to_string(), t.span.clone()));
        }
        let bad = |p: &Self| p.error(QueryError::InvalidSelect);
        let (first, first_span) = self.name().ok_or_else(|| bad(self))?;
        if !self.eat(&Tok::Dot) {
            return Ok(Select { alias: String::new(), alias_span: first_span, field: first, explicit_alias: false });
        }
        if self.eat(&Tok::Star) {
            return Ok(star(first, first_span));
        }
        let (field, _) = self.name().ok_or_else(|| bad(self))?;
        Ok(Select { alias: first, alias_span: first_span, field, explicit_alias: true })
    }

    /// The LIMIT's number; a negative one is 0, which the query then raises to 1.
    fn limit(&mut self) -> Result<usize, Located> {
        match self.peek().map(|t| &t.tok) {
            Some(Tok::Number(n)) => {
                let limit = n.parse::<i64>().map_err(|_| self.error(QueryError::InvalidLimit))?;
                self.pos += 1;
                Ok(usize::try_from(limit).unwrap_or(0))
            }
            _ => Err(self.error(QueryError::InvalidLimit)),
        }
    }

    /// After a clause, the next token must start another (or the statement must end).
    fn end_of_clause(&self) -> Result<(), Located> {
        match self.peek() {
            Some(t) if !self.at_clause_end() => Err(Located::parse(QueryError::UnexpectedText(self.text_from(self.pos).to_string()), Some(t.span.clone()))),
            _ => Ok(()),
        }
    }

    fn query(&mut self) -> Result<Query, Located> {
        if self.tokens.is_empty() {
            return Err(Located::parse(QueryError::EmptyQuery, None));
        }
        let mut base: Option<Source> = None;
        let (mut joins, mut wheres, mut selects, mut limit) = (Vec::new(), Vec::new(), Vec::new(), DEFAULT_LIMIT);
        let mut seen: Vec<&str> = Vec::new();
        while let Some(clause) = self.next() {
            if let Some(single) = SINGLE_CLAUSES.into_iter().find(|c| clause.is_keyword(c)) {
                if seen.contains(&single) {
                    return Err(Located::parse(QueryError::RepeatedClause(single.to_uppercase()), Some(clause.span.clone())));
                }
                seen.push(single);
            }
            if clause.is_keyword("from") {
                base = Some(self.from()?);
            } else if clause.is_keyword("join") {
                joins.push(self.join()?);
            } else if clause.is_keyword("where") {
                while !self.at_clause_end() {
                    wheres.push(self.condition()?);
                    if !self.eat_keyword("and") {
                        break;
                    }
                }
            } else if clause.is_keyword("select") {
                while !self.at_clause_end() {
                    selects.push(self.select_item()?);
                    if !self.eat(&Tok::Comma) {
                        break;
                    }
                }
            } else if clause.is_keyword("limit") {
                limit = self.limit()?;
            } else {
                self.pos -= 1;
                return Err(Located::parse(QueryError::UnknownClause(self.text_from(self.pos).to_string()), Some(clause.span.clone())));
            }
            self.end_of_clause()?;
        }
        let base = base.ok_or_else(|| Located::parse(QueryError::MissingFrom, None))?;
        resolve(Query { base, joins, wheres, selects, limit: limit.clamp(1, MAX_LIMIT) })
    }
}

/// Fills in the FROM key's alias where a field has none, and checks every alias
/// named is one the query defines.
fn resolve(mut query: Query) -> Result<Query, Located> {
    let base_alias = query.base.alias.clone();
    for w in query.wheres.iter_mut().filter(|w| w.alias.is_empty()) {
        w.alias.clone_from(&base_alias);
    }
    for s in query.selects.iter_mut().filter(|s| s.alias.is_empty()) {
        s.alias.clone_from(&base_alias);
    }
    let mut known: Vec<String> = vec![base_alias.to_lowercase()];
    for join in &query.joins {
        let alias = join.alias.to_lowercase();
        if known.contains(&alias) {
            return Err(Located::parse(QueryError::DuplicateAlias(join.alias.clone()), Some(join.alias_span.clone())));
        }
        if !known.contains(&join.anchor_alias.to_lowercase()) {
            let error = QueryError::UnknownJoinAlias { alias: join.alias.clone(), other: join.anchor_alias.clone() };
            return Err(Located::parse(error, Some(join.anchor_span.clone())));
        }
        known.push(alias);
    }
    if let Some(w) = query.wheres.iter().find(|w| !known.contains(&w.alias.to_lowercase())) {
        return Err(Located::parse(QueryError::UnknownWhereAlias(w.alias.clone()), Some(w.alias_span.clone())));
    }
    if let Some(s) = query.selects.iter().find(|s| s.alias != "*" && !known.contains(&s.alias.to_lowercase())) {
        return Err(Located::parse(QueryError::UnknownSelectAlias(s.alias.clone()), Some(s.alias_span.clone())));
    }
    Ok(query)
}

/// The query `tokens` (from `src`) spell out.
pub fn parse_query(src: &str, tokens: &[Token]) -> Result<Query, Located> {
    Parser { src, tokens, pos: 0 }.query()
}

/// A `var name = value` declaration as its name and value.
pub fn parse_declaration(src: &str, tokens: &[Token]) -> Result<(String, String), Located> {
    let mut p = Parser { src, tokens, pos: 0 };
    let bad = |p: &Parser| p.error(QueryError::InvalidDeclaration);
    if !p.eat_keyword("var") {
        return Err(bad(&p));
    }
    let (name, _) = p.name().ok_or_else(|| bad(&p))?;
    if !p.eat(&Tok::Eq) {
        return Err(bad(&p));
    }
    let value = p.value().filter(|v| !v.is_empty()).ok_or_else(|| bad(&p))?;
    if p.peek().is_some() {
        return Err(bad(&p));
    }
    Ok((name, value))
}

#[cfg(test)]
mod tests;
