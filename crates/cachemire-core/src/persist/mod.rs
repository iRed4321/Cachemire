//! Reading and writing the app's JSON files (`databases.json`, `settings.json`)
//! so that a crash or a bad file can't cost the user what they saved.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::error::{Error, FileError};

/// What reading a file gave.
pub enum Loaded<T> {
    /// there is no file: a first run
    Missing,
    Read(T),
    /// the file is there but can't be used (not valid JSON, an unknown shape,
    /// unreadable); why, for the log
    Unusable(String),
}

pub fn load<T: DeserializeOwned>(path: &Path) -> Loaded<T> {
    match fs::read_to_string(path) {
        Ok(json) => match serde_json::from_str(&json) {
            Ok(value) => Loaded::Read(value),
            Err(e) => Loaded::Unusable(e.to_string()),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Loaded::Missing,
        Err(e) => Loaded::Unusable(e.to_string()),
    }
}

/// Moves a file that can't be used out of the way, keeping it (`<name>.unusable-<ms>`
/// next to it), so that starting again from defaults, and saving those, doesn't
/// destroy what was in it. Returns where it went.
pub fn set_aside(path: &Path) -> Option<PathBuf> {
    let stamp = crate::now_ms();
    let name = path.file_name()?.to_string_lossy().into_owned();
    let target = path.with_file_name(format!("{name}.unusable-{stamp}"));
    fs::rename(path, &target).ok().map(|()| target)
}

/// A store's file read at startup: what it holds (`None` with no file), and whether the
/// store may save over it. A file that can't be used is moved aside first (see
/// [`set_aside`]); one that can't be moved is never written over this run.
pub fn load_store<T: DeserializeOwned>(path: &Path) -> (Option<T>, bool) {
    match load(path) {
        Loaded::Read(value) => (Some(value), true),
        Loaded::Missing => (None, true),
        Loaded::Unusable(why) => match set_aside(path) {
            Some(kept) => {
                tracing::error!("{} can't be read ({why}); kept as {}", path.display(), kept.display());
                (None, true)
            }
            None => {
                tracing::error!("{} can't be read ({why}) nor moved aside: nothing is saved over it", path.display());
                (None, false)
            }
        },
    }
}

/// Writes `value` as pretty JSON via a temp file, flushed then renamed over
/// the original, so the file is always the old content or the whole new one
/// — writing in place would truncate first, risking an unparseable half-write.
pub fn save<T: Serialize>(path: &Path, value: &T) -> Result<(), Error> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(FileError::from)?;
    }
    let json = serde_json::to_string_pretty(value).map_err(FileError::from)?;
    let name = path.file_name().ok_or_else(|| FileError::NoFileName(path.to_path_buf()))?.to_string_lossy().into_owned();
    let temporary = path.with_file_name(format!("{name}.tmp"));
    let written = (|| -> std::io::Result<()> {
        let mut file = fs::File::create(&temporary)?;
        file.write_all(json.as_bytes())?;
        file.sync_all()?;
        fs::rename(&temporary, path)
    })();
    written.map_err(|e| {
        let _ = fs::remove_file(&temporary);
        FileError::from(e).into()
    })
}

#[cfg(test)]
mod tests;
