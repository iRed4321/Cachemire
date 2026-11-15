---
name: redisql-feature
description: Use when changing the Redisql query language (keyword, function, parsing, execution, completion, history) or its panel in the UI.
---

# Redisql feature

Core, `crates/cachemire-core/src/redisql/`: `lexer.rs` (tokens), `ast.rs`, `parser/mod.rs`, `exec/mod.rs`,
`complete/mod.rs` (autocomplete), `history/`, `script/`. Tests: `redisql/tests.rs`; real-Redis ones in `src/e2e/redisql`.
UI: `crates/cachemire/src/redisql/mod.rs` and `ui/components/query-panel.slint`.

Order: lexer -> ast -> parser -> exec -> complete -> UI. Update completion whenever syntax changes.
Run `cargo test`, and `cargo xtask e2e` for execution changes. Add tests only when asked.

Avoid: Redis calls in the parser; panics on user input (return typed errors); skipping the completion update.
