//! Reading and writing the app's JSON files (`databases.json`, `settings.json`)
//! so that a crash or a bad file can't cost the user what they saved.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;
use serde::Serialize;

use super::error::AppError;

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
    let stamp = super::now_ms();
    let name = path.file_name()?.to_string_lossy().into_owned();
    let target = path.with_file_name(format!("{name}.unusable-{stamp}"));
    fs::rename(path, &target).ok().map(|()| target)
}

/// Writes `value` as pretty JSON via a temp file, flushed then renamed over
/// the original, so the file is always the old content or the whole new one
/// — writing in place would truncate first, risking an unparseable half-write.
pub fn save<T: Serialize>(path: &Path, value: &T) -> Result<(), AppError> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| AppError::File(e.to_string()))?;
    }
    let json = serde_json::to_string_pretty(value).map_err(|e| AppError::File(e.to_string()))?;
    let name = path.file_name().ok_or_else(|| AppError::File("no file name".into()))?.to_string_lossy().into_owned();
    let temporary = path.with_file_name(format!("{name}.tmp"));
    let written = (|| -> std::io::Result<()> {
        let mut file = fs::File::create(&temporary)?;
        file.write_all(json.as_bytes())?;
        file.sync_all()?;
        fs::rename(&temporary, path)
    })();
    written.map_err(|e| {
        let _ = fs::remove_file(&temporary);
        AppError::File(e.to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct Sample {
        name: String,
    }

    fn temp_dir(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("cachemire-persist-{name}-{}-{}", std::process::id(), super::super::now_ms()))
    }

    #[test]
    fn a_saved_file_reads_back_and_leaves_nothing_else_behind() {
        let dir = temp_dir("save");
        let file = dir.join("data.json");
        assert!(matches!(load::<Sample>(&file), Loaded::Missing));

        save(&file, &Sample { name: "one".into() }).unwrap();
        save(&file, &Sample { name: "two".into() }).unwrap();
        assert!(matches!(load::<Sample>(&file), Loaded::Read(Sample { name }) if name == "two"));
        let files: Vec<String> = fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        assert_eq!(files, vec!["data.json".to_string()], "the temporary file is gone");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_that_cannot_be_used_is_kept_aside() {
        let dir = temp_dir("aside");
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("data.json");
        fs::write(&file, "{ not json").unwrap();
        assert!(matches!(load::<Sample>(&file), Loaded::Unusable(_)));
        fs::write(&file, r#"{"other": 1}"#).unwrap();
        assert!(matches!(load::<Sample>(&file), Loaded::Unusable(_)), "the wrong shape is unusable too");

        let kept = set_aside(&file).expect("moved");
        assert!(!file.exists());
        assert_eq!(fs::read_to_string(kept).unwrap(), r#"{"other": 1}"#);
        let _ = fs::remove_dir_all(&dir);
    }
}
