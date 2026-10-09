//! `cargo xtask build-windows`: the release executable, an MSI installer (WiX v6 or v7) and
//! a portable zip (and a zip of the MSI on request), in target/windows. Built from the latest release tag.

use std::path::PathBuf;
use std::process::Command;

use crate::report;
use crate::tools::{Result, cargo, repo_root, run, works, zip_file};
use crate::version::{in_release, msi_version, read_version};

pub fn run_task(fast: bool, stable: bool, clean: bool, skip_installer: bool, with_zip: bool, reported: bool) -> Result<()> {
    in_release(fast, stable, || build(fast, clean, skip_installer, with_zip, reported))
}

fn build(fast: bool, clean: bool, skip_installer: bool, with_zip: bool, reported: bool) -> Result<()> {
    let root = repo_root();
    let profile = if fast { "fast" } else { "release" };
    let release = root.join("target").join(profile);
    let exe = release.join(format!("cachemire{}", std::env::consts::EXE_SUFFIX));
    let out = root.join(if fast { "target/windows-fast" } else { "target/windows" });

    if clean {
        run(cargo().current_dir(&root).arg("clean"))?;
    }
    let version = read_version(&std::fs::read_to_string(root.join("Cargo.toml")).map_err(|e| e.to_string())?)?;
    println!("Building Cachemire {version} ({profile})...");
    run(cargo().current_dir(&root).args(["build", "--profile", profile, "--locked"]).args(reported.then_some("--timings")))?;
    if !exe.is_file() {
        return Err(format!("executable not found: {}", exe.display()));
    }

    std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
    let zip_path = out.join(format!("Cachemire-Portable-{version}-x64.zip"));
    zip_file(&exe, &zip_path, 0o755)?;

    let mut msi = None;
    if !skip_installer {
        let wix = wix_command()?;
        let ui_extension = ensure_ui_extension(&wix)?;
        let path = out.join(format!("Cachemire-{version}-x64.msi"));
        let _ = std::fs::remove_file(&path);
        // -b: base path of the .wxs's relative SourceFile references (the icon)
        run(Command::new(wix)
            .args(["build", "-arch", "x64", "-ext"])
            .arg(&ui_extension)
            .arg("-b")
            .arg(&root)
            .arg("-d")
            .arg(format!("Version={}", msi_version(&version)))
            .arg("-d")
            .arg(format!("FullVersion={version}"))
            .arg("-d")
            .arg(format!("ExePath={}", exe.display()))
            .arg("-o")
            .arg(&path)
            .arg(root.join("assets/cachemire.wxs")))?;
        if with_zip {
            zip_file(&path, &path.with_extension("msi.zip"), 0o644)?;
        }
        msi = Some(path);
    }

    println!("\nExecutable: {}\nPortable:   {}", exe.display(), zip_path.display());
    if let Some(msi) = &msi {
        println!("Installer:  {}", msi.display());
        if with_zip {
            println!("MSI zip:    {}", msi.with_extension("msi.zip").display());
        }
    }
    if reported {
        report::save("Release build", &[Some(zip_path.as_path()), msi.as_deref()].into_iter().flatten().collect::<Vec<_>>())?;
    }
    Ok(())
}

/// The WiX CLI: an installed one, else the .NET global tool, installed if missing
/// (it needs the .NET SDK, which can't be installed from here).
fn wix_command() -> Result<PathBuf> {
    if cfg!(not(windows)) {
        return Err("the MSI can only be built on Windows (pass --skip-installer for the exe and zip)".into());
    }
    // Several WiX versions can be installed side by side: take the newest Program Files one
    // (WiX 7 needs its OSMF EULA accepted once: `wix eula accept wix7`)
    let program_files = std::env::var_os("ProgramFiles").map(PathBuf::from).unwrap_or_else(|| "C:/Program Files".into());
    for dir in ["WiX Toolset v7.0", "WiX Toolset v6.0"] {
        let installed = program_files.join(dir).join("bin/wix.exe");
        if installed.is_file() {
            return Ok(installed);
        }
    }
    if works("wix", "--version") {
        return Ok("wix".into());
    }
    let tool = dirs_home()?.join(".dotnet/tools/wix.exe");
    if !tool.is_file() {
        if !works("dotnet", "--version") {
            return Err("the .NET SDK is needed to install WiX: `winget install Microsoft.DotNet.SDK.9`, then run this again".into());
        }
        println!("WiX is not installed: installing it (once)...");
        run(Command::new("dotnet").args(["tool", "install", "--global", "wix"]))?;
    }
    Ok(tool)
}

/// The WiX UI extension (the installer's dialogs) at the CLI's own version, as the `-ext` argument.
/// Several WiX versions can coexist (v6 and v7 installs), each with its own global extension cache
/// entries, so the version is pinned; a damaged entry for it is removed and reinstalled.
fn ensure_ui_extension(wix: &PathBuf) -> Result<String> {
    let output = |args: &[&str]| Command::new(wix).args(args).output().map_err(|e| format!("could not run wix: {e}"));
    let version = String::from_utf8_lossy(&output(&["--version"])?.stdout).trim().split('+').next().unwrap_or_default().to_string();
    let id = format!("WixToolset.UI.wixext/{version}");
    let listed = String::from_utf8_lossy(&output(&["extension", "list", "--global"])?.stdout).into_owned();
    let entry = listed.lines().find(|l| l.split_whitespace().take(2).eq(["WixToolset.UI.wixext", version.as_str()]));
    match entry {
        Some(l) if !l.contains("damaged") => return Ok(id),
        Some(_) => {
            println!("The WiX UI extension {version} is damaged: reinstalling it...");
            run(Command::new(wix).args(["extension", "remove", "--global"]).arg(&id))?;
        }
        None => println!("The WiX UI extension is not installed: installing it (once)..."),
    }
    run(Command::new(wix).args(["extension", "add", "--global"]).arg(&id))?;
    Ok(id)
}

fn dirs_home() -> Result<PathBuf> {
    std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(PathBuf::from).ok_or_else(|| "no home folder".to_string())
}
