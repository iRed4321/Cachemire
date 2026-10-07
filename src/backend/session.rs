//! The one way to talk to a Redis server: every command the app sends is built,
//! and its reply decoded, by a method here. Each command is bounded by the
//! connection's response timeout (see `redis_client`).

use std::sync::Arc;
use std::sync::atomic::Ordering;

use super::connection_cache::{ConnectionPool, RedisConnection};
use super::error::AppError;
use super::key_name::KeyName;
use super::redis_value_type::RedisValueType;
use super::state::{resolve_connection, AppState};

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

static SEARCH_SCRIPT: std::sync::LazyLock<redis::Script> = std::sync::LazyLock::new(|| redis::Script::new(include_str!("search.lua")));

/// Whether the server runs in cluster mode (`cluster_enabled:1` in `INFO cluster`); a
/// server that won't say (INFO not allowed) is taken not to.
pub(super) async fn cluster_enabled(con: &mut RedisConnection) -> bool {
    let info: Result<String, _> = redis::cmd("INFO").arg("cluster").query_async(con).await;
    info.is_ok_and(|info| info.lines().any(|line| line.trim() == "cluster_enabled:1"))
}

pub(super) async fn ping(con: &mut RedisConnection) -> Result<(), AppError> {
    redis::cmd("PING").query_async::<String>(con).await?;
    Ok(())
}

pub(super) fn lossy(bytes: Vec<u8>) -> String {
    String::from_utf8_lossy(&bytes).into_owned()
}

/// Pairs up a flat reply (`HSCAN`'s fields, `ZRANGE ... WITHSCORES`).
pub(super) fn pairs(flat: Vec<Vec<u8>>) -> impl Iterator<Item = (String, String)> {
    let mut flat = flat.into_iter();
    std::iter::from_fn(move || Some((lossy(flat.next()?), lossy(flat.next()?))))
}

