# Session: per-connection Redis operations

[`Session`](crate::session::Session) is the one place where Redis commands are built and
their replies decoded. Everything above it (the explorer, Redisql, the app) speaks in
[`KeyName`](crate::key_name::KeyName)s and Rust strings; everything below it is the
connection pool of the [connection](crate::wiki::connection) chapter.

## Where to look

| File | What it holds |
|------|---------------|
| `src/session/mod.rs` | `Session`, `KeyScan`, the Lua scripts, the per-type reads and writes |
| `src/session/search.lua` | the server-side prefilter of the global search |
| `src/session/tests.rs` | unit tests of the reply decoding |
| `src/key_name/mod.rs` | `KeyName` and the `\xNN` text encoding of bytes |
| `src/explorer.rs` | the callers: see [explorer](crate::wiki::explorer) |

## What a Session is

```text
Session
  con        : RedisConnection      one multiplexed connection picked from the pool
  pool       : Arc<ConnectionPool>  kept to pick siblings and to share per-pool flags
  background : bool                 true when `con` is the pool's long-read connection
```

A session owns no state of its own about the server: it is a handle on one connection of a
[`ConnectionPool`](crate::connection::pool::ConnectionPool). Each connection is a
`redis::aio::ConnectionManager` ([`RedisConnection`](crate::connection::pool::RedisConnection)),
so many sessions can share one underlying socket; commands are bounded by the connection's
response timeout (see the [connection](crate::wiki::connection) chapter).

### Opening

| Constructor | Connection used | For |
|-------------|-----------------|-----|
| [`Session::open`](crate::session::Session::open) | `pool.pick()` | short commands a click triggers |
| [`Session::open_background`](crate::session::Session::open_background) | `pool.background()` | long reads: walks, searches, Redisql, exports |
| [`Session::sibling`](crate::session::Session::sibling) | same kind as the parent, from the same pool | commands run beside another session's |

Both `open` functions take `Option<&str>`: a connection id, or `None` for the active
connection. They call [`resolve_connection`](crate::state::resolve_connection) (which also
completes an SSH tunnel with the active SSH profile) and then ask the
[`ConnectionCache`](crate::connection::pool::ConnectionCache) for the pool, which is opened
on first use. How pools, tunnels and TLS are built is not repeated here.

The split between `pick` and `background` is the whole concurrency design: a long
`SCAN`/search/export runs on its own connection so it never queues in front of the short
commands a click makes. A `sibling` of a background session stays on the background
connection.

### Read-only is not enforced here

`Session` itself writes whatever it is told. The refusal for a read-only connection lives
one level up: the explorer's `write_session` and `write_background_session` call
[`ensure_writable`](crate::state::ensure_writable) before opening. Any new write path must
go through them (the rule is stated in the doc of `ensure_writable`).

## Keys are bytes

Redis key names and hash field names need not be UTF-8. The session boundary therefore
works with:

- [`KeyName`](crate::key_name::KeyName), the raw bytes of a key. Its `id()` is the text the
  UI holds: the name itself, with `\` written `\\` and every non-UTF-8 byte written `\xNN`;
  [`KeyName::from_id`](crate::key_name::KeyName::from_id) reads it back to the exact bytes.
- For hash fields, [`field_name`](crate::key_name::field_name) and
  [`field_bytes`](crate::key_name::field_bytes) do the same round trip, so a field that is
  not text can be shown and still addressed.
- Values are turned into text with [`display_bytes`](crate::key_name::display_bytes) (through
  the session's private `lossy` and `lossy_flagged` helpers): valid UTF-8 as is, each other
  byte as `\xNN`, plus a flag telling the caller that the value was not all text. That flag
  is how the UI knows a value is binary and must not be edited
  (see [`Error::BinaryData`](crate::error::Error::BinaryData)).

## Reads by type

Every command that can return a lot is bounded or paged. The constant
`READ_CHUNK` (1000) is the most entries one command reads of a big key: a bigger read takes
several commands, so none holds the server up for long or builds a huge reply.

| Type | Methods | How |
|------|---------|-----|
| any | [`key_type`](crate::session::Session::key_type), [`types`](crate::session::Session::types), [`type_and_ttl`](crate::session::Session::type_and_ttl) | `TYPE` alone, pipelined for many keys, or `TYPE` + `TTL` in one round trip |
| string | [`get`](crate::session::Session::get), [`string_len`](crate::session::Session::string_len), [`string_head`](crate::session::Session::string_head) | `GET`; `STRLEN`; `STRLEN` + `GETRANGE 0 max-1` pipelined |
| hash | [`hash_len`](crate::session::Session::hash_len), [`hash_scan`](crate::session::Session::hash_scan), [`hash_scan_names`](crate::session::Session::hash_scan_names), [`hash_values`](crate::session::Session::hash_values), [`hash_value_sizes`](crate::session::Session::hash_value_sizes), [`hash_exists`](crate::session::Session::hash_exists) | `HLEN`; one `HSCAN` step; `HSCAN ... NOVALUES`; `HMGET` per chunk; `HSTRLEN` pipelined per chunk; `HEXISTS` |
| list | [`list_head`](crate::session::Session::list_head) | `LLEN` + first `LRANGE` pipelined, then one `LRANGE` per chunk |
| set | [`set_len`](crate::session::Session::set_len), [`set_scan`](crate::session::Session::set_scan) | `SCARD`; one `SSCAN` step |
| zset | [`zset_head`](crate::session::Session::zset_head) | `ZCARD` + first `ZRANGE ... WITHSCORES` pipelined, then one per chunk |
| stream | [`stream_len`](crate::session::Session::stream_len), [`stream_head`](crate::session::Session::stream_head) | `XLEN`; `XRANGE - + COUNT n` repeated |

### Chunked reads

The "head" methods (`list_head`, `zset_head`, `stream_head`) return the first `count` entries.
They share one shape:

```text
first round trip : length command + first range (pipelined)
while items.len() < min(count, len):
    next range from items.len() to min(items.len() + READ_CHUNK, wanted) - 1
    stop early if the server returns nothing (the key shrank)
