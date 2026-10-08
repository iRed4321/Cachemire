//! `cargo xtask build-appimage`: the release build as a single-file AppImage in
//! `target/appimage/`, packed by `appimagetool` (downloaded into `target/tools/` if missing).

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::timings;
use crate::tools::{Result, cargo, repo_root, run, summary_packages, works, zip_file};
use crate::version::{in_release, read_version};

const TOOL_URL: &str = "https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-x86_64.AppImage";

pub fn run_task(fast: bool, stable: bool, with_zip: bool, timed: bool) -> Result<()> {
    if !cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        return Err("build-appimage only runs on x86_64 Linux".into());
    }
    in_release(fast, stable, || {
        compile(fast, timed)?;
        summary_packages(&[&package(fast, with_zip)?])
    })
}

/// The release (or `fast`) build of the binary, into `target/<profile>`; `timed` reports its timings.
pub fn compile(fast: bool, timed: bool) -> Result<()> {
    let root = repo_root();
    let profile = if fast { "fast" } else { "release" };
    let version = read_version(&std::fs::read_to_string(root.join("Cargo.toml")).map_err(|e| e.to_string())?)?;
    println!("Building Cachemire {version} ({profile})...");
    run(cargo().current_dir(&root).args(["build", "--profile", profile, "--locked"]).args(timed.then_some("--timings")))?;
    if timed {
        timings::publish(&format!("Cachemire {version} for Linux ({profile})"))?;
    }
    Ok(())
}

/// Packs the binary already built by [`compile`] as an AppImage, and returns its path.
pub fn package(fast: bool, with_zip: bool) -> Result<PathBuf> {
    let root = repo_root();
    let profile = if fast { "fast" } else { "release" };
    let version = read_version(&std::fs::read_to_string(root.join("Cargo.toml")).map_err(|e| e.to_string())?)?;
    let out = root.join(if fast { "target/appimage-fast" } else { "target/appimage" });
    let app_dir = out.join("Cachemire.AppDir");
    let _ = std::fs::remove_dir_all(&app_dir);
    stage(&root, &app_dir, profile)?;

    let output = out.join(format!("Cachemire-{version}-x86_64.AppImage"));
    let _ = std::fs::remove_file(&output);
    // extract-and-run: no FUSE needed to run appimagetool itself
    run(Command::new(appimagetool(&root)?).env("APPIMAGE_EXTRACT_AND_RUN", "1").env("ARCH", "x86_64").arg(&app_dir).arg(&output))?;
    println!("\nAppImage: {}", output.display());
    if with_zip {
        zip_file(&output, &output.with_extension("AppImage.zip"), 0o755)?;
    }
    Ok(output)
}

/// The AppDir layout: the binary, `AppRun`, and the desktop entry and icons at its root and in `usr/share`.
fn stage(root: &Path, app_dir: &Path, profile: &str) -> Result<()> {
    let put = |from: &str, to: &str, mode: u32| -> Result<()> {
        let to = app_dir.join(to);
        std::fs::create_dir_all(to.parent().expect("a file path")).map_err(|e| e.to_string())?;
        std::fs::copy(root.join(from), &to).map_err(|e| format!("{from}: {e}"))?;
        set_mode(&to, mode)
    };
    put(&format!("target/{profile}/cachemire"), "usr/bin/cachemire", 0o755)?;
    put("assets/cachemire.desktop", "cachemire.desktop", 0o644)?;
    put("assets/cachemire.desktop", "usr/share/applications/cachemire.desktop", 0o644)?;
    put("assets/icon.png", "cachemire.png", 0o644)?;
    put("assets/icon.png", "usr/share/icons/hicolor/256x256/apps/cachemire.png", 0o644)?;
    put("assets/icon.svg", "usr/share/icons/hicolor/scalable/apps/cachemire.svg", 0o644)?;
    symlink("usr/bin/cachemire", &app_dir.join("AppRun"))
}

/// `appimagetool` from PATH, else the copy kept in `target/tools/`, downloaded on first use.
fn appimagetool(root: &Path) -> Result<PathBuf> {
    if works("appimagetool", "--version") {
        return Ok("appimagetool".into());
    }
    let tool = root.join("target/tools/appimagetool-x86_64.AppImage");
    if !tool.is_file() {
        println!("appimagetool is not installed: downloading it (once)...");
        std::fs::create_dir_all(tool.parent().expect("a file path")).map_err(|e| e.to_string())?;
        run(Command::new("curl").args(["--fail", "--location", "--silent", "--show-error", "-o"]).arg(&tool).arg(TOOL_URL))?;
        set_mode(&tool, 0o755)?;
    }
    Ok(tool)
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> Result<()> {
    std::fs::set_permissions(path, std::os::unix::fs::PermissionsExt::from_mode(mode)).map_err(|e| e.to_string())
}

#[cfg(not(unix))]
fn set_mode(_: &Path, _: u32) -> Result<()> {
    Ok(())
}

#[cfg(unix)]
fn symlink(target: &str, link: &Path) -> Result<()> {
    std::os::unix::fs::symlink(target, link).map_err(|e| e.to_string())
}

#[cfg(not(unix))]
fn symlink(_: &str, _: &Path) -> Result<()> {
    Err("symlinks are only supported on Unix".into())
}
