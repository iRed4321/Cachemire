//! `cargo xtask build-deb`: a Debian package in `target/debian/`, laid out by the
//! `[package.metadata.deb]` table of `Cargo.toml` and built by `cargo-deb` (installed if missing).

use crate::tools::{Result, cargo, repo_root, run, works};
use crate::version::in_release;

pub fn run_task(fast: bool, stable: bool) -> Result<()> {
    if !cfg!(target_os = "linux") {
        return Err("build-deb only runs on Linux".into());
    }
    in_release(fast, stable, || build(fast))
}

fn build(fast: bool) -> Result<()> {
    if !works("cargo-deb", "--version") {
        println!("cargo-deb is not installed: installing it (once)...");
        run(cargo().args(["install", "cargo-deb", "--locked"]))?;
    }
    run(cargo().current_dir(repo_root()).args(["deb", "--locked", "--profile", if fast { "fast" } else { "release" }]))?;
    println!("\nPackage written under {}", repo_root().join("target/debian").display());
    Ok(())
}
