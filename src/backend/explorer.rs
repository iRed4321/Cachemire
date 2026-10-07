use rust_i18n::t;
use rustc_hash::FxHashSet;
use std::collections::VecDeque;

use super::error::AppError;
use super::key_name::KeyName;
use serde_json::Value;

use super::redis_value_type::RedisValueType;
use super::session::{lossy, pairs, ScriptKey, Session, TypedKey};
use super::state::AppState;

/// One increment of a level listing from [`scan_namespaces`]. `folders`
/// holds only prefixes first seen this batch; `keys` may repeat (per SCAN's
/// guarantees) — consumers dedupe by name.
pub struct NamespaceBatch {
    pub folders: Vec<KeyName>,
    pub keys: Vec<TypedKey>,
}

/// `bytes` as a MATCH pattern matching exactly itself: the glob's special
/// characters are backslash-escaped.
fn glob_escape(bytes: &[u8]) -> Vec<u8> {
    let mut escaped = Vec::with_capacity(bytes.len());
    for &b in bytes {
        if matches!(b, b'*' | b'?' | b'[' | b']' | b'\\') {
            escaped.push(b'\\');
        }
        escaped.push(b);
    }
    escaped
}

/// Lists one "level" of the keyspace under `prefix` into `delimiter`-folders,
/// streaming through `on_batch` as SCAN advances (uncapped — a partial
/// listing can't be topped up later). `keep_going` stops it quietly if reset.
pub async fn scan_namespaces(
    state: &AppState,
    connection_id: Option<&str>,
    prefix: &KeyName,
    delimiter: &str,
    keep_going: impl Fn() -> bool,
    mut on_batch: impl FnMut(NamespaceBatch),
) -> Result<(), AppError> {
    let mut session = Session::open(state, connection_id).await?;

    // SCAN cursors the whole keyspace regardless of MATCH, so a prefix's
    // cost is round trips, not results — a bigger COUNT there skips more
    // server-side per trip; with no prefix a smaller COUNT bounds each reply.
    let prefix = prefix.as_bytes();
    let delimiter = delimiter.as_bytes();
    let (match_pattern, count) = if prefix.is_empty() {
        (b"*".to_vec(), 1000)
    } else {
        ([glob_escape(prefix), b"*".to_vec()].concat(), 10_000)
    };

    let mut seen_folders: FxHashSet<Vec<u8>> = FxHashSet::default();
    let mut cursor: u64 = 0;

    loop {
        if !keep_going() {
            return Ok(());
        }
        let (next_cursor, batch) = session.scan(cursor, Some(&match_pattern), count).await?;

        let mut folders: Vec<KeyName> = Vec::new();
        let mut leaf_keys: Vec<KeyName> = Vec::new();
        for key in batch {
            let bytes = key.as_bytes();
            if !bytes.starts_with(prefix) {
                continue;
            }

            let remainder = &bytes[prefix.len()..];
            let delimiter_at = if delimiter.is_empty() { None } else { remainder.windows(delimiter.len()).position(|w| w == delimiter) };
            match delimiter_at {
                Some(at) => {
                    // the folder's full name is a prefix of the key itself, so
                    // nothing is allocated for keys under an already-seen folder
                    let end = prefix.len() + at + delimiter.len();
                    if !seen_folders.contains(&bytes[..end]) {
                        seen_folders.insert(bytes[..end].to_vec());
                        folders.push(key.prefix(end));
                    }
                }
                None => leaf_keys.push(key),
            }
        }

        let keys = session.types(leaf_keys).await?;

        cursor = next_cursor;
        if !folders.is_empty() || !keys.is_empty() {
            on_batch(NamespaceBatch { folders, keys });
        }
        if cursor == 0 {
            return Ok(());
        }
    }
}

/// One page of [`walk_keyspace`]: the keys of one SCAN page, typed (SCAN may
/// repeat a key already delivered — consumers dedupe by name).
pub struct WalkBatch {
    pub keys: Vec<TypedKey>,
    /// keys walked so far, and how many the database held when the walk began
    /// (`DBSIZE`, 0 if that couldn't be told)
    pub scanned: i64,
    pub total: i64,
    /// the walk is over: the cursor came back to 0, or `max_keys` were walked
    pub done: bool,
    pub capped: bool,
}

