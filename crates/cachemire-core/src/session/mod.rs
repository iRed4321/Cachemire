//! The one way to talk to a Redis server: every command the app sends is built,
//! and its reply decoded, by a method here. Each command is bounded by the
//! connection's response timeout (see `connection::client`).

use std::sync::Arc;
use std::sync::atomic::Ordering;

use crate::connection::pool::{ConnectionPool, RedisConnection};
use crate::error::{Error, ErrorKind};
use crate::key_name::{KeyName, display_bytes, field_bytes, field_name};
use crate::redis_value_type::RedisValueType;
use crate::state::{AppState, resolve_connection};

/// A key with its type, as `TYPE` tells it.
pub struct TypedKey {
    pub name: KeyName,
    pub key_type: RedisValueType,
}

/// One key of the search script's answer: key, type, status, and the flat
/// field/value list of the entries that might match (see `search.lua`).
pub type ScriptKey = (Vec<u8>, Vec<u8>, i64, Vec<Vec<u8>>);

/// A stream entry: its id and its field/value pairs.
pub type StreamEntry = (String, Vec<(String, String)>);

/// How many entries one command reads of a big key: a bigger read takes several
/// commands, so none holds the server up for long or makes a huge reply.
const READ_CHUNK: usize = 1000;

/// How much of a string value a search reads: its first MB.
const SEARCH_STRING_BYTES: usize = 1024 * 1024;

static SEARCH_SCRIPT: std::sync::LazyLock<redis::Script> = std::sync::LazyLock::new(|| redis::Script::new(include_str!("search.lua")));

/// Moves a hash field to a free name with its new value in one step: 0 when the name is taken.
static RENAME_FIELD_SCRIPT: std::sync::LazyLock<redis::Script> = std::sync::LazyLock::new(|| {
    redis::Script::new(
        "if redis.call('HSETNX', KEYS[1], ARGV[2], ARGV[3]) == 0 then return 0 end \
         redis.call('HDEL', KEYS[1], ARGV[1]) return 1",
    )
});

/// Writes a hash field's new value if it still holds the expected one: `{1}` done, or
/// `{0, current}` when it changed (`current` is false when the field is gone).
static CHECKED_HASH_SET_SCRIPT: std::sync::LazyLock<redis::Script> = std::sync::LazyLock::new(|| {
    redis::Script::new(
        "local cur = redis.call('HGET', KEYS[1], ARGV[1]) \
         if cur == false or cur ~= ARGV[2] then return {0, cur} end \
         redis.call('HSET', KEYS[1], ARGV[1], ARGV[3]) return {1, false}",
    )
});

/// The same for a string key (its TTL kept).
static CHECKED_STRING_SET_SCRIPT: std::sync::LazyLock<redis::Script> = std::sync::LazyLock::new(|| {
    redis::Script::new(
        "local cur = redis.call('GET', KEYS[1]) \
         if cur == false or cur ~= ARGV[1] then return {0, cur} end \
         redis.call('SET', KEYS[1], ARGV[2], 'KEEPTTL') return {1, false}",
    )
});

/// Moves a hash field to a free name, with a new value, if it still holds the expected
/// one: `{1}` done, `{2}` the name is taken, or `{0, current}` when it changed.
static CHECKED_RENAME_FIELD_SCRIPT: std::sync::LazyLock<redis::Script> = std::sync::LazyLock::new(|| {
    redis::Script::new(
        "local cur = redis.call('HGET', KEYS[1], ARGV[1]) \
         if cur == false or cur ~= ARGV[4] then return {0, cur} end \
         if redis.call('HSETNX', KEYS[1], ARGV[2], ARGV[3]) == 0 then return {2, false} end \
         redis.call('HDEL', KEYS[1], ARGV[1]) return {1, false}",
    )
});

/// What a write that first checks the value it replaces came to.
#[derive(Debug, PartialEq, Eq)]
pub enum Checked {
    /// written
    Done,
    /// a renamed field's new name is a field already; nothing changed
    Taken,
    /// the value is no longer the expected one (`None`: it is gone); nothing changed
    Changed(Option<String>),
}

