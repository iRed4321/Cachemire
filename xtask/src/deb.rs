//! `cargo xtask build-deb`: a Debian package in `target/debian/`, laid out by the
//! `[package.metadata.deb]` table of `Cargo.toml` and built by `cargo-deb` (installed if missing).

use crate::tools::{Result, cargo, repo_root, run, works, zip_file};
use crate::version::in_release;

pub fn run_task(fast: bool, stable: bool, with_zip: bool) -> Result<()> {
    if !cfg!(target_os = "linux") {
        return Err("build-deb only runs on Linux".into());
    }
    in_release(fast, stable, || build(fast, with_zip, false))
}

/// Packages the .deb; with `no_build`, from the binary already in `target/<profile>`.
pub fn build(fast: bool, with_zip: bool, no_build: bool) -> Result<()> {
    if !works("cargo-deb", "--version") {
        println!("cargo-deb is not installed: installing it (once)...");
        run(cargo().args(["install", "cargo-deb", "--locked"]))?;
    }
    let out = repo_root().join("target/debian");
    let mut command = cargo();
    command.current_dir(repo_root()).args(["deb", "--locked", "--profile", if fast { "fast" } else { "release" }]);
    if no_build {
        command.arg("--no-build");
    }
    run(&mut command)?;
    println!("\nPackage written under {}", out.display());
    if with_zip {
        // the package cargo-deb just wrote: the newest .deb of the folder
        let newest = std::fs::read_dir(&out)
            .map_err(|e| e.to_string())?
            .filter_map(|entry| entry.ok().map(|e| e.path()))
            .filter(|path| path.extension().is_some_and(|ext| ext == "deb"))
            .max_by_key(|path| std::fs::metadata(path).and_then(|m| m.modified()).ok())
            .ok_or("no .deb package found")?;
        zip_file(&newest, &newest.with_extension("deb.zip"), 0o644)?;
    }
    Ok(())
}
