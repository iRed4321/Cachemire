//! Writes keys to JSON files in a folder: one file per folder of keys (`redis-folder-*.json`,
//! its items streamed in), and one per lone key (`redis-key-*.json`), or per parent folder when grouping.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use rustc_hash::FxHashSet;
use serde_json::{Value, json};

use crate::error::{Error, FileError};
use crate::explorer::{self, ExportedKey};
use crate::key_name::KeyName;
use crate::session::Session;
use crate::state::AppState;

/// What to export, as key ids: whole folders (their prefix and every key under it) and lone keys.
#[derive(Clone)]
pub struct Plan {
    pub folders: Vec<(String, Vec<String>)>,
    pub keys: Vec<String>,
    /// lone keys go into one file per parent folder instead of one file each
    pub group: bool,
}

impl Plan {
    pub fn total(&self) -> usize {
        self.folders.iter().map(|(_, keys)| keys.len()).sum::<usize>() + self.keys.len()
    }

    /// The name of the file the plan writes, when it writes just one.
    pub fn sole_file(&self) -> Option<String> {
        let mut files = outputs(self.clone()).into_iter().map(|output| match output {
            Output::Items { file, .. } | Output::Single { file, .. } => file,
        });
        files.next().filter(|_| files.next().is_none())
    }
}

/// How an export ended.
pub struct Outcome {
    pub keys: usize,
    pub cancelled: bool,
}

/// One file to write: many keys under a folder prefix, or a single key.
enum Output {
    Items { file: String, prefix: String, keys: Vec<String> },
    Single { file: String, key: String },
}

/// `value` as a file name's stem: characters a file name can't hold become `_`.
fn file_safe(value: &str) -> String {
    let mut out = String::new();
    for c in value.chars() {
        let bad = matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') || c.is_control();
        if !bad {
            out.push(c);
        } else if !out.ends_with('_') {
            out.push('_');
        }
    }
    let out: String = out.trim().chars().take(120).collect();
    if out.is_empty() { "export".to_string() } else { out }
}

/// `stem` plus `.json`, numbered when another file of this export already has that name,
/// whatever its case (Windows' and macOS' file names ignore it).
fn unique_name(used: &mut FxHashSet<String>, stem: String) -> String {
    let mut name = format!("{stem}.json");
    let mut n = 2;
    while !used.insert(name.to_lowercase()) {
        name = format!("{stem}-{n}.json");
        n += 1;
    }
    name
}

/// The folder a key sits in: its id up to the last `:` (empty for a key at the root).
fn parent_folder(key: &str) -> &str {
    key.rfind(':').map_or("", |i| &key[..=i])
}

fn outputs(plan: Plan) -> Vec<Output> {
    let mut used = FxHashSet::default();
    let mut out = Vec::new();
    let one_lone_key = plan.folders.is_empty() && plan.keys.len() == 1;
    for (prefix, keys) in plan.folders {
        let file = unique_name(&mut used, format!("redis-folder-{}", file_safe(if prefix.is_empty() { "root" } else { &prefix })));
        out.push(Output::Items { file, prefix, keys });
    }
    if plan.group && !one_lone_key {
        let mut buckets: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for key in plan.keys {
            buckets.entry(parent_folder(&key).to_string()).or_default().push(key);
        }
        for (prefix, keys) in buckets {
            let file = unique_name(&mut used, format!("redis-folder-{}", file_safe(if prefix.is_empty() { "root-keys" } else { &prefix })));
            out.push(Output::Items { file, prefix, keys });
        }
    } else {
        for key in plan.keys {
            let file = unique_name(&mut used, format!("redis-key-{}", file_safe(&key)));
            out.push(Output::Single { file, key });
        }
    }
    out
}

fn io_error(e: std::io::Error) -> Error {
    FileError::from(e).into()
}

/// The key as the `{key, type, ttl, value}` object a file holds.
fn item(key: &str, read: &ExportedKey) -> Value {
    let mut item =
        json!({ "key": KeyName::from_id(key).to_string_lossy(), "type": read.key_type.as_str(), "ttl": read.ttl, "value": read.value });
    if read.truncated {
        item["truncated"] = Value::Bool(true);
    }
    item
}

/// Creates `path` and writes the start of `output`'s file into it.
fn start_file(path: &Path, output: &Output) -> Result<BufWriter<File>, Error> {
    let mut writer = BufWriter::new(File::create(path).map_err(io_error)?);
    if let Output::Items { prefix, .. } = output {
        write!(writer, "{{\n  \"prefix\": {},\n  \"items\": [", Value::from(prefix.as_str())).map_err(io_error)?;
    }
    Ok(writer)
}

/// Writes `plan`'s files into `dir`, reading each key off `connection_id`'s server; `progress` gets
/// the keys done so far and the total after each one. A plan writing a single file names it
/// `file_name` when given. Stops at once, leaving the file it was writing out, if `cancel` is set.
pub async fn run(
    state: &AppState,
    connection_id: &str,
    plan: Plan,
    dir: &Path,
    file_name: Option<&str>,
    cancel: &AtomicBool,
    mut progress: impl FnMut(usize, usize),
) -> Result<Outcome, Error> {
    let total = plan.total();
    let mut session = Session::open(state, Some(connection_id)).await?;
    std::fs::create_dir_all(dir).map_err(io_error)?;
    let mut done = 0;
    let sole = plan.sole_file().and(file_name);
    for output in outputs(plan) {
        let (file, keys): (&str, &[String]) = match &output {
            Output::Items { file, keys, .. } => (file, keys),
            Output::Single { file, key } => (file, std::slice::from_ref(key)),
        };
        let path = dir.join(sole.unwrap_or(file));
        let mut writer: Option<BufWriter<File>> = None;
        let mut written = 0;
        for key in keys {
            if cancel.load(Ordering::Relaxed) {
                if writer.take().is_some() {
                    let _ = std::fs::remove_file(&path);
                }
                return Ok(Outcome { keys: done, cancelled: true });
            }
            match explorer::read_for_export(&mut session, &KeyName::from_id(key)).await? {
                Some(read) => {
                    let writer = match &mut writer {
                        Some(writer) => writer,
                        None => writer.insert(start_file(&path, &output)?),
                    };
                    let json = item(key, &read);
                    if matches!(output, Output::Single { .. }) {
                        serde_json::to_writer_pretty(&mut *writer, &json).map_err(FileError::from)?;
                    } else {
                        let text = serde_json::to_string_pretty(&json).map_err(FileError::from)?;
                        writer.write_all(if written == 0 { b"\n    " } else { b",\n    " }).map_err(io_error)?;
                        writer.write_all(text.replace('\n', "\n    ").as_bytes()).map_err(io_error)?;
                    }
                    written += 1;
                }
                None => tracing::warn!(key = %KeyName::display_id(key), "key gone before it could be exported; left out"),
            }
            done += 1;
            progress(done, total);
        }
        // no file at all when every key of it was gone
        if let Some(mut writer) = writer {
            if matches!(output, Output::Items { .. }) {
                write!(writer, "\n  ],\n  \"totalKeys\": {written}\n}}").map_err(io_error)?;
            }
            writer.flush().map_err(io_error)?;
        }
    }
    Ok(Outcome { keys: done, cancelled: false })
}

#[cfg(test)]
mod tests;