/// Walks every key of the keyspace once with SCAN, a typed page at a time through
/// `on_batch`, stopping after `max_keys`. `keep_going` stops it quietly once false
/// (a reload, another connection).
pub async fn walk_keyspace(
    state: &AppState,
    connection_id: Option<&str>,
    max_keys: i64,
    keep_going: impl Fn() -> bool,
    mut on_batch: impl FnMut(WalkBatch),
) -> Result<(), AppError> {
    let mut session = Session::open(state, connection_id).await?;
    let total = session.dbsize().await.unwrap_or(0);
    let (mut cursor, mut scanned) = (0u64, 0i64);
    loop {
        if !keep_going() {
            return Ok(());
        }
        let (next, names) = session.scan(cursor, None, 1000).await?;
        scanned += names.len() as i64;
        let keys = session.types(names).await?;
        cursor = next;
        let capped = cursor != 0 && scanned >= max_keys;
        let done = cursor == 0 || capped;
        on_batch(WalkBatch { keys, scanned, total, done, capped });
        if done {
            return Ok(());
        }
    }
}

// ---------------------------------------------------------------------------
// Searching the values of the whole instance
// ---------------------------------------------------------------------------

/// A key's searchable content as (field, value) pairs: a string is one pair with
/// no field, a hash its fields, a list its items by index, a set its members, a
/// sorted set its members by score, a stream its entries by id.
pub struct KeyEntries {
    pub key: KeyName,
    pub key_type: RedisValueType,
    pub entries: Vec<(String, String)>,
}

/// One page of [`scan_entries`].
pub struct EntriesBatch {
    pub keys: Vec<KeyEntries>,
    /// Cumulative number of keys walked up to and including this batch.
    pub scanned: i64,
    /// How many keys the database held when the walk began (`DBSIZE`), or 0 if
    /// that couldn't be told: what `scanned` is measured against.
    pub total: i64,
    pub done: bool,
}

/// The entries of a value read the way the key panel reads it (`read_value`), for
/// a key the pipelined read could not fetch.
fn entries_of_value(key_type: &RedisValueType, value: Value) -> Vec<(String, String)> {
    let text = |v: &Value| match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    match (key_type, value) {
        (RedisValueType::String, value) => vec![(String::new(), text(&value))],
        (RedisValueType::Hash, Value::Object(map)) => map.iter().map(|(k, v)| (k.clone(), text(v))).collect(),
        (RedisValueType::List, Value::Array(items)) => items.iter().enumerate().map(|(i, v)| (format!("[{i}]"), text(v))).collect(),
        (RedisValueType::Set, Value::Array(items)) => items.iter().map(|v| (String::new(), text(v))).collect(),
        (RedisValueType::Zset, Value::Array(items)) => items
            .iter()
            .filter_map(|v| match v {
                Value::Array(pair) if pair.len() == 2 => Some((text(&pair[1]), text(&pair[0]))),
                _ => None,
            })
            .collect(),
        (RedisValueType::Stream, Value::Array(items)) => items
            .iter()
            .filter_map(|v| Some((text(v.get("id")?), v.get("values")?.to_string())))
            .collect(),
        _ => Vec::new(),
    }
}

/// Reads at most `limit` entries of each of `keys` in one pipelined round trip.
/// If the pipeline fails (a key vanished or changed type), each key is read on
/// its own and the failing ones are left empty.
async fn read_entries(session: &mut Session, keys: Vec<TypedKey>, limit: i64) -> Vec<KeyEntries> {
    if keys.is_empty() {
        return Vec::new();
    }
    let heads = session.entry_heads(&keys, limit).await.ok();
    let mut heads = heads.map(Vec::into_iter);
    let mut out = Vec::with_capacity(keys.len());
    for TypedKey { name, key_type } in keys {
        let entries = match heads.as_mut().and_then(Iterator::next) {
            Some(entries) => entries,
            // the pipeline failed as a whole: read this key alone
            None => match read_value(session, &name, &key_type, limit).await {
                Ok(read) => entries_of_value(&key_type, read.value),
                Err(_) => Vec::new(),
            },
        };
        out.push(KeyEntries { key: name, key_type, entries });
    }
    out
}