```

`list_head` and `zset_head` also return the total length, so the caller can tell whether the
result was cut. `stream_head` has no length to rely on, so it advances by id: each page starts at
`next_stream_id` of the last id read (`ms-seq` with `seq + 1`, rolling over to `ms + 1` when
`seq` is `u64::MAX`), and stops when a page comes back short. `count: None` reads the whole
stream.

`hash_values` and `hash_value_sizes` chunk over the field names they were given instead of over
the key. `hash_value_sizes` is what lets the explorer leave out a field whose value is bigger than
`MAX_FIELD_BYTES` before reading it (see [explorer](crate::wiki::explorer)).

### Scans never repeat a promise

`SCAN`, `HSCAN` and `SSCAN` may return a name twice. The session returns each page as it came
and leaves de-duplication to the caller (the explorer and Redisql both do it, with sets of names
already seen). The doc of [`KeyScan`](crate::session::KeyScan) says so as well.

[`KeyScan`](crate::session::KeyScan) is the keyspace cursor: it holds the `MATCH` pattern, the
`COUNT`, and the next cursor (`None` once the walk is over). [`KeyScan::next`](crate::session::KeyScan::next)
runs one `SCAN` step through a session and returns the page, `None` when finished. The session's private
`scan` method issues the command.

### Server differences handled here

- `HSCAN ... NOVALUES` exists only on Redis 7.4+. `hash_scan_names` tries it while the pool's
  shared flag `hscan_novalues` is true; the first "unknown option" error
  ([`Error::is_unknown_option`](crate::error::Error::is_unknown_option)) clears the flag for the
  whole pool and the call falls back to a plain `HSCAN` whose values are dropped.
- `string_set` writes with `SET ... KEEPTTL`; a Redis error (pre-6.0 servers) makes it read
  `PTTL` and write `SET ... PX ttl` instead.
- Scripts may be refused (no `EVAL` permission, a server without them): see the next section.
- `cluster_enabled` (a `pub(super)` helper) reads `INFO cluster` for `cluster_enabled:1`; a server
  that won't answer INFO is taken not to be a cluster. It is used by the connection code.

## Writes

Plain writes: `hash_set` (`HSET`), `hash_set_new` (`HSETNX`), `hash_delete` (`HDEL`),
`string_set`, `unlink` (`UNLINK`, memory freed in the background), `rename_new` and
`rename_new_many` (`RENAMENX`, the latter pipelined with per-pair results), `existing`
(pipelined `EXISTS`).

### Scripts for atomic edits

Four small Lua scripts (`static` `redis::Script`s, loaded by hash and re-sent when needed) make
multi-step edits atomic:

| Script | Effect |
|--------|--------|
| `RENAME_FIELD_SCRIPT` | `HSETNX` the new name with the new value, then `HDEL` the old: returns 0 when the new name exists |
| `CHECKED_HASH_SET_SCRIPT` | writes a hash field only if it still holds the expected value |
| `CHECKED_STRING_SET_SCRIPT` | the same for a string, with `KEEPTTL` |
| `CHECKED_RENAME_FIELD_SCRIPT` | rename a field if it still holds the expected value and the target name is free |

The "checked" variants implement optimistic concurrency for the editor: the UI remembers the
value it showed (`expected`) and the write is refused if someone else changed it meanwhile. They
answer with a [`Checked`](crate::session::Checked):

```text
Done                 written
Taken                the renamed field's new name already exists; nothing changed
Changed(Some(text))  the value is no longer the expected one; carries what is stored now
Changed(None)        the field or key is gone
```

`Changed` with a stored value that is not UTF-8 becomes
[`Error::BinaryData`](crate::error::Error::BinaryData) (the editor cannot hold it).

Every scripted method has a fallback: when
[`Error::is_script_refused`](crate::error::Error::is_script_refused) is true (no scripting, ACL,
settings) it logs at `info` and does the same thing as separate commands (check, then write).
That path is no longer atomic; the doc comments say "two commands". The scripts' raw replies are
decoded by `checked_of`/`changed`.

## Search support

The global search (a tab in the app, driven by
[`scan_entries`](crate::explorer::scan_entries)) needs the first entries of many keys. Two
implementations exist and the explorer picks one at run time.

### Plain pipelined path

[`Session::entry_heads`](crate::session::Session::entry_heads) takes a slice of
[`TypedKey`](crate::session::TypedKey)s and queues, per key, a type-appropriate read of at most
`limit` entries (`push_head_read`): `GETRANGE` for a string (first `SEARCH_STRING_BYTES`, 1 MB),
`HSCAN`, `LRANGE`, `SSCAN`, `ZRANGE ... WITHSCORES`, `XRANGE ... COUNT`. Errors are ignored
per key (`ignore_errors`), so a key that vanished or changed type yields no entries. Replies
are turned into `(field, value)` pairs by `entries_of_reply`; the mapping is:

```text
string -> [("", value)]
hash   -> [(field, value) ...]
list   -> [("[0]", item0), ("[1]", item1) ...]
set    -> [("", member) ...]
zset   -> [(score, member) ...]          (the score plays the field's part)
stream -> [(id, {"field":"value",...} as JSON text) ...]
```

### The Lua prefilter: search.lua

[`Session::search_script`](crate::session::Session::search_script) runs `search.lua` with the keys
as `KEYS` and `ARGV = [needle, depth, string_bytes]`. The script reads the same bounded head of each
key and returns, for each key that could hold a hit, `{key, type, status, entries}`.

```text
status  0   normal: entries holds field, value, field, value ... of entries that may match
status  1   a stream: not read by the script, the caller reads it itself
status -1   the key does not exist (left out of the answer)
```

Design points, all visible in the script:

- It is a prefilter, never a filter: it must never drop a real hit. The caller re-checks what it
  receives exactly.
- Matching is a case-insensitive substring test, done in Lua only for ASCII (`string.lower`);
  any text with a non-ASCII byte is always passed on, because only the Rust side lowercases
  Unicode the way the search does. A needle with non-ASCII letters therefore matches nothing in
  an all-ASCII text and everything non-ASCII passes.
- A key whose own name may match is kept even with no matching entry.
- It only reads. Streams are left to the caller (`status 1`) because their entries are not
  simply field/value pairs.
- The result type [`ScriptKey`](crate::session::ScriptKey) is `(key, type, status, flat entries)`.

If the script is refused the explorer sets a shared flag and uses the plain path from then on (see
[explorer](crate::wiki::explorer)).

## Sizes and bounds, at a glance

| Constant | Value | Meaning |
|----------|-------|---------|
| `READ_CHUNK` (session) | 1000 | entries per command on a big key |
| `SEARCH_STRING_BYTES` (session) | 1 MiB | how much of a string a search looks at |
| `MAX_STRING_BYTES` ([explorer](crate::explorer::MAX_STRING_BYTES)) | 4 MiB | longest string the key panel reads |
| `MAX_FIELD_BYTES` ([explorer](crate::explorer::MAX_FIELD_BYTES)) | 4 MiB | biggest hash field value read with its page |
| `HASH_NAMES_BATCH` ([explorer](crate::explorer::HASH_NAMES_BATCH)) | 10 000 | hash field names read before sorting |
| Redisql string/field limits | see [redisql](crate::wiki::redisql) | |

## Errors

Every method returns [`Error`](crate::error::Error); a `redis::RedisError` converts into it via
`From`, so `?` is enough, and the kind (`Redis`, `Denied`, `Network`, `Timeout`...) is derived from
the server's reply. See [errors](crate::wiki::errors). Two helpers on `Error` are specific to
this module: `is_unknown_option` and `is_script_refused`.

## Tests

`src/session/tests.rs` covers the pure decoding (`entries_of_reply` for every type). Commands
against a real server are covered by the `e2e` tests (`cargo xtask e2e`), not by unit tests.
