//! Helpers shared by the tasks: running commands and finding the repository.

use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

pub type Result<T> = std::result::Result<T, String>;

/// The repository root (the folder holding the root `Cargo.toml`).
pub fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).parent().expect("xtask sits in the repository").to_path_buf()
}

/// The `cargo` that is running this task.
pub fn cargo() -> Command {
    Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
}

/// Whether `program` starts and succeeds with `arg` (e.g. `--version`).
pub fn works(program: impl AsRef<std::ffi::OsStr>, arg: &str) -> bool {
    Command::new(program).arg(arg).output().is_ok_and(|o| o.status.success())
}

/// Runs `command` with its output shown, failing if it can't start or exits non-zero.
pub fn run(command: &mut Command) -> Result<()> {
    let shown = format!("{command:?}").replace('"', "");
    println!("> {shown}");
    let status = command.status().map_err(|e| format!("could not run `{shown}`: {e}"))?;
    status.success().then_some(()).ok_or_else(|| format!("`{shown}` failed ({status})"))
}

/// Runs `git` in `dir` and returns its trimmed output, failing on a non-zero exit.
pub fn git(dir: &std::path::Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git").current_dir(dir).args(args).output().map_err(|e| format!("could not run git: {e}"))?;
    if !output.status.success() {
        return Err(format!("`git {}` failed: {}", args.join(" "), String::from_utf8_lossy(&output.stderr).trim()));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Appends `markdown` to the GitHub Actions job summary; does nothing outside of GitHub Actions.
pub fn summary(markdown: &str) -> Result<()> {
    let Some(path) = std::env::var_os("GITHUB_STEP_SUMMARY") else { return Ok(()) };
    let mut file = std::fs::OpenOptions::new().append(true).create(true).open(path).map_err(|e| format!("job summary: {e}"))?;
    io::Write::write_all(&mut file, format!("{markdown}\n").as_bytes()).map_err(|e| format!("job summary: {e}"))
}

/// Adds the packages in `files` to the job summary, with their sizes.
pub fn summary_packages(files: &[&Path]) -> Result<()> {
    let mut markdown = String::from("#### Packages\n\n| File | Size |\n|---|---:|\n");
    for file in files {
        let size = std::fs::metadata(file).map_err(|e| format!("{}: {e}", file.display()))?.len();
        let name = file.file_name().unwrap_or_default().to_string_lossy();
        markdown += &format!("| `{name}` | {:.1} MB |\n", size as f64 / 1_048_576.0);
    }
    summary(&markdown)
}

/// Writes `zip_path`, a zip holding just `file` with the Unix `mode`.
pub fn zip_file(file: &Path, zip_path: &Path, mode: u32) -> Result<()> {
    println!("Creating zip: {}", zip_path.display());
    let err = |e: &dyn std::fmt::Display| format!("zip {}: {e}", zip_path.display());
    let mut zip = zip::ZipWriter::new(File::create(zip_path).map_err(|e| err(&e))?);
    let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated).unix_permissions(mode);
    zip.start_file(file.file_name().and_then(|n| n.to_str()).unwrap_or("cachemire"), options).map_err(|e| err(&e))?;
    io::copy(&mut File::open(file).map_err(|e| err(&e))?, &mut zip).map_err(|e| err(&e))?;
    zip.finish().map_err(|e| err(&e))?;
    Ok(())
}
