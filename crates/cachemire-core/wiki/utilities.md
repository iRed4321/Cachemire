# Utilities: JSON path, export files and small helpers

Small, mostly self-contained modules that do not belong to a bigger chapter. Two of them
(`json_path`, `export_files`) are described in full; the rest are listed with where they are
documented.

## Where to look

| Module | File | Section |
|--------|------|---------|
| [`json_path`](crate::json_path) | `src/json_path/mod.rs` | [JSON path](#json-path) |
| [`export_files`](crate::export_files) | `src/export_files/mod.rs` | [Export files](#export-files) |
| [`server_info`](crate::server_info) | `src/server_info.rs` | [other helpers](#other-small-modules) |
| [`redis_value_type`](crate::redis_value_type) | `src/redis_value_type.rs` | [other helpers](#other-small-modules) |
| [`key_name`](crate::key_name) | `src/key_name/mod.rs` | [explorer](crate::wiki::explorer) |
| [`now_ms`](crate::now_ms) | `src/lib.rs` | [other helpers](#other-small-modules) |

## JSON path

A tiny JSONPath-style query for the key panel's filter bar. [`parse`](crate::json_path::parse)
turns a filter string into a [`JsonPath`](crate::json_path::JsonPath);
[`JsonPath::select`](crate::json_path::JsonPath::select) returns everything it matches in a
`serde_json::Value`, in document order. It has no dependency on Redis.

### Syntax

```text
$                 optional leading root; "$" alone selects the value itself
key               an object key; on an array a numeric key is an index
a.b.c             nested keys, separated by '.'
*                 every value of an object or array (also written [*])
[3]               an array index
['key'] ["key"]   a quoted key (may hold dots and spaces); no escapes inside
..key             "key" at the current value and at every depth below it
```

A path may start with a key directly (`records.*`); after `$` or any segment a separator (`.` or `[`)
is needed. Spaces inside brackets are skipped; a name is trimmed. The empty string is not a filter:
callers test for it first (the doc of `parse` says so), `$` is the explicit "whole value".

### Model

A path is a list of `Segment`s (`Key`, `Index`, `Wildcard`, `Descend(Box<Segment>)`).
`select` starts with the root and applies each segment to every value found so far, collecting the
children; it returns early with an empty list as soon as a step finds nothing. `Descend(x)` applies
`x` at the value and then recurses into every child.

Helpers used by the key panel to interpret the field/value layout (a hash is shown as rows of
`field` and `value`):

| Method | Meaning |
|--------|---------|
| `is_identity` | the path is `$` |
| `is_value_alias_only` | the path is just `value`, the name the layout gives a field's value |
| `without_value_alias` | the path minus a leading `value` key (`value.records` read as `records`) |
| `without_trailing_wildcard` | the path minus a trailing `*` (`records.*` as `records`) |

### Errors

[`ParseError`](crate::json_path::ParseError) variants name what was expected and carry a
[`Found`](crate::json_path::Found): a character with its position counted from 1, or the end of the
filter. Examples: `expected a key, found '.' at position 3`, `a quoted key is missing its closing
quote`, `'99999999999999999999' is too big an index`. The error converts to an
[`Error`](crate::error::Error) of kind `Parse` (see [errors](crate::wiki::errors)); the app localises
it.

## Export files

[`export_files`](crate::export_files) writes keys to JSON files in a folder. It reads each key with
[`explorer::read_for_export`](crate::explorer::read_for_export) on a background
[`Session`](crate::session::Session) and streams the text to disk.

### Plan

A [`Plan`](crate::export_files::Plan) says what to export, as key ids:

```text
Plan {
  folders : Vec<(prefix, Vec<key id>)>   whole folders: their prefix and every key under it
  keys    : Vec<key id>                  lone keys
  group   : bool                         lone keys go one file per parent folder instead of one each
}
```

`total()` counts all keys; `sole_file()` returns the file name when the plan writes exactly one file,
so the dialog can offer a name for it.

### Output files

The private `outputs` function turns a plan into `Output`s with unique file names:

| Output | File name | Content |
|--------|-----------|---------|
| `Items` (a folder, or a group of lone keys) | `redis-folder-<prefix>.json` (`root` or `root-keys` when empty) | `{"prefix": ..., "items": [ {key,type,ttl,value}, ... ], "totalKeys": n}` |
| `Single` (a lone key) | `redis-key-<key>.json` | one `{key, type, ttl, value[, truncated]}` object |

- `file_safe` replaces characters a file name cannot hold (`< > : " / \ | ? *`, controls) by `_`
  (runs collapse), trims, caps the stem at 120 characters, and falls back to `export`.
- `unique_name` numbers a repeated name (`-2`, `-3`), comparing lower-cased (Windows and macOS file
  names ignore case).
- When `group` is set with a single lone key and no folder, the key is written as a `Single` file.

### Writing

[`run`](crate::export_files::run)`(state, connection_id, plan, dir, file_name, cancel, progress)`:

```text
open a background session; create dir
for each output:
    path = dir/file_name   if the plan writes one file and a name was given (replaces it)
           free_path(dir, file)  otherwise: "name (2).json" if it exists, never replaces
    for each key:
        stop if cancel is set
        read_for_export:  Err(Redis|Denied) -> skip, count `skipped`, log
                          Ok(None)          -> key gone, left out
                          Ok(Some)          -> open the Sink on first item, append it
        progress(done, total)
    close the array and the object, flush
```

Details:

- Output is streamed: the file head (`{"prefix": ..., "items": [`) is written at the first key and
  items are appended with indentation, so a large folder is never held in memory.
- The `Sink` writes on a blocking thread fed by a bounded channel (`SINK_QUEUE` 32 chunks): a slow
  disk makes the export wait rather than grow memory. The file is created through
  [`persist::create_private`](crate::persist::create_private) (owner-only permissions). It stops at
  the first I/O error, which comes back through `write` or `join`.
- On cancel or on an error the file being written is removed (`Sink::discard` only removes a file
  the thread actually created), and `run` returns the error, or an [`Outcome`](crate::export_files::Outcome)
  with `cancelled: true`. Files already finished stay.
- A file with no key actually written (all gone) is not created.
- Keys over `EXPORT_MAX_ITEMS` entries are cut and flagged `"truncated": true` in their item.
- [`Outcome`](crate::export_files::Outcome) reports `keys` done, `cancelled`, and `skipped`.
- I/O errors become [`FileError`](crate::error::FileError), so kind `File`.

## Other small modules

Not part of this chapter's own logic, but small and often met:

- [`server_info`](crate::server_info): `used_memory` and `format_bytes`, covered in
  [explorer](crate::wiki::explorer).
- [`redis_value_type`](crate::redis_value_type): `RedisValueType`, covered in
  [explorer](crate::wiki::explorer).
- [`key_name`](crate::key_name): key and field byte/text encoding, covered in
  [explorer](crate::wiki::explorer) and [session](crate::wiki::session).
- [`now_ms`](crate::now_ms) (in `lib.rs`): milliseconds since the Unix epoch, 0 if the clock is
  before it. Used for timestamps such as the Redisql history's `executed_at`.

Modules described elsewhere: `connection`, `aws`, `secret`, `secret_vault`
([connection](crate::wiki::connection), [secrets](crate::wiki::secrets), [aws](crate::wiki::aws));
`settings` and `persist` ([settings](crate::wiki::settings)); `error` ([errors](crate::wiki::errors));
`state`, `runtime`, `i18n`, `link` and `updates` are covered by the app crate's wiki
(architecture, state, i18n and platform chapters).