/// How many keys one read of content covers: a SCAN page is split into reads of
/// this many, so a huge key only holds up its own few neighbours.
const SEARCH_READ_KEYS: usize = 50;

/// How many of those reads run at once. Each goes through the next connection
/// of the pool, so a big reply on one doesn't hold the others up.
const SEARCH_READS_IN_FLIGHT: usize = 8;

/// How many SCAN pages the walk may be ahead of the reads: enough that the
/// reads never wait for a SCAN, few enough to not pile up key names.
const SEARCH_PAGES_AHEAD: usize = 4;

/// How often a paused walk looks whether it may go on (or must stop).
const PAUSED_CHECK_INTERVAL: std::time::Duration = std::time::Duration::from_millis(100);

/// A spawned task that is cancelled if it is dropped before it ends.
struct AbortOnDrop<T>(tokio::task::JoinHandle<T>);

impl<T> Drop for AbortOnDrop<T> {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// What the SCAN task hands over: a page of key names, or why the walk failed.
type ScanPage = Result<Vec<KeyName>, AppError>;

/// A read of some keys' content under way, with the number of keys SCAN had
/// walked when it was started.
type PendingRead = (i64, AbortOnDrop<Result<Vec<KeyEntries>, AppError>>);

/// Follows SCAN's cursor on its own task, a few pages ahead of the reads (the
/// channel's room). Ends, closing the channel, with the last page or the error
/// that stopped it.
async fn scan_pages(mut session: Session, pages: tokio::sync::mpsc::Sender<ScanPage>) {
    let mut cursor: u64 = 0;
    loop {
        match session.scan(cursor, None, 500).await {
            Ok((next, names)) => {
                // (a closed channel: nobody is reading any more)
                if pages.send(Ok(names)).await.is_err() || next == 0 {
                    return;
                }
                cursor = next;
            }
            Err(e) => {
                let _ = pages.send(Err(e)).await;
                return;
            }
        }
    }
}

/// The server-side prefilter of a search: the word (lowercased), and whether the
/// server has been found to take the script — once it doesn't (no permission to
/// run scripts, a server without them), the plain reads are used from then on.
#[derive(Clone)]
struct ServerFilter {
    needle: std::sync::Arc<str>,
    working: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

/// What the search script's answer holds, as the keys' entries; the keys whose
/// content the script leaves to the caller (streams) come back empty, with their
/// position, to be read the ordinary way.
fn keys_of_script_reply(reply: Vec<ScriptKey>) -> (Vec<KeyEntries>, Vec<usize>) {
    let mut keys = Vec::with_capacity(reply.len());
    let mut left_to_read = Vec::new();
    for (key, kind, status, flat) in reply {
        let key_type = RedisValueType::from(lossy(kind));
        if status == 1 {
            left_to_read.push(keys.len());
            keys.push(KeyEntries { key: KeyName::from(key), key_type, entries: Vec::new() });
        } else {
            keys.push(KeyEntries { key: KeyName::from(key), key_type, entries: pairs(flat).collect() });
        }
    }
    (keys, left_to_read)
}

/// `read_entries` done by the server: one round trip returning only the keys and
/// entries that might match `needle` (lowercased); the caller still checks them
/// exactly.
async fn read_entries_lua(session: &mut Session, names: &[KeyName], needle: &str, depth: i64) -> Result<Vec<KeyEntries>, AppError> {
    let reply = session.search_script(names, needle, depth).await?;
    let (mut keys, left_to_read) = keys_of_script_reply(reply);
    if !left_to_read.is_empty() {
        let asked = left_to_read.iter().map(|&i| TypedKey { name: keys[i].key.clone(), key_type: RedisValueType::Stream }).collect();
        for (&i, read) in left_to_read.iter().zip(read_entries(session, asked, depth).await) {
            keys[i].entries = read.entries;
        }
    }
    Ok(keys)
}

/// Reads what `names` hold: through the search script when there is a server
/// filter that works, otherwise (or if the server turns the script down) with a
/// pipelined `TYPE` and a pipelined read.
async fn read_chunk(mut session: Session, names: Vec<KeyName>, filter: Option<ServerFilter>, depth: i64) -> Result<Vec<KeyEntries>, AppError> {
    if let Some(filter) = &filter
        && filter.working.load(std::sync::atomic::Ordering::Relaxed)
    {
        match read_entries_lua(&mut session, &names, &filter.needle, depth).await {
            Ok(keys) => return Ok(keys),
            Err(_) => filter.working.store(false, std::sync::atomic::Ordering::Relaxed),
        }
    }
    let typed = session.types(names).await?;
    Ok(read_entries(&mut session, typed, depth).await)
}

/// What the loop of [`scan_entries`] waits for next.
enum Wake {
    Page(Option<ScanPage>),
    Read(Result<Result<Vec<KeyEntries>, AppError>, tokio::task::JoinError>),
}

/// Walks every key with SCAN, handing each page's keys and entries (at most `depth`
/// per key) to `on_batch` in SCAN order until `keep_going` is false. `server_filter`
/// (lowercased) has a Redis script drop what can't match; `is_paused` holds off new reads.
pub async fn scan_entries(
    state: &AppState,
    connection_id: Option<&str>,
    depth: i64,
    server_filter: Option<&str>,
    keep_going: impl Fn() -> bool,
    is_paused: impl Fn() -> bool,
    mut on_batch: impl FnMut(EntriesBatch),
) -> Result<(), AppError> {
    let mut session = Session::open(state, connection_id).await?;
    let total = session.dbsize().await.unwrap_or(0);
    let filter = server_filter.map(|needle| ServerFilter { needle: needle.into(), working: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true)) });

