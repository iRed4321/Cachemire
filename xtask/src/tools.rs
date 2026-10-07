//! Helpers shared by the tasks: running commands and finding the repository.

use std::path::PathBuf;
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