/// The outcome a checked script answered: its code, and the current value when it changed.
fn checked_of(reply: (i64, Option<Vec<u8>>)) -> Result<Checked, Error> {
    match reply {
        (1, _) => Ok(Checked::Done),
        (2, _) => Ok(Checked::Taken),
        (_, current) => changed(current),
    }
}

/// [`Checked::Changed`] with the value found: refused when it isn't UTF-8 text, which
/// the editor can't hold.
fn changed(current: Option<Vec<u8>>) -> Result<Checked, Error> {
    match current.map(String::from_utf8) {
        Some(Err(_)) => Err(Error::BinaryData),
        current => Ok(Checked::Changed(current.map(|text| text.unwrap_or_default()))),
    }
}

/// Whether the server runs in cluster mode (`cluster_enabled:1` in `INFO cluster`); a
/// server that won't say (INFO not allowed) is taken not to.
pub(super) async fn cluster_enabled(con: &mut RedisConnection) -> bool {
    let info: Result<String, _> = redis::cmd("INFO").arg("cluster").query_async(con).await;
    info.is_ok_and(|info| info.lines().any(|line| line.trim() == "cluster_enabled:1"))
}

pub(super) async fn ping(con: &mut RedisConnection) -> Result<(), Error> {
    redis::cmd("PING").query_async::<String>(con).await?;
    Ok(())
}

/// `bytes` as text for the screen: each byte that isn't UTF-8 is written `\xNN`.
pub(super) fn lossy(bytes: Vec<u8>) -> String {
    display_bytes(&bytes).0
}

/// [`lossy`], with whether the bytes weren't all UTF-8.
pub(super) fn lossy_flagged(bytes: Vec<u8>) -> (String, bool) {
    display_bytes(&bytes)
}

/// The smallest stream id after `id` (`ms-seq`), the start of the next page of a range.
pub(super) fn next_stream_id(id: &str) -> Option<String> {
    let (ms, seq) = id.split_once('-')?;
    match seq.parse::<u64>().ok()?.checked_add(1) {
        Some(seq) => Some(format!("{ms}-{seq}")),
        None => Some(format!("{}-0", ms.parse::<u64>().ok()?.checked_add(1)?)),
    }
}

/// Pairs up a flat reply (`HSCAN`'s fields, `ZRANGE ... WITHSCORES`).
pub(super) fn pairs(flat: Vec<Vec<u8>>) -> impl Iterator<Item = (String, String)> {
    let mut flat = flat.into_iter();
    std::iter::from_fn(move || Some((field_name(&flat.next()?), lossy(flat.next()?))))
}

/// A stream entry as the server sends it, before its bytes are made text.
type RawStreamEntry = (Vec<u8>, Vec<(Vec<u8>, Vec<u8>)>);

fn stream_entry(raw: RawStreamEntry) -> StreamEntry {
    (lossy(raw.0), raw.1.into_iter().map(|(field, value)| (lossy(field), lossy(value))).collect())
}

/// A stream entry's fields as a JSON object, in their order.
pub(super) fn stream_fields(fields: Vec<(String, String)>) -> serde_json::Value {
    serde_json::Value::Object(fields.into_iter().map(|(k, v)| (k, serde_json::Value::String(v))).collect())
}