    let (sender, mut pages) = tokio::sync::mpsc::channel::<ScanPage>(SEARCH_PAGES_AHEAD);
    let _walk = AbortOnDrop(tokio::spawn(scan_pages(session.sibling(), sender)));

    let mut scanned: i64 = 0;
    let mut pages_done = false;
    let mut in_flight: VecDeque<PendingRead> = VecDeque::new();
    loop {
        if !keep_going() {
            return Ok(());
        }
        if pages_done && in_flight.is_empty() {
            // the last page held no keys: nothing left to read, but the caller
            // is still owed the batch that says it is over
            on_batch(EntriesBatch { keys: Vec::new(), scanned, total, done: true });
            return Ok(());
        }

        let paused = is_paused();
        if paused && in_flight.is_empty() {
            tokio::time::sleep(PAUSED_CHECK_INTERVAL).await;
            continue;
        }

        // Waits for the oldest read to end (its result is what goes out next, so
        // the order is kept), or, while there is room for one more, for the next page.
        let room = !paused && !pages_done && in_flight.len() < SEARCH_READS_IN_FLIGHT;
        let mut oldest = in_flight.pop_front();
        let wake = std::future::poll_fn(|cx| {
            use std::future::Future;
            use std::pin::Pin;
            use std::task::Poll;
            if let Some((_, read)) = oldest.as_mut()
                && let Poll::Ready(joined) = Pin::new(&mut read.0).poll(cx)
            {
                return Poll::Ready(Wake::Read(joined));
            }
            if room && let Poll::Ready(page) = pages.poll_recv(cx) {
                return Poll::Ready(Wake::Page(page));
            }
            Poll::Pending
        })
        .await;

        match wake {
            Wake::Read(joined) => {
                let (scanned_then, _) = oldest.take().expect("a read ended, so there was one");
                let keys = joined??;
                let done = pages_done && in_flight.is_empty();
                on_batch(EntriesBatch { keys, scanned: scanned_then, total, done });
                if done {
                    return Ok(());
                }
            }
            Wake::Page(page) => {
                if let Some(read) = oldest {
                    in_flight.push_front(read);
                }
                match page {
                    None => pages_done = true,
                    Some(Err(e)) => return Err(e),
                    Some(Ok(names)) => {
                        for chunk in names.chunks(SEARCH_READ_KEYS) {
                            // each read reports the keys walked up to its own end, so the
                            // count climbs read by read rather than page by page
                            scanned += chunk.len() as i64;
                            let read = tokio::spawn(read_chunk(session.sibling(), chunk.to_vec(), filter.clone(), depth));
                            in_flight.push_back((scanned, AbortOnDrop(read)));
                        }
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Reading a single key's value
// ---------------------------------------------------------------------------

/// Reads stream entries as `{id, values}` objects, from the start.
async fn read_stream(session: &mut Session, key: &KeyName, limit: Option<i64>) -> Result<Vec<Value>, AppError> {
    let raw = session.stream_head(key, limit).await?;

    Ok(raw
        .into_iter()
        .map(|(id, fields)| {
            let values: serde_json::Map<String, Value> =
                fields.into_iter().map(|(k, v)| (k, Value::String(v))).collect();
            let mut obj = serde_json::Map::new();
            obj.insert("id".to_string(), Value::String(id));
            obj.insert("values".to_string(), Value::Object(values));
            Value::Object(obj)
        })
        .collect())
}

fn score_to_json(score: f64) -> Value {
    serde_json::Number::from_f64(score).map(Value::Number).unwrap_or(Value::Null)
}

struct ReadValueResult {
    value: Value,
    truncated: bool,
}

fn no_truncation(value: Value) -> ReadValueResult {
    ReadValueResult { value, truncated: false }
}

/// Orders items by field name for display, in natural order (`a2` before `a10`, `2` before `10`),
/// or its reverse when `descending`: `HSCAN` gives them in an arbitrary bucket order.
fn sort_by_field_name<T>(items: &mut [T], name: impl Fn(&T) -> &str, descending: bool) {
    items.sort_by(|a, b| if descending { natord::compare(name(b), name(a)) } else { natord::compare(name(a), name(b)) });
}

/// Fetches `field_names`' values via `HMGET`, pairing them into a JSON
/// object in that order (a field deleted since its name was read is skipped).
/// Needs serde_json's `preserve_order` feature for the order to reach the UI.
async fn fetch_hash_fields_as_map(session: &mut Session, key: &KeyName, field_names: Vec<String>) -> Result<serde_json::Map<String, Value>, AppError> {
    let values = session.hash_values(key, &field_names).await?;

    Ok(field_names
        .into_iter()
        .zip(values)
        .filter_map(|(field, value)| Some((field, Value::String(value?))))
        .collect())
}

/// How many field names of a hash are read before they are sorted and shown: a
/// hash up to this size is shown fully sorted; past it, a batch at a time.
pub const HASH_NAMES_BATCH: usize = 10_000;

/// The longest string value read; past it, the value is cut and marked so.
pub const MAX_STRING_BYTES: usize = 4 * 1024 * 1024;

/// The field names of `key` from `HSCAN` cursor `cursor` on, until `limit` are read
/// or the scan ends (cursor 0), without the duplicates a scan may repeat. Each step
/// is short, so a huge hash never holds the server up the way `HKEYS` would.
async fn scan_field_names(session: &mut Session, key: &KeyName, mut cursor: u64, limit: usize) -> Result<(Vec<String>, u64), AppError> {
    let mut seen = FxHashSet::default();
    let mut names = Vec::new();
    loop {
        let (next, batch) = session.hash_scan_names(key, cursor, 1000).await?;
        names.extend(batch.into_iter().filter(|name| seen.insert(name.clone())));
        cursor = next;
        if cursor == 0 || names.len() >= limit {
            return Ok((names, cursor));
        }
    }
}

/// Reads a hash bounded to `max_items` fields, in field-name order among those read,
/// instead of a full `HGETALL`.
async fn read_hash_bounded(session: &mut Session, key: &KeyName, max_items: i64) -> Result<ReadValueResult, AppError> {
    let total = session.hash_len(key).await?;
    let limit = max_items.max(0) as usize;
    let (mut field_names, _) = scan_field_names(session, key, 0, limit).await?;
    field_names.truncate(limit);
    sort_by_field_name(&mut field_names, |name| name.as_str(), false);

    let map = fetch_hash_fields_as_map(session, key, field_names).await?;
    Ok(ReadValueResult { truncated: total > map.len() as i64, value: Value::Object(map) })
}

/// Same idea as [`read_hash_bounded`] but for sets via `SSCAN` instead of a
/// full `SMEMBERS`.
async fn read_set_bounded(session: &mut Session, key: &KeyName, max_items: i64) -> Result<ReadValueResult, AppError> {
    let total = session.set_len(key).await?;

    let mut items: Vec<String> = Vec::new();
    let mut cursor: u64 = 0;
    let count_hint = max_items.max(10);
    while (items.len() as i64) < max_items {
        let (next_cursor, chunk) = session.set_scan(key, cursor, count_hint).await?;
        let room = max_items as usize - items.len();
        items.extend(chunk.into_iter().take(room));
        cursor = next_cursor;
        if cursor == 0 {
            break;
        }
    }

    Ok(ReadValueResult {
        truncated: total > items.len() as i64,
        value: Value::Array(items.into_iter().map(Value::String).collect()),
    })
}

async fn read_value(session: &mut Session, key: &KeyName, value_type: &RedisValueType, max_items: i64) -> Result<ReadValueResult, AppError> {
    match value_type {
        RedisValueType::String => {
            let (len, head) = session.string_head(key, MAX_STRING_BYTES).await?;
            Ok(ReadValueResult { value: Value::String(lossy(head)), truncated: len > MAX_STRING_BYTES })
        }
        RedisValueType::Hash => read_hash_bounded(session, key, max_items).await,
        RedisValueType::List => {
            let (total, items) = session.list_head(key, max_items).await?;
            Ok(ReadValueResult {
                truncated: total > max_items,
                value: Value::Array(items.into_iter().map(Value::String).collect()),
            })
        }
        RedisValueType::Set => read_set_bounded(session, key, max_items).await,
        RedisValueType::Zset => {
            let (total, items) = session.zset_head(key, max_items).await?;
            Ok(ReadValueResult {
                truncated: total > max_items,
                value: Value::Array(
                    items
                        .into_iter()
                        .map(|(member, score)| Value::Array(vec![Value::String(member), score_to_json(score)]))
                        .collect(),
                ),
            })
        }
        RedisValueType::Stream => {
            Ok(no_truncation(Value::Array(read_stream(session, key, Some(max_items)).await?)))
        }
        // (get_key_details answers a missing key before it reads anything)
        RedisValueType::Missing => Err(AppError::not_found(t!("Key not found."))),
        RedisValueType::Other(other) => {
            Ok(no_truncation(Value::String(t!("[unsupported type: %{kind}]", kind = other).into_owned())))
        }
    }
}

/// The most entries of one key an export reads; a bigger key is cut and marked `truncated`.
const EXPORT_MAX_ITEMS: i64 = 1_000_000;

/// A key as an export writes it.
pub struct ExportedKey {
    pub key_type: RedisValueType,
    pub ttl: i64,
    pub value: Value,
    pub truncated: bool,
}

/// Reads a key whole for an export: its type, TTL (-1 for none) and value; `None` once it is gone.
pub async fn read_for_export(session: &mut Session, key: &KeyName) -> Result<Option<ExportedKey>, AppError> {
    let (key_type, ttl) = session.type_and_ttl(key).await?;
    if key_type == RedisValueType::Missing {
        return Ok(None);
    }
    let read = read_value(session, key, &key_type, EXPORT_MAX_ITEMS).await?;
    Ok(Some(ExportedKey { key_type, ttl: ttl.max(-1), value: read.value, truncated: read.truncated }))
}

pub struct KeyDetails {
    pub key_type: RedisValueType,
    pub ttl: i64,
    pub value: Value,
    pub truncated: bool,
    /// For a hash: the field names read so far, sorted as shown; `value` holds only one
    /// page, the rest are read a page at a time as the panel scrolls or a search
    /// needs them. `None` for every other type.
    pub all_field_names: Option<Vec<String>>,
    /// For a hash: where in `all_field_names` the fields of `value` start.
    pub page_start: usize,
    /// For a hash: how many fields it has, and where the scan of its names goes on
    /// (`None` once every name is read).
    pub field_count: usize,
    pub names_cursor: Option<u64>,
}

/// Fetches a key's type, TTL, and value, bounded to `max_items` entries for
/// hash/list/set/zset/stream; a hash's value is the page holding `focus`, else the first.
/// Type and TTL come in one pipelined round-trip; a missing key is type `none`.
pub async fn get_key_details(
    state: &AppState,
    connection_id: Option<&str>,
    key: &KeyName,
    max_items: i64,
    hash_page_size: usize,
    focus: Option<&str>,
    descending: bool,
) -> Result<KeyDetails, AppError> {
    if key.as_bytes().is_empty() {
        return Err(AppError::invalid(t!("The key parameter is required.")));
    }
    let mut session = Session::open(state, connection_id).await?;
    let (key_type, ttl) = session.type_and_ttl(key).await?;
    if key_type == RedisValueType::Missing {
        return Err(AppError::not_found(t!("Key not found.")));
    }
    let ttl = ttl.max(-1);

    if key_type == RedisValueType::Hash {
        let field_count = session.hash_len(key).await?.max(0) as usize;
        let (mut names, cursor) = scan_field_names(&mut session, key, 0, HASH_NAMES_BATCH).await?;
        // a field asked for that this batch doesn't hold is shown with it
        if let Some(focus) = focus
            && cursor != 0
            && !names.iter().any(|n| n == focus)
            && session.hash_exists(key, focus).await?
        {
            names.push(focus.to_string());
        }
        sort_by_field_name(&mut names, |name| name.as_str(), descending);
        let page_start = focus.and_then(|f| names.iter().position(|n| n == f)).map_or(0, |i| i / hash_page_size * hash_page_size);
        let page: Vec<String> = names.iter().skip(page_start).take(hash_page_size).cloned().collect();
        let map = fetch_hash_fields_as_map(&mut session, key, page).await?;
        let names_cursor = (cursor != 0).then_some(cursor);
        return Ok(KeyDetails { key_type, ttl, value: Value::Object(map), truncated: false, all_field_names: Some(names), page_start, field_count, names_cursor });
    }

    let read = read_value(&mut session, key, &key_type, max_items).await?;
    Ok(KeyDetails { key_type, ttl, value: read.value, truncated: read.truncated, all_field_names: None, page_start: 0, field_count: 0, names_cursor: None })
}

/// The next batch of a hash's field names, from where the last one's scan stopped:
/// sorted among themselves, and where the scan goes on (`None` at its end).
pub async fn hash_more_names(state: &AppState, connection_id: Option<&str>, key: &KeyName, cursor: u64, descending: bool) -> Result<(Vec<String>, Option<u64>), AppError> {
    let mut session = Session::open(state, connection_id).await?;
    let (mut names, next) = scan_field_names(&mut session, key, cursor, HASH_NAMES_BATCH).await?;
    sort_by_field_name(&mut names, |name| name.as_str(), descending);
    Ok((names, (next != 0).then_some(next)))
}

/// The values of exactly `names` (known field names of a hash) as a JSON object
/// in that order: a page past the first, fetched on scroll, search or JSON path
/// filter.
pub async fn hash_field_page(state: &AppState, connection_id: Option<&str>, key: &KeyName, names: &[String]) -> Result<Value, AppError> {
    if names.is_empty() {
        return Ok(Value::Object(serde_json::Map::new()));
    }
    let mut session = Session::open(state, connection_id).await?;
    let map = fetch_hash_fields_as_map(&mut session, key, names.to_vec()).await?;
    Ok(Value::Object(map))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes(text: &str) -> Vec<u8> {
        text.as_bytes().to_vec()
    }

    #[test]
    fn the_search_scripts_answer_becomes_the_keys_entries_and_marks_the_streams_to_read() {
        let reply: Vec<ScriptKey> = vec![
            (bytes("h"), bytes("hash"), 0, vec![bytes("a"), bytes("needle"), bytes("b"), bytes("Needle 2")]),
            (bytes("s"), bytes("stream"), 1, Vec::new()),
            (bytes("only-the-name"), bytes("string"), 0, Vec::new()),
            (bytes("z"), bytes("zset"), 0, vec![bytes("1.5"), bytes("member")]),
        ];
        let (keys, to_read) = keys_of_script_reply(reply);
        assert_eq!(keys.iter().map(|k| k.key.id()).collect::<Vec<_>>(), ["h", "s", "only-the-name", "z"]);
        assert_eq!(keys[0].entries, vec![("a".to_string(), "needle".to_string()), ("b".to_string(), "Needle 2".to_string())]);
        assert!(keys[0].key_type == RedisValueType::Hash && keys[1].key_type == RedisValueType::Stream);
        assert_eq!(to_read, vec![1], "the stream is read the ordinary way, at its place");
        assert!(keys[2].entries.is_empty(), "a key that only matches by name has no entries");
        assert_eq!(keys[3].entries, vec![("1.5".to_string(), "member".to_string())]);
    }
}