/// The searchable entries in one key's reply to the read `push_head_read` queued;
/// a reply of the wrong shape (the key changed meanwhile) gives none.
fn entries_of_reply(key_type: &RedisValueType, reply: redis::Value) -> Vec<(String, String)> {
    match key_type {
        RedisValueType::String => redis::from_redis_value::<Option<Vec<u8>>>(reply)
            .ok()
            .flatten()
            .map(|v| vec![(String::new(), lossy(v))])
            .unwrap_or_default(),
        RedisValueType::Hash => redis::from_redis_value::<(Vec<u8>, Vec<Vec<u8>>)>(reply).map(|(_, flat)| pairs(flat).collect()).unwrap_or_default(),
        RedisValueType::List => redis::from_redis_value::<Vec<Vec<u8>>>(reply)
            .map(|items| items.into_iter().enumerate().map(|(i, v)| (format!("[{i}]"), lossy(v))).collect())
            .unwrap_or_default(),
        RedisValueType::Set => redis::from_redis_value::<(Vec<u8>, Vec<Vec<u8>>)>(reply)
            .map(|(_, members)| members.into_iter().map(|m| (String::new(), lossy(m))).collect())
            .unwrap_or_default(),
        RedisValueType::Zset => redis::from_redis_value::<Vec<Vec<u8>>>(reply)
            .map(|flat| pairs(flat).map(|(member, score)| (score, member)).collect())
            .unwrap_or_default(),
        RedisValueType::Stream => redis::from_redis_value::<Vec<StreamEntry>>(reply)
            .map(|entries| {
                entries
                    .into_iter()
                    .map(|(id, fields)| {
                        let values: serde_json::Map<String, serde_json::Value> = fields.into_iter().map(|(k, v)| (k, serde_json::Value::String(v))).collect();
                        (id, serde_json::Value::Object(values).to_string())
                    })
                    .collect()
            })
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

/// Queues the read of at most `limit` entries of `key`; false for a type with
/// nothing to read.
fn push_head_read(pipe: &mut redis::Pipeline, key: &TypedKey, limit: i64) -> bool {
    let name = &key.name;
    match key.key_type {
        RedisValueType::String => pipe.cmd("GET").arg(name),
        RedisValueType::Hash => pipe.cmd("HSCAN").arg(name).arg(0).arg("COUNT").arg(limit),
        RedisValueType::List => pipe.cmd("LRANGE").arg(name).arg(0).arg(limit - 1),
        RedisValueType::Set => pipe.cmd("SSCAN").arg(name).arg(0).arg("COUNT").arg(limit),
        RedisValueType::Zset => pipe.cmd("ZRANGE").arg(name).arg(0).arg(limit - 1).arg("WITHSCORES"),
        RedisValueType::Stream => pipe.cmd("XRANGE").arg(name).arg("-").arg("+").arg("COUNT").arg(limit),
        _ => return false,
    };
    true
}

/// A connection to one saved connection's server, taken from its pool.
pub struct Session {
    con: RedisConnection,
    pool: Arc<ConnectionPool>,
}

impl Session {
    /// A session on `connection_id`, or on the active connection when `None`;
    /// the connection's pool is opened on first use.
    pub async fn open(state: &AppState, connection_id: Option<&str>) -> Result<Self, AppError> {
        let connection = resolve_connection(state, connection_id)?;
        let pool = state.redis_connections.pool(&connection).await?;
        Ok(Self::on(pool))
    }

    pub(super) fn on(pool: Arc<ConnectionPool>) -> Self {
        Self { con: pool.pick(), pool }
    }

    /// Another session on the same pool, for commands run beside this one's.
    pub fn sibling(&self) -> Self {
        Self::on(self.pool.clone())
    }

    pub async fn ping(&mut self) -> Result<(), AppError> {
        ping(&mut self.con).await
    }

    /// The `section` part of `INFO`, as the server words it.
    pub async fn info(&mut self, section: &str) -> Result<String, AppError> {
        Ok(redis::cmd("INFO").arg(section).query_async(&mut self.con).await?)
    }

    pub async fn dbsize(&mut self) -> Result<i64, AppError> {
        Ok(redis::cmd("DBSIZE").query_async(&mut self.con).await?)
    }

    /// One `SCAN` step from `cursor`: the next cursor (0 once done) and the names found.
    pub async fn scan(&mut self, cursor: u64, pattern: Option<&[u8]>, count: usize) -> Result<(u64, Vec<KeyName>), AppError> {
        let mut cmd = redis::cmd("SCAN");
        cmd.arg(cursor);
        if let Some(pattern) = pattern {
            cmd.arg("MATCH").arg(pattern);
        }
        Ok(cmd.arg("COUNT").arg(count).query_async(&mut self.con).await?)
    }

    pub async fn key_type(&mut self, key: &KeyName) -> Result<RedisValueType, AppError> {
        let kind: String = redis::cmd("TYPE").arg(key).query_async(&mut self.con).await?;
        Ok(RedisValueType::from(kind))
    }

    /// The types of `names`, in one pipelined round trip.
    pub async fn types(&mut self, names: Vec<KeyName>) -> Result<Vec<TypedKey>, AppError> {
        if names.is_empty() {
            return Ok(Vec::new());
        }
        let mut pipe = redis::pipe();
        for name in &names {
            pipe.cmd("TYPE").arg(name);
        }
        let kinds: Vec<String> = pipe.query_async(&mut self.con).await?;
        Ok(names.into_iter().zip(kinds).map(|(name, kind)| TypedKey { name, key_type: RedisValueType::from(kind) }).collect())
    }

    /// The key's type and TTL in seconds (-1 for none, -2 for a missing key).
    pub async fn type_and_ttl(&mut self, key: &KeyName) -> Result<(RedisValueType, i64), AppError> {
        let (kind, ttl): (String, i64) = redis::pipe().cmd("TYPE").arg(key).cmd("TTL").arg(key).query_async(&mut self.con).await?;
        Ok((RedisValueType::from(kind), ttl))
    }

    /// At most `limit` searchable entries of each of `keys`, in one pipelined
    /// round trip; an error when the pipeline fails as a whole.
    pub async fn entry_heads(&mut self, keys: &[TypedKey], limit: i64) -> Result<Vec<Vec<(String, String)>>, AppError> {
        let mut pipe = redis::pipe();
        let read: Vec<bool> = keys.iter().map(|key| push_head_read(&mut pipe, key, limit)).collect();
        let mut replies = if read.contains(&true) { pipe.query_async::<Vec<redis::Value>>(&mut self.con).await?.into_iter() } else { Vec::new().into_iter() };
        Ok(keys
            .iter()
            .zip(read)
            .map(|(key, read)| if read { replies.next().map(|reply| entries_of_reply(&key.key_type, reply)).unwrap_or_default() } else { Vec::new() })
            .collect())
    }

    /// Runs the search script on `names`: Redis answers only the entries that may
    /// hold `needle` (lowercased), at most `depth` per key.
    pub async fn search_script(&mut self, names: &[KeyName], needle: &str, depth: i64) -> Result<Vec<ScriptKey>, AppError> {
        Ok(SEARCH_SCRIPT.key(names).arg(needle).arg(depth).invoke_async(&mut self.con).await?)
    }

    pub async fn get(&mut self, key: &KeyName) -> Result<Option<String>, AppError> {
        Ok(redis::cmd("GET").arg(key).query_async(&mut self.con).await?)
    }

    /// The string's length in bytes, and at most its first `max` bytes.
    pub async fn string_head(&mut self, key: &KeyName, max: usize) -> Result<(usize, Vec<u8>), AppError> {
        let last = max.saturating_sub(1);
        Ok(redis::pipe().cmd("STRLEN").arg(key).cmd("GETRANGE").arg(key).arg(0).arg(last).query_async(&mut self.con).await?)
    }

    pub async fn hash_len(&mut self, key: &KeyName) -> Result<i64, AppError> {
        Ok(redis::cmd("HLEN").arg(key).query_async(&mut self.con).await?)
    }

    pub async fn hash_exists(&mut self, key: &KeyName, field: &str) -> Result<bool, AppError> {
        Ok(redis::cmd("HEXISTS").arg(key).arg(field).query_async(&mut self.con).await?)
    }

    /// One `HSCAN` step from `cursor` for the field names only: with `NOVALUES` where
    /// the server knows it (Redis 7.4+), else reading the values too and dropping them.
    pub async fn hash_scan_names(&mut self, key: &KeyName, cursor: u64, count: usize) -> Result<(u64, Vec<String>), AppError> {
        if self.pool.hscan_novalues.load(Ordering::Relaxed) {
            let reply: Result<(u64, Vec<Vec<u8>>), _> = redis::cmd("HSCAN").arg(key).arg(cursor).arg("COUNT").arg(count).arg("NOVALUES").query_async(&mut self.con).await;
            match reply.map_err(AppError::from) {
                Ok((next, names)) => return Ok((next, names.into_iter().map(lossy).collect())),
                Err(AppError::Redis(_)) => self.pool.hscan_novalues.store(false, Ordering::Relaxed),
                Err(e) => return Err(e),
            }
        }
        let (next, pairs) = self.hash_scan(key, cursor, count).await?;
        Ok((next, pairs.into_iter().map(|(name, _)| name).collect()))
    }

    /// The values of `fields`, in that order (`None` for a field that is gone).
    pub async fn hash_values(&mut self, key: &KeyName, fields: &[String]) -> Result<Vec<Option<String>>, AppError> {
        if fields.is_empty() {
            return Ok(Vec::new());
        }
        let values: Vec<Option<Vec<u8>>> = redis::cmd("HMGET").arg(key).arg(fields).query_async(&mut self.con).await?;
        Ok(values.into_iter().map(|value| value.map(lossy)).collect())
    }

    /// One `HSCAN` step from `cursor`: the next cursor and the field/value pairs found.
    pub async fn hash_scan(&mut self, key: &KeyName, cursor: u64, count: usize) -> Result<(u64, Vec<(String, String)>), AppError> {
        let (next, flat): (u64, Vec<Vec<u8>>) = redis::cmd("HSCAN").arg(key).arg(cursor).arg("COUNT").arg(count).query_async(&mut self.con).await?;
        Ok((next, pairs(flat).collect()))
    }

    pub async fn set_len(&mut self, key: &KeyName) -> Result<i64, AppError> {
        Ok(redis::cmd("SCARD").arg(key).query_async(&mut self.con).await?)
    }

    /// One `SSCAN` step from `cursor`: the next cursor and the members found.
    pub async fn set_scan(&mut self, key: &KeyName, cursor: u64, count: i64) -> Result<(u64, Vec<String>), AppError> {
        let (next, members): (u64, Vec<Vec<u8>>) = redis::cmd("SSCAN").arg(key).arg(cursor).arg("COUNT").arg(count).query_async(&mut self.con).await?;
        Ok((next, members.into_iter().map(lossy).collect()))
    }

    /// The list's length and its first `count` items.
    pub async fn list_head(&mut self, key: &KeyName, count: i64) -> Result<(i64, Vec<String>), AppError> {
        let last = (count - 1).max(0);
        let (len, items): (i64, Vec<Vec<u8>>) = redis::pipe().cmd("LLEN").arg(key).cmd("LRANGE").arg(key).arg(0).arg(last).query_async(&mut self.con).await?;
        Ok((len, items.into_iter().map(lossy).collect()))
    }

    /// The sorted set's size and its first `count` members with their scores.
    pub async fn zset_head(&mut self, key: &KeyName, count: i64) -> Result<(i64, Vec<(String, f64)>), AppError> {
        let last = (count - 1).max(0);
        let (len, members): (i64, Vec<(Vec<u8>, f64)>) =
            redis::pipe().cmd("ZCARD").arg(key).cmd("ZRANGE").arg(key).arg(0).arg(last).arg("WITHSCORES").query_async(&mut self.con).await?;
        Ok((len, members.into_iter().map(|(member, score)| (lossy(member), score)).collect()))
    }

    /// The stream's first `count` entries (all of them for `None`), with an explicit
    /// `COUNT`: `limit` as the range's end id would miss real timestamp ids.
    pub async fn stream_head(&mut self, key: &KeyName, count: Option<i64>) -> Result<Vec<StreamEntry>, AppError> {
        let mut cmd = redis::cmd("XRANGE");
        cmd.arg(key).arg("-").arg("+");
        if let Some(count) = count {
            cmd.arg("COUNT").arg(count.max(0));
        }
        Ok(cmd.query_async(&mut self.con).await?)
    }
}