/// The searchable entries in one key's reply to the read `push_head_read` queued;
/// a reply of the wrong shape (the key changed meanwhile) gives none.
fn entries_of_reply(key_type: &RedisValueType, reply: redis::Value) -> Vec<(String, String)> {
    match key_type {
        RedisValueType::String => {
            redis::from_redis_value::<Option<Vec<u8>>>(reply).ok().flatten().map(|v| vec![(String::new(), lossy(v))]).unwrap_or_default()
        }
        RedisValueType::Hash => {
            redis::from_redis_value::<(Vec<u8>, Vec<Vec<u8>>)>(reply).map(|(_, flat)| pairs(flat).collect()).unwrap_or_default()
        }
        RedisValueType::List => redis::from_redis_value::<Vec<Vec<u8>>>(reply)
            .map(|items| items.into_iter().enumerate().map(|(i, v)| (format!("[{i}]"), lossy(v))).collect())
            .unwrap_or_default(),
        RedisValueType::Set => redis::from_redis_value::<(Vec<u8>, Vec<Vec<u8>>)>(reply)
            .map(|(_, members)| members.into_iter().map(|m| (String::new(), lossy(m))).collect())
            .unwrap_or_default(),
        RedisValueType::Zset => redis::from_redis_value::<Vec<Vec<u8>>>(reply)
            .map(|flat| pairs(flat).map(|(member, score)| (score, member)).collect())
            .unwrap_or_default(),
        RedisValueType::Stream => redis::from_redis_value::<Vec<RawStreamEntry>>(reply)
            .map(|entries| entries.into_iter().map(stream_entry).map(|(id, fields)| (id, stream_fields(fields).to_string())).collect())
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

/// Queues the read of at most `limit` entries of `key`; false for a type with
/// nothing to read.
fn push_head_read(pipe: &mut redis::Pipeline, key: &TypedKey, limit: usize) -> bool {
    let name = &key.name;
    let last = limit.saturating_sub(1);
    match key.key_type {
        RedisValueType::String => pipe.cmd("GETRANGE").arg(name).arg(0).arg(SEARCH_STRING_BYTES - 1),
        RedisValueType::Hash => pipe.cmd("HSCAN").arg(name).arg(0).arg("COUNT").arg(limit),
        RedisValueType::List => pipe.cmd("LRANGE").arg(name).arg(0).arg(last),
        RedisValueType::Set => pipe.cmd("SSCAN").arg(name).arg(0).arg("COUNT").arg(limit),
        RedisValueType::Zset => pipe.cmd("ZRANGE").arg(name).arg(0).arg(last).arg("WITHSCORES"),
        RedisValueType::Stream => pipe.cmd("XRANGE").arg(name).arg("-").arg("+").arg("COUNT").arg(limit),
        _ => return false,
    };
    true
}

/// A walk of the keyspace with `SCAN`, matching `pattern` when there is one, `count`
/// names asked for per step. A page may repeat a name an earlier one gave.
pub struct KeyScan {
    pattern: Option<Vec<u8>>,
    count: usize,
    // where the next step starts; `None` once the walk is over
    cursor: Option<u64>,
}

impl KeyScan {
    pub fn new(pattern: Option<Vec<u8>>, count: usize) -> Self {
        Self { pattern, count, cursor: Some(0) }
    }

    /// The next page of names, `None` once the walk is over.
    pub async fn next(&mut self, session: &mut Session) -> Result<Option<Vec<KeyName>>, Error> {
        let Some(cursor) = self.cursor else { return Ok(None) };
        let (next, names) = session.scan(cursor, self.pattern.as_deref(), self.count).await?;
        self.cursor = (next != 0).then_some(next);
        Ok(Some(names))
    }

    /// Whether the last page was the walk's last.
    pub fn finished(&self) -> bool {
        self.cursor.is_none()
    }
}

/// A connection to one saved connection's server, taken from its pool.
pub struct Session {
    con: RedisConnection,
    pool: Arc<ConnectionPool>,
    // on the pool's connection for long reads (see `open_background`)
    background: bool,
}

impl Session {
    /// A session on `connection_id`, or on the active connection when `None`;
    /// the connection's pool is opened on first use.
    pub async fn open(state: &AppState, connection_id: Option<&str>) -> Result<Self, Error> {
        let connection = resolve_connection(state, connection_id)?;
        let pool = state.redis_connections.pool(&connection).await?;
        Ok(Self::on(pool))
    }

    /// `open`, on the connection kept for long reads, so they don't hold up the
    /// short ones a click makes; its siblings stay on it.
    pub async fn open_background(state: &AppState, connection_id: Option<&str>) -> Result<Self, Error> {
        let connection = resolve_connection(state, connection_id)?;
        let pool = state.redis_connections.pool(&connection).await?;
        Ok(Self { con: pool.background(), pool, background: true })
    }

    pub(super) fn on(pool: Arc<ConnectionPool>) -> Self {
        Self { con: pool.pick(), pool, background: false }
    }

    /// Another session on the same pool, for commands run beside this one's.
    pub fn sibling(&self) -> Self {
        if self.background {
            Self { con: self.pool.background(), pool: self.pool.clone(), background: true }
        } else {
            Self::on(self.pool.clone())
        }
    }

    pub async fn ping(&mut self) -> Result<(), Error> {
        ping(&mut self.con).await
    }

    /// The `section` part of `INFO`, as the server words it.
    pub async fn info(&mut self, section: &str) -> Result<String, Error> {
        Ok(redis::cmd("INFO").arg(section).query_async(&mut self.con).await?)
    }

    pub async fn dbsize(&mut self) -> Result<usize, Error> {
        Ok(redis::cmd("DBSIZE").query_async(&mut self.con).await?)
    }

    /// One `SCAN` step from `cursor`: the next cursor (0 once done) and the names found.
    async fn scan(&mut self, cursor: u64, pattern: Option<&[u8]>, count: usize) -> Result<(u64, Vec<KeyName>), Error> {
        let mut cmd = redis::cmd("SCAN");
        cmd.arg(cursor);
        if let Some(pattern) = pattern {
            cmd.arg("MATCH").arg(pattern);
        }
        Ok(cmd.arg("COUNT").arg(count).query_async(&mut self.con).await?)
    }

    pub async fn key_type(&mut self, key: &KeyName) -> Result<RedisValueType, Error> {
        let kind: String = redis::cmd("TYPE").arg(key).query_async(&mut self.con).await?;
        Ok(RedisValueType::from(kind))
    }

    /// The types of `names`, in one pipelined round trip; a key whose `TYPE` the server
    /// refuses (an ACL that doesn't cover it) is of an unknown type, `Other("")`.
    pub async fn types(&mut self, names: Vec<KeyName>) -> Result<Vec<TypedKey>, Error> {
        if names.is_empty() {
            return Ok(Vec::new());
        }
        let mut pipe = redis::pipe();
        pipe.ignore_errors();
        for name in &names {
            pipe.cmd("TYPE").arg(name);
        }
        let kinds: Vec<redis::RedisResult<String>> = pipe.query_async(&mut self.con).await?;
        Ok(names
            .into_iter()
            .zip(kinds)
            .map(|(name, kind)| TypedKey { name, key_type: kind.map_or(RedisValueType::Other(String::new()), RedisValueType::from) })
            .collect())
    }

    /// The key's type and TTL in seconds (-1 for none, -2 for a missing key).
    pub async fn type_and_ttl(&mut self, key: &KeyName) -> Result<(RedisValueType, i64), Error> {
        let (kind, ttl): (String, i64) = redis::pipe().cmd("TYPE").arg(key).cmd("TTL").arg(key).query_async(&mut self.con).await?;
        Ok((RedisValueType::from(kind), ttl))
    }

    /// At most `limit` searchable entries of each of `keys`, in one pipelined round
    /// trip; a key whose read fails (gone, changed type, refused) has none.
    pub async fn entry_heads(&mut self, keys: &[TypedKey], limit: usize) -> Result<Vec<Vec<(String, String)>>, Error> {
        let mut pipe = redis::pipe();
        pipe.ignore_errors();
        let read: Vec<bool> = keys.iter().map(|key| push_head_read(&mut pipe, key, limit)).collect();
        let mut replies = if read.contains(&true) {
            pipe.query_async::<Vec<redis::Value>>(&mut self.con).await?.into_iter()
        } else {
            Vec::new().into_iter()
        };
        Ok(keys
            .iter()
            .zip(read)
            .map(
                |(key, read)| {
                    if read { replies.next().map(|reply| entries_of_reply(&key.key_type, reply)).unwrap_or_default() } else { Vec::new() }
                },
            )
            .collect())
    }

    /// Runs the search script on `names`: Redis answers only the entries that may
    /// hold `needle` (lowercased), at most `depth` per key.
    pub async fn search_script(&mut self, names: &[KeyName], needle: &str, depth: usize) -> Result<Vec<ScriptKey>, Error> {
        Ok(SEARCH_SCRIPT.key(names).arg(needle).arg(depth).arg(SEARCH_STRING_BYTES).invoke_async(&mut self.con).await?)
    }

    pub async fn get(&mut self, key: &KeyName) -> Result<Option<String>, Error> {
        let value: Option<Vec<u8>> = redis::cmd("GET").arg(key).query_async(&mut self.con).await?;
        Ok(value.map(lossy))
    }

    /// The string's length in bytes (`STRLEN`), 0 for a missing key.
    pub async fn string_len(&mut self, key: &KeyName) -> Result<usize, Error> {
        Ok(redis::cmd("STRLEN").arg(key).query_async(&mut self.con).await?)
    }

    /// The string's length in bytes, and at most its first `max` bytes.
    pub async fn string_head(&mut self, key: &KeyName, max: usize) -> Result<(usize, Vec<u8>), Error> {
        let last = max.saturating_sub(1);
        Ok(redis::pipe().cmd("STRLEN").arg(key).cmd("GETRANGE").arg(key).arg(0).arg(last).query_async(&mut self.con).await?)
    }

    pub async fn hash_len(&mut self, key: &KeyName) -> Result<usize, Error> {
        Ok(redis::cmd("HLEN").arg(key).query_async(&mut self.con).await?)
    }

    pub async fn hash_exists(&mut self, key: &KeyName, field: &str) -> Result<bool, Error> {
        Ok(redis::cmd("HEXISTS").arg(key).arg(&*field_bytes(field)).query_async(&mut self.con).await?)
    }

    /// One `HSCAN` step from `cursor` for the field names only: with `NOVALUES` where
    /// the server knows it (Redis 7.4+), else reading the values too and dropping them.
    pub async fn hash_scan_names(&mut self, key: &KeyName, cursor: u64, count: usize) -> Result<(u64, Vec<String>), Error> {
        if self.pool.hscan_novalues.load(Ordering::Relaxed) {
            let reply: Result<(u64, Vec<Vec<u8>>), _> =
                redis::cmd("HSCAN").arg(key).arg(cursor).arg("COUNT").arg(count).arg("NOVALUES").query_async(&mut self.con).await;
            match reply.map_err(Error::from) {
                Ok((next, names)) => return Ok((next, names.iter().map(|name| field_name(name)).collect())),
                Err(e) if e.is_unknown_option() => self.pool.hscan_novalues.store(false, Ordering::Relaxed),
                Err(e) => return Err(e),
            }
        }
        let (next, pairs) = self.hash_scan(key, cursor, count).await?;
        Ok((next, pairs.into_iter().map(|(name, _)| name).collect()))
    }

    /// The values of `fields`, in that order (`None` for a field that is gone), an `HMGET`
    /// per [`READ_CHUNK`] fields. Each value comes with whether it isn't UTF-8 text; a field
    /// name is read as the exact bytes it stands for.
    pub async fn hash_values(&mut self, key: &KeyName, fields: &[String]) -> Result<Vec<Option<(String, bool)>>, Error> {
        let mut values = Vec::with_capacity(fields.len());
        for chunk in fields.chunks(READ_CHUNK) {
            let mut cmd = redis::cmd("HMGET");
            cmd.arg(key);
            for field in chunk {
                cmd.arg(&*field_bytes(field));
            }
            let read: Vec<Option<Vec<u8>>> = cmd.query_async(&mut self.con).await?;
            values.extend(read.into_iter().map(|value| value.map(lossy_flagged)));
        }
        Ok(values)
    }

    /// Writes `value` into `field` of the hash (`HSET`).
    pub async fn hash_set(&mut self, key: &KeyName, field: &str, value: &str) -> Result<(), Error> {
        redis::cmd("HSET").arg(key).arg(&*field_bytes(field)).arg(value).query_async::<()>(&mut self.con).await?;
        Ok(())
    }

    /// Writes `value` into `field` of the hash unless it has that field already
    /// (`HSETNX`); whether it was written.
    pub async fn hash_set_new(&mut self, key: &KeyName, field: &str, value: &str) -> Result<bool, Error> {
        Ok(redis::cmd("HSETNX").arg(key).arg(&*field_bytes(field)).arg(value).query_async(&mut self.con).await?)
    }

    /// Moves field `from` of the hash to `to`, with `value` as its value, unless the hash
    /// has a field `to` already; whether it moved. One script, so it all happens or none
    /// of it does; a server that refuses scripts gets `HSETNX` then `HDEL`.
    pub async fn hash_rename(&mut self, key: &KeyName, from: &str, to: &str, value: &str) -> Result<bool, Error> {
        let moved: Result<i64, _> =
            RENAME_FIELD_SCRIPT.key(key).arg(&*field_bytes(from)).arg(&*field_bytes(to)).arg(value).invoke_async(&mut self.con).await;
        match moved.map_err(Error::from) {
            Ok(moved) => Ok(moved == 1),
            Err(e) if e.is_script_refused() => {
                tracing::info!("the field rename script was refused ({e}); renaming in two steps");
                if !self.hash_set_new(key, to, value).await? {
                    return Ok(false);
                }
                self.hash_delete(key, from).await?;
                Ok(true)
            }
            Err(e) => Err(e),
        }
    }

    /// [`hash_set`](Self::hash_set) if the field still holds `expected`, in one script; a
    /// server that refuses scripts gets the check and the write as two commands.
    pub async fn hash_set_checked(&mut self, key: &KeyName, field: &str, expected: &str, value: &str) -> Result<Checked, Error> {
        let reply: Result<(i64, Option<Vec<u8>>), _> =
            CHECKED_HASH_SET_SCRIPT.key(key).arg(&*field_bytes(field)).arg(expected).arg(value).invoke_async(&mut self.con).await;
        match reply.map_err(Error::from) {
            Ok(reply) => checked_of(reply),
            Err(e) if e.is_script_refused() => {
                tracing::info!("the checked write script was refused ({e}); checking then writing");
                let current: Option<Vec<u8>> = redis::cmd("HGET").arg(key).arg(&*field_bytes(field)).query_async(&mut self.con).await?;
                if current.as_deref() != Some(expected.as_bytes()) {
                    return changed(current);
                }
                self.hash_set(key, field, value).await?;
                Ok(Checked::Done)
            }
            Err(e) => Err(e),
        }
    }

    /// [`hash_rename`](Self::hash_rename) if field `from` still holds `expected`.
    pub async fn hash_rename_checked(
        &mut self,
        key: &KeyName,
        from: &str,
        to: &str,
        expected: &str,
        value: &str,
    ) -> Result<Checked, Error> {
        let reply: Result<(i64, Option<Vec<u8>>), _> = CHECKED_RENAME_FIELD_SCRIPT
            .key(key)
            .arg(&*field_bytes(from))
            .arg(&*field_bytes(to))
            .arg(value)
            .arg(expected)
            .invoke_async(&mut self.con)
            .await;
        match reply.map_err(Error::from) {
            Ok(reply) => checked_of(reply),
            Err(e) if e.is_script_refused() => {
                tracing::info!("the checked rename script was refused ({e}); checking then renaming");
                let current: Option<Vec<u8>> = redis::cmd("HGET").arg(key).arg(&*field_bytes(from)).query_async(&mut self.con).await?;
                if current.as_deref() != Some(expected.as_bytes()) {
                    return changed(current);
                }
                if !self.hash_set_new(key, to, value).await? {
                    return Ok(Checked::Taken);
                }
                self.hash_delete(key, from).await?;
                Ok(Checked::Done)
            }
            Err(e) => Err(e),
        }
    }

    /// [`string_set`](Self::string_set) if the string still holds `expected`, in one script
    /// (Redis 6.0+); otherwise the check and the write are two commands.
    pub async fn string_set_checked(&mut self, key: &KeyName, expected: &str, value: &str) -> Result<Checked, Error> {
        let reply: Result<(i64, Option<Vec<u8>>), _> =
            CHECKED_STRING_SET_SCRIPT.key(key).arg(expected).arg(value).invoke_async(&mut self.con).await;
        match reply.map_err(Error::from) {
            Ok(reply) => checked_of(reply),
            Err(e) if e.is_script_refused() => {
                tracing::info!("the checked write script was refused ({e}); checking then writing");
                let current: Option<Vec<u8>> = redis::cmd("GET").arg(key).query_async(&mut self.con).await?;
                if current.as_deref() != Some(expected.as_bytes()) {
                    return changed(current);
                }
                self.string_set(key, value).await?;
                Ok(Checked::Done)
            }
            Err(e) => Err(e),
        }
    }

    /// Removes `field` from the hash (`HDEL`).
    pub async fn hash_delete(&mut self, key: &KeyName, field: &str) -> Result<(), Error> {
        redis::cmd("HDEL").arg(key).arg(&*field_bytes(field)).query_async::<()>(&mut self.con).await?;
        Ok(())
    }

    /// Which of `keys` exist, in their order (`EXISTS` each, in one round trip).
    pub async fn existing(&mut self, keys: &[KeyName]) -> Result<Vec<bool>, Error> {
        if keys.is_empty() {
            return Ok(Vec::new());
        }
        let mut pipe = redis::pipe();
        for key in keys {
            pipe.cmd("EXISTS").arg(key);
        }
        Ok(pipe.query_async(&mut self.con).await?)
    }

    /// Removes `keys`, their memory freed in the background (`UNLINK`).
    pub async fn unlink(&mut self, keys: &[KeyName]) -> Result<(), Error> {
        if !keys.is_empty() {
            redis::cmd("UNLINK").arg(keys).query_async::<()>(&mut self.con).await?;
        }
        Ok(())
    }

    /// `RENAMENX` of each pair, in one round trip: whether each was renamed, or why not.
    pub async fn rename_new_many(&mut self, moves: &[(KeyName, KeyName)]) -> Result<Vec<Result<bool, Error>>, Error> {
        if moves.is_empty() {
            return Ok(Vec::new());
        }
        let mut pipe = redis::pipe();
        pipe.ignore_errors();
        for (from, to) in moves {
            pipe.cmd("RENAMENX").arg(from).arg(to);
        }
        let renamed: Vec<redis::RedisResult<bool>> = pipe.query_async(&mut self.con).await?;
        Ok(renamed.into_iter().map(|r| r.map_err(Error::from)).collect())
    }

    /// Renames `from` to `to` unless a key `to` exists (`RENAMENX`); whether it was renamed.
    pub async fn rename_new(&mut self, from: &KeyName, to: &KeyName) -> Result<bool, Error> {
        Ok(redis::cmd("RENAMENX").arg(from).arg(to).query_async(&mut self.con).await?)
    }

    /// Replaces the string's value, its TTL kept: `SET … KEEPTTL`, or before Redis 6,
    /// `SET … PX` with the TTL it had.
    pub async fn string_set(&mut self, key: &KeyName, value: &str) -> Result<(), Error> {
        let kept: Result<(), _> = redis::cmd("SET").arg(key).arg(value).arg("KEEPTTL").query_async(&mut self.con).await;
        match kept.map_err(Error::from) {
            Err(e) if e.kind() == ErrorKind::Redis => {
                let ttl: i64 = redis::cmd("PTTL").arg(key).query_async(&mut self.con).await?;
                let mut set = redis::cmd("SET");
                set.arg(key).arg(value);
                if ttl > 0 {
                    set.arg("PX").arg(ttl);
                }
                set.query_async::<()>(&mut self.con).await?;
                Ok(())
            }
            kept => kept,
        }
    }

    /// The size in bytes of each of `fields`' values, in that order (0 for a field that
    /// is gone), from one `HSTRLEN` per field, pipelined [`READ_CHUNK`] at a time.
    pub async fn hash_value_sizes(&mut self, key: &KeyName, fields: &[String]) -> Result<Vec<usize>, Error> {
        let mut sizes = Vec::with_capacity(fields.len());
        for chunk in fields.chunks(READ_CHUNK) {
            let mut pipe = redis::pipe();
            for field in chunk {
                pipe.cmd("HSTRLEN").arg(key).arg(&*field_bytes(field));
            }
            sizes.extend(pipe.query_async::<Vec<usize>>(&mut self.con).await?);
        }
        Ok(sizes)
    }

    /// One `HSCAN` step from `cursor`: the next cursor and the field/value pairs found.
    pub async fn hash_scan(&mut self, key: &KeyName, cursor: u64, count: usize) -> Result<(u64, Vec<(String, String)>), Error> {
        let (next, flat): (u64, Vec<Vec<u8>>) =
            redis::cmd("HSCAN").arg(key).arg(cursor).arg("COUNT").arg(count).query_async(&mut self.con).await?;
        Ok((next, pairs(flat).collect()))
    }

    pub async fn set_len(&mut self, key: &KeyName) -> Result<usize, Error> {
        Ok(redis::cmd("SCARD").arg(key).query_async(&mut self.con).await?)
    }

    /// One `SSCAN` step from `cursor`: the next cursor and the members found.
    pub async fn set_scan(&mut self, key: &KeyName, cursor: u64, count: usize) -> Result<(u64, Vec<String>), Error> {
        let (next, members): (u64, Vec<Vec<u8>>) =
            redis::cmd("SSCAN").arg(key).arg(cursor).arg("COUNT").arg(count).query_async(&mut self.con).await?;
        Ok((next, members.into_iter().map(lossy).collect()))
    }

    /// The list's length and its first `count` items, an `LRANGE` per [`READ_CHUNK`] items.
    pub async fn list_head(&mut self, key: &KeyName, count: usize) -> Result<(usize, Vec<String>), Error> {
        let first = count.min(READ_CHUNK).saturating_sub(1);
        let (len, mut items): (usize, Vec<Vec<u8>>) =
            redis::pipe().cmd("LLEN").arg(key).cmd("LRANGE").arg(key).arg(0).arg(first).query_async(&mut self.con).await?;
        let wanted = count.min(len);
        while items.len() < wanted {
            let last = (items.len() + READ_CHUNK).min(wanted) - 1;
            let more: Vec<Vec<u8>> = redis::cmd("LRANGE").arg(key).arg(items.len()).arg(last).query_async(&mut self.con).await?;
            if more.is_empty() {
                break;
            }
            items.extend(more);
        }
        Ok((len, items.into_iter().map(lossy).collect()))
    }

    /// The sorted set's size and its first `count` members with their scores, a `ZRANGE`
    /// per [`READ_CHUNK`] members.
    pub async fn zset_head(&mut self, key: &KeyName, count: usize) -> Result<(usize, Vec<(String, f64)>), Error> {
        let range = |start: usize, last: usize| {
            let mut cmd = redis::cmd("ZRANGE");
            cmd.arg(key).arg(start).arg(last).arg("WITHSCORES");
            cmd
        };
        let first = count.min(READ_CHUNK).saturating_sub(1);
        let (len, mut members): (usize, Vec<(Vec<u8>, f64)>) =
            redis::pipe().cmd("ZCARD").arg(key).add_command(range(0, first)).query_async(&mut self.con).await?;
        let wanted = count.min(len);
        while members.len() < wanted {
            let last = (members.len() + READ_CHUNK).min(wanted) - 1;
            let more: Vec<(Vec<u8>, f64)> = range(members.len(), last).query_async(&mut self.con).await?;
            if more.is_empty() {
                break;
            }
            members.extend(more);
        }
        Ok((len, members.into_iter().map(|(member, score)| (lossy(member), score)).collect()))
    }

    /// The stream's length (`XLEN`).
    pub async fn stream_len(&mut self, key: &KeyName) -> Result<usize, Error> {
        Ok(redis::cmd("XLEN").arg(key).query_async(&mut self.con).await?)
    }

    /// The stream's first `count` entries (all of them for `None`), an `XRANGE … COUNT`
    /// per [`READ_CHUNK`] entries, each starting right after the last id read.
    pub async fn stream_head(&mut self, key: &KeyName, count: Option<usize>) -> Result<Vec<StreamEntry>, Error> {
        let wanted = count.unwrap_or(usize::MAX);
        let mut entries: Vec<StreamEntry> = Vec::new();
        let mut start = "-".to_string();
        while entries.len() < wanted {
            let chunk = (wanted - entries.len()).min(READ_CHUNK);
            let page: Vec<RawStreamEntry> =
                redis::cmd("XRANGE").arg(key).arg(&start).arg("+").arg("COUNT").arg(chunk).query_async(&mut self.con).await?;
            let page: Vec<StreamEntry> = page.into_iter().map(stream_entry).collect();
            let next = page.last().and_then(|(id, _)| next_stream_id(id));
            let full = page.len() == chunk;
            entries.extend(page);
            match next {
                Some(next) if full => start = next,
                _ => break,
            }
        }
        Ok(entries)
    }
}

#[cfg(test)]
mod tests;
