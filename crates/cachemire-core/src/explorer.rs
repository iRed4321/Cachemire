use futures_util::stream::{FuturesOrdered, StreamExt};
use rustc_hash::{FxHashMap, FxHashSet};

use crate::error::{Error, Invalid, NotFound};
use crate::key_name::KeyName;
use serde_json::Value;

use crate::redis_value_type::RedisValueType;
use crate::session::{ScriptKey, Session, TypedKey, lossy, pairs, stream_fields};
use crate::state::AppState;

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
) -> Result<(), Error> {
    let mut session = Session::open(state, connection_id).await?;

    // SCAN cursors the whole keyspace regardless of MATCH, so a prefix's
    // cost is round trips, not results — a bigger COUNT there skips more
    // server-side per trip; with no prefix a smaller COUNT bounds each reply.
    let prefix = prefix.as_bytes();
    let delimiter = delimiter.as_bytes();
    let (match_pattern, count) =
        if prefix.is_empty() { (b"*".to_vec(), 1000) } else { ([glob_escape(prefix), b"*".to_vec()].concat(), 10_000) };

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
    pub scanned: usize,
    pub total: usize,
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
    max_keys: usize,
    keep_going: impl Fn() -> bool,
    mut on_batch: impl FnMut(WalkBatch),
) -> Result<(), Error> {
    let mut session = Session::open(state, connection_id).await?;
    let total = session.dbsize().await.unwrap_or(0);
    let (mut cursor, mut scanned) = (0u64, 0usize);
    loop {
        if !keep_going() {
            return Ok(());
        }
        let (next, names) = session.scan(cursor, None, 1000).await?;
        scanned += names.len();
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
    pub scanned: usize,
    /// How many keys the database held when the walk began (`DBSIZE`), or 0 if
    /// that couldn't be told: what `scanned` is measured against.
    pub total: usize,
    pub done: bool,
}

/// Reads at most `limit` entries of each of `keys` in one pipelined round trip.
/// If the pipeline fails (a key vanished or changed type), each key is read on
/// its own and the failing ones are left empty.
async fn read_entries(session: &mut Session, keys: Vec<TypedKey>, limit: usize) -> Vec<KeyEntries> {
    if keys.is_empty() {
        return Vec::new();
    }
    let heads = session.entry_heads(&keys, limit).await.ok();
    let mut heads = heads.map(Vec::into_iter);
    let mut out = Vec::with_capacity(keys.len());
    for key in keys {
        let entries = match heads.as_mut().and_then(Iterator::next) {
            Some(entries) => entries,
            // the pipeline failed as a whole: read this key alone
            None => session.entry_heads(std::slice::from_ref(&key), limit).await.ok().and_then(|mut one| one.pop()).unwrap_or_default(),
        };
        out.push(KeyEntries { key: key.name, key_type: key.key_type, entries });
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

/// A spawned task that is cancelled if it is dropped before it ends; awaiting it
/// awaits the task.
struct AbortOnDrop<T>(tokio::task::JoinHandle<T>);

impl<T> Drop for AbortOnDrop<T> {
    fn drop(&mut self) {
        self.0.abort();
    }
}

impl<T> std::future::Future for AbortOnDrop<T> {
    type Output = Result<T, tokio::task::JoinError>;

    fn poll(mut self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<Self::Output> {
        std::pin::Pin::new(&mut self.0).poll(cx)
    }
}

/// What the SCAN task hands over: a page of key names, or why the walk failed.
type ScanPage = Result<Vec<KeyName>, Error>;

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
async fn read_entries_lua(session: &mut Session, names: &[KeyName], needle: &str, depth: usize) -> Result<Vec<KeyEntries>, Error> {
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
async fn read_chunk(
    mut session: Session,
    names: Vec<KeyName>,
    filter: Option<ServerFilter>,
    depth: usize,
) -> Result<Vec<KeyEntries>, Error> {
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

/// Walks every key with SCAN, handing each page's keys and entries (at most `depth`
/// per key) to `on_batch` in SCAN order until `keep_going` is false. `server_filter`
/// (lowercased) has a Redis script drop what can't match; `paused` holds off new reads.
pub async fn scan_entries(
    state: &AppState,
    connection_id: Option<&str>,
    depth: usize,
    server_filter: Option<&str>,
    keep_going: impl Fn() -> bool,
    mut paused: tokio::sync::watch::Receiver<bool>,
    mut on_batch: impl FnMut(EntriesBatch),
) -> Result<(), Error> {
    let mut session = Session::open(state, connection_id).await?;
    let total = session.dbsize().await.unwrap_or(0);
    let filter = server_filter
        .map(|needle| ServerFilter { needle: needle.into(), working: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true)) });

    let (sender, mut pages) = tokio::sync::mpsc::channel::<ScanPage>(SEARCH_PAGES_AHEAD);
    let _walk = AbortOnDrop(tokio::spawn(scan_pages(session.sibling(), sender)));

    // the reads under way, each with the number of keys SCAN had walked when it was
    // started; they end in any order but come out in the order they were started
    let mut reads = FuturesOrdered::new();
    let mut scanned: usize = 0;
    let mut pages_done = false;
    loop {
        if !keep_going() {
            return Ok(());
        }
        if pages_done && reads.is_empty() {
            // the last page held no keys: nothing left to read, but the caller
            // is still owed the batch that says it is over
            on_batch(EntriesBatch { keys: Vec::new(), scanned, total, done: true });
            return Ok(());
        }

        let is_paused = *paused.borrow_and_update();
        if is_paused && reads.is_empty() {
            // resumed, or the search is gone (its sender dropped)
            if paused.changed().await.is_err() {
                return Ok(());
            }
            continue;
        }

        let room = !is_paused && !pages_done && reads.len() < SEARCH_READS_IN_FLIGHT;
        tokio::select! {
            biased;
            Some((scanned_then, joined)) = reads.next() => {
                let keys = joined??;
                let done = pages_done && reads.is_empty();
                on_batch(EntriesBatch { keys, scanned: scanned_then, total, done });
                if done {
                    return Ok(());
                }
            }
            page = pages.recv(), if room => match page {
                None => pages_done = true,
                Some(Err(e)) => return Err(e),
                Some(Ok(names)) => {
                    for chunk in names.chunks(SEARCH_READ_KEYS) {
                        // each read reports the keys walked up to its own end, so the
                        // count climbs read by read rather than page by page
                        scanned += chunk.len();
                        let read = AbortOnDrop(tokio::spawn(read_chunk(session.sibling(), chunk.to_vec(), filter.clone(), depth)));
                        reads.push_back(async move { (scanned, read.await) });
                    }
                }
            },
        }
    }
}

// ---------------------------------------------------------------------------
// Reading a single key's value
// ---------------------------------------------------------------------------

/// Reads stream entries as `{id, values}` objects, from the start.
async fn read_stream(session: &mut Session, key: &KeyName, limit: Option<usize>) -> Result<Vec<Value>, Error> {
    let raw = session.stream_head(key, limit).await?;

    Ok(raw.into_iter().map(|(id, fields)| serde_json::json!({ "id": id, "values": stream_fields(fields) })).collect())
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

/// Values of a hash's fields, as [`fetch_hash_fields_as_map`] reads them: a JSON object
/// in field order, and the size in bytes of each field left out for being too big.
pub type HashValues = (serde_json::Map<String, Value>, FxHashMap<String, usize>);

/// Fetches `field_names`' values via `HMGET` into a JSON object in that order (a field
/// deleted since is skipped); with `max_bytes`, a pipelined `HSTRLEN` first leaves out
/// any value past it. Needs serde_json's `preserve_order` for the order to reach the UI.
async fn fetch_hash_fields_as_map(
    session: &mut Session,
    key: &KeyName,
    field_names: Vec<String>,
    max_bytes: Option<usize>,
) -> Result<HashValues, Error> {
    let mut oversized = FxHashMap::default();
    let field_names = match max_bytes {
        Some(max) => {
            let sizes = session.hash_value_sizes(key, &field_names).await?;
            field_names
                .into_iter()
                .zip(sizes)
                .filter_map(|(field, size)| {
                    if size <= max {
                        return Some(field);
                    }
                    oversized.insert(field, size);
                    None
                })
                .collect()
        }
        None => field_names,
    };
    let values = session.hash_values(key, &field_names).await?;

    let map = field_names.into_iter().zip(values).filter_map(|(field, value)| Some((field, Value::String(value?)))).collect();
    Ok((map, oversized))
}

/// How many field names of a hash are read before they are sorted and shown: a
/// hash up to this size is shown fully sorted; past it, a batch at a time.
pub const HASH_NAMES_BATCH: usize = 10_000;

/// The longest string value read; past it, the value is cut and marked so.
pub const MAX_STRING_BYTES: usize = 4 * 1024 * 1024;

/// The biggest hash field value the key panel reads with its page; a bigger one is
/// shown by its size, and read only when it's opened, saved, copied or asked for.
pub const MAX_FIELD_BYTES: usize = 4 * 1024 * 1024;

/// The field names of `key` from `HSCAN` cursor `cursor` on, until `limit` are read
/// or the scan ends (cursor 0), without the duplicates a scan may repeat. Each step
/// is short, so a huge hash never holds the server up the way `HKEYS` would.
async fn scan_field_names(session: &mut Session, key: &KeyName, mut cursor: u64, limit: usize) -> Result<(Vec<String>, u64), Error> {
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
async fn read_hash_bounded(session: &mut Session, key: &KeyName, max_items: usize) -> Result<ReadValueResult, Error> {
    let total = session.hash_len(key).await?;
    let (mut field_names, _) = scan_field_names(session, key, 0, max_items).await?;
    field_names.truncate(max_items);
    sort_by_field_name(&mut field_names, |name| name.as_str(), false);

    let (map, _) = fetch_hash_fields_as_map(session, key, field_names, None).await?;
    Ok(ReadValueResult { truncated: total > map.len(), value: Value::Object(map) })
}

/// Same idea as [`read_hash_bounded`] but for sets via `SSCAN` instead of a
/// full `SMEMBERS`.
async fn read_set_bounded(session: &mut Session, key: &KeyName, max_items: usize) -> Result<ReadValueResult, Error> {
    let total = session.set_len(key).await?;

    let mut items: Vec<String> = Vec::new();
    let mut cursor: u64 = 0;
    let count_hint = max_items.max(10);
    while items.len() < max_items {
        let (next_cursor, chunk) = session.set_scan(key, cursor, count_hint).await?;
        let room = max_items - items.len();
        items.extend(chunk.into_iter().take(room));
        cursor = next_cursor;
        if cursor == 0 {
            break;
        }
    }

    Ok(ReadValueResult { truncated: total > items.len(), value: Value::Array(items.into_iter().map(Value::String).collect()) })
}

async fn read_value(session: &mut Session, key: &KeyName, value_type: &RedisValueType, max_items: usize) -> Result<ReadValueResult, Error> {
    match value_type {
        RedisValueType::String => {
            let (len, head) = session.string_head(key, MAX_STRING_BYTES).await?;
            Ok(ReadValueResult { value: Value::String(lossy(head)), truncated: len > MAX_STRING_BYTES })
        }
        RedisValueType::Hash => read_hash_bounded(session, key, max_items).await,
        RedisValueType::List => {
            let (total, items) = session.list_head(key, max_items).await?;
            Ok(ReadValueResult { truncated: total > max_items, value: Value::Array(items.into_iter().map(Value::String).collect()) })
        }
        RedisValueType::Set => read_set_bounded(session, key, max_items).await,
        RedisValueType::Zset => {
            let (total, items) = session.zset_head(key, max_items).await?;
            Ok(ReadValueResult {
                truncated: total > max_items,
                value: Value::Array(
                    items.into_iter().map(|(member, score)| Value::Array(vec![Value::String(member), score_to_json(score)])).collect(),
                ),
            })
        }
        RedisValueType::Stream => Ok(no_truncation(Value::Array(read_stream(session, key, Some(max_items)).await?))),
        // (get_key_details answers a missing key before it reads anything)
        RedisValueType::Missing => Err(NotFound::Key.into()),
        RedisValueType::Other(other) => Ok(no_truncation(Value::String(format!("[unsupported type: {other}]")))),
    }
}

/// The most entries of one key an export reads; a bigger key is cut and marked `truncated`.
const EXPORT_MAX_ITEMS: usize = 1_000_000;

/// A key as an export writes it.
pub struct ExportedKey {
    pub key_type: RedisValueType,
    pub ttl: i64,
    pub value: Value,
    pub truncated: bool,
}

/// Reads a key whole for an export: its type, TTL (-1 for none) and value; `None` once it is gone.
pub async fn read_for_export(session: &mut Session, key: &KeyName) -> Result<Option<ExportedKey>, Error> {
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
    /// For a hash: the size in bytes of each field of the page left out of `value`
    /// for being over [`MAX_FIELD_BYTES`].
    pub oversized: FxHashMap<String, usize>,
    /// For a hash: where in `all_field_names` the fields of `value` start.
    pub page_start: usize,
    /// For a hash: how many fields it has, and where the scan of its names goes on
    /// (`None` once every name is read).
    pub field_count: usize,
    pub names_cursor: Option<u64>,
}

impl KeyDetails {
    /// A hash with no field yet: one made in the app, which Redis only stores
    /// once it has a field.
    pub fn empty_hash() -> Self {
        Self {
            key_type: RedisValueType::Hash,
            ttl: -1,
            value: Value::Object(serde_json::Map::new()),
            truncated: false,
            all_field_names: Some(Vec::new()),
            oversized: FxHashMap::default(),
            page_start: 0,
            field_count: 0,
            names_cursor: None,
        }
    }
}

/// Fetches a key's type, TTL, and value, bounded to `max_items` entries for
/// hash/list/set/zset/stream; a hash's value is the page holding `focus`, else the first.
/// Type and TTL come in one pipelined round-trip; a missing key is type `none`.
pub async fn get_key_details(
    state: &AppState,
    connection_id: Option<&str>,
    key: &KeyName,
    max_items: usize,
    hash_page_size: usize,
    focus: Option<&str>,
    descending: bool,
) -> Result<KeyDetails, Error> {
    if key.as_bytes().is_empty() {
        return Err(Invalid::KeyRequired.into());
    }
    let mut session = Session::open(state, connection_id).await?;
    let (key_type, ttl) = session.type_and_ttl(key).await?;
    if key_type == RedisValueType::Missing {
        return Err(NotFound::Key.into());
    }
    let ttl = ttl.max(-1);

    if key_type == RedisValueType::Hash {
        let field_count = session.hash_len(key).await?;
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
        let (map, oversized) = fetch_hash_fields_as_map(&mut session, key, page, Some(MAX_FIELD_BYTES)).await?;
        let names_cursor = (cursor != 0).then_some(cursor);
        return Ok(KeyDetails {
            key_type,
            ttl,
            value: Value::Object(map),
            truncated: false,
            all_field_names: Some(names),
            oversized,
            page_start,
            field_count,
            names_cursor,
        });
    }

    let read = read_value(&mut session, key, &key_type, max_items).await?;
    Ok(KeyDetails {
        key_type,
        ttl,
        value: read.value,
        truncated: read.truncated,
        all_field_names: None,
        oversized: FxHashMap::default(),
        page_start: 0,
        field_count: 0,
        names_cursor: None,
    })
}

/// The next batch of a hash's field names, from where the last one's scan stopped:
/// sorted among themselves, and where the scan goes on (`None` at its end).
pub async fn hash_more_names(
    state: &AppState,
    connection_id: Option<&str>,
    key: &KeyName,
    cursor: u64,
    descending: bool,
) -> Result<(Vec<String>, Option<u64>), Error> {
    let mut session = Session::open(state, connection_id).await?;
    let (mut names, next) = scan_field_names(&mut session, key, cursor, HASH_NAMES_BATCH).await?;
    sort_by_field_name(&mut names, |name| name.as_str(), descending);
    Ok((names, (next != 0).then_some(next)))
}

/// The values of exactly `names` (known field names of a hash), those over
/// [`MAX_FIELD_BYTES`] left out: a page past the first, fetched on scroll, search
/// or JSON path filter.
pub async fn hash_field_page(state: &AppState, connection_id: Option<&str>, key: &KeyName, names: &[String]) -> Result<HashValues, Error> {
    if names.is_empty() {
        return Ok(Default::default());
    }
    let mut session = Session::open(state, connection_id).await?;
    fetch_hash_fields_as_map(&mut session, key, names.to_vec(), Some(MAX_FIELD_BYTES)).await
}

/// The values of `names` in a hash, whatever their size, in that order (`None` for
/// a field that is gone).
pub async fn hash_full_values(
    state: &AppState,
    connection_id: Option<&str>,
    key: &KeyName,
    names: &[String],
) -> Result<Vec<Option<String>>, Error> {
    let mut session = Session::open(state, connection_id).await?;
    session.hash_values(key, names).await
}

/// Writes `value` as what `key` holds: the value of its field `field` for a hash
/// (`Some`), its whole value, TTL kept, for a string (`None`).
pub async fn write_value(
    state: &AppState,
    connection_id: Option<&str>,
    key: &KeyName,
    field: Option<&str>,
    value: &str,
) -> Result<(), Error> {
    let mut session = Session::open(state, connection_id).await?;
    match field {
        Some(field) => session.hash_set(key, field, value).await,
        None => session.string_set(key, value).await,
    }
}

/// Moves hash `key`'s field `from` to the new name `to`, with `value` as its value:
/// false, nothing changed, when the hash has a field `to` already.
pub async fn rename_hash_field(
    state: &AppState,
    connection_id: Option<&str>,
    key: &KeyName,
    from: &str,
    to: &str,
    value: &str,
) -> Result<bool, Error> {
    let mut session = Session::open(state, connection_id).await?;
    if !session.hash_set_new(key, to, value).await? {
        return Ok(false);
    }
    session.hash_delete(key, from).await?;
    Ok(true)
}

/// Writes `value` into a new field `field` of hash `key` (making the hash if there is
/// none): false, nothing changed, when it has that field already.
pub async fn write_new_hash_field(
    state: &AppState,
    connection_id: Option<&str>,
    key: &KeyName,
    field: &str,
    value: &str,
) -> Result<bool, Error> {
    Session::open(state, connection_id).await?.hash_set_new(key, field, value).await
}

/// Removes field `field` from hash `key`.
pub async fn delete_hash_field(state: &AppState, connection_id: Option<&str>, key: &KeyName, field: &str) -> Result<(), Error> {
    Session::open(state, connection_id).await?.hash_delete(key, field).await
}

/// Whether `key` exists.
pub async fn key_exists(state: &AppState, connection_id: Option<&str>, key: &KeyName) -> Result<bool, Error> {
    let mut session = Session::open(state, connection_id).await?;
    Ok(session.existing(std::slice::from_ref(key)).await?.first().copied().unwrap_or(false))
}

/// Removes `key`.
pub async fn delete_key(state: &AppState, connection_id: Option<&str>, key: &KeyName) -> Result<(), Error> {
    Session::open(state, connection_id).await?.unlink(std::slice::from_ref(key)).await
}

/// Renames `from` to `to`: false, nothing changed, when a key `to` exists already.
pub async fn rename_key(state: &AppState, connection_id: Option<&str>, from: &KeyName, to: &KeyName) -> Result<bool, Error> {
    Session::open(state, connection_id).await?.rename_new(from, to).await
}

/// Every key whose name starts with `prefix`.
async fn keys_under(session: &mut Session, prefix: &KeyName) -> Result<Vec<KeyName>, Error> {
    let mut pattern = glob_escape(prefix.as_bytes());
    pattern.push(b'*');
    let (mut cursor, mut keys) = (0u64, FxHashSet::default());
    loop {
        let (next, names) = session.scan(cursor, Some(&pattern), 1000).await?;
        keys.extend(names);
        cursor = next;
        if cursor == 0 {
            return Ok(keys.into_iter().collect());
        }
    }
}

/// How many keys one `UNLINK` or one round of renames takes.
const KEYS_PER_WRITE: usize = 500;

/// Removes every key whose name starts with `prefix` (a folder); how many there were.
pub async fn delete_folder(state: &AppState, connection_id: Option<&str>, prefix: &KeyName) -> Result<usize, Error> {
    let mut session = Session::open(state, connection_id).await?;
    let keys = keys_under(&mut session, prefix).await?;
    for chunk in keys.chunks(KEYS_PER_WRITE) {
        session.unlink(chunk).await?;
    }
    Ok(keys.len())
}

/// What moving a folder's keys did.
pub enum FolderMove {
    /// every key, as (old name, new name)
    Moved(Vec<(KeyName, KeyName)>),
    /// nothing, since this new name is a key already
    Taken(KeyName),
}

/// Moves every key whose name starts with `from` to the same name starting with `to`,
/// once none of the new names is taken.
pub async fn rename_folder(state: &AppState, connection_id: Option<&str>, from: &KeyName, to: &KeyName) -> Result<FolderMove, Error> {
    let mut session = Session::open(state, connection_id).await?;
    let renamed = |key: &KeyName| {
        let mut name = to.as_bytes().to_vec();
        name.extend_from_slice(&key.as_bytes()[from.as_bytes().len()..]);
        KeyName::from(name)
    };
    let moves: Vec<(KeyName, KeyName)> = keys_under(&mut session, from)
        .await?
        .into_iter()
        .map(|key| {
            let new = renamed(&key);
            (key, new)
        })
        .collect();
    for chunk in moves.chunks(KEYS_PER_WRITE) {
        let targets: Vec<KeyName> = chunk.iter().map(|(_, new)| new.clone()).collect();
        if let Some(i) = session.existing(&targets).await?.iter().position(|taken| *taken) {
            return Ok(FolderMove::Taken(targets[i].clone()));
        }
    }
    let mut moved = Vec::with_capacity(moves.len());
    for (old, new) in moves {
        if session.rename_new(&old, &new).await? {
            moved.push((old, new));
        }
    }
    Ok(FolderMove::Moved(moved))
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

    #[test]
    fn a_folder_named_like_a_glob_only_matches_itself() {
        assert_eq!(glob_escape(b"user:*"), b"user:\\*");
        assert_eq!(glob_escape(b"a?[b]\\c"), b"a\\?\\[b\\]\\\\c");
        assert_eq!(glob_escape(b"plain:"), b"plain:", "an ordinary prefix stays as it is");
    }

    #[test]
    fn hash_fields_are_shown_in_natural_order_either_way() {
        let mut fields = vec!["a10", "b", "a2", "10", "2"];
        sort_by_field_name(&mut fields, |f| f, false);
        assert_eq!(fields, ["2", "10", "a2", "a10", "b"]);
        sort_by_field_name(&mut fields, |f| f, true);
        assert_eq!(fields, ["b", "a10", "a2", "10", "2"]);
    }

    #[test]
    fn a_score_json_cant_hold_becomes_null() {
        assert_eq!(score_to_json(1.5), serde_json::json!(1.5));
        assert_eq!(score_to_json(f64::INFINITY), Value::Null, "ZADD accepts +inf");
        assert_eq!(score_to_json(f64::NAN), Value::Null);
    }
}
