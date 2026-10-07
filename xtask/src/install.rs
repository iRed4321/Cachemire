//! `cargo xtask install`: `cargo install` of the app, plus what the desktop looks up by app id: on Linux
//! the desktop entry and icons in ~/.local/share, on Windows a Start menu shortcut (`--skip-binary`: Linux files only).

use std::path::{Path, PathBuf};

use crate::tools::{Result, cargo, repo_root, run};

pub fn run_task(skip_binary: bool) -> Result<()> {
    if skip_binary && !cfg!(target_os = "linux") {
        return Err("--skip-binary only exists on Linux".into());
    }
    if !cfg!(any(target_os = "linux", windows)) {
        return Err("install only runs on Linux and Windows".into());
    }
    let root = repo_root();
    let cargo_bin = env_path("CARGO_HOME").unwrap_or(home()?.join(".cargo")).join("bin");
    let exe = cargo_bin.join(format!("cachemire{}", std::env::consts::EXE_SUFFIX));

    if !skip_binary {
        println!("Installing Cachemire (cargo install)...");
        run(cargo().current_dir(&root).args(["install", "--path", ".", "--locked", "--force"]))?;
    }
    if cfg!(windows) { start_menu_shortcut(&exe) } else { linux_desktop_files(&root, &exe, skip_binary) }
}

/// The desktop entry and icons under `$XDG_DATA_HOME` (~/.local/share), with `Exec` set to the installed binary.
fn linux_desktop_files(root: &Path, exe: &Path, skip_binary: bool) -> Result<()> {
    let data = env_path("XDG_DATA_HOME").unwrap_or(home()?.join(".local/share"));
    let mut desktop = std::fs::read_to_string(root.join("assets/cachemire.desktop")).map_err(|e| e.to_string())?;
    if !skip_binary {
        desktop = desktop.replace("Exec=cachemire", &format!("Exec={}", exe.display()));
    }

    write(&data.join("applications/cachemire.desktop"), desktop.as_bytes())?;
    copy(&root.join("assets/icon.svg"), &data.join("icons/hicolor/scalable/apps/cachemire.svg"))?;
    copy(&root.join("assets/icon.png"), &data.join("icons/hicolor/256x256/apps/cachemire.png"))?;
    // best effort: only some desktops need the icon cache refreshed
    let _ = std::process::Command::new("gtk-update-icon-cache").arg("-f").arg("-t").arg(data.join("icons/hicolor")).output();
    println!("\nDesktop entry and icons installed under {}", data.display());
    Ok(())
}

/// A "Cachemire" shortcut to `exe` in the current user's Start menu, made through PowerShell's WScript.Shell.
fn start_menu_shortcut(exe: &Path) -> Result<()> {
    let programs = env_path("APPDATA").ok_or("no %APPDATA% folder")?.join("Microsoft/Windows/Start Menu/Programs");
    let link = programs.join("Cachemire.lnk");
    let script = "$s = (New-Object -ComObject WScript.Shell).CreateShortcut($env:CACHEMIRE_LINK); \
                  $s.TargetPath = $env:CACHEMIRE_EXE; $s.IconLocation = $env:CACHEMIRE_EXE; $s.Save()";
    run(std::process::Command::new("powershell")
        .args(["-NoProfile", "-Command", script])
        .env("CACHEMIRE_LINK", &link)
        .env("CACHEMIRE_EXE", exe))?;
    println!("\nStart menu shortcut: {}", link.display());
    Ok(())
}

fn env_path(name: &str) -> Option<PathBuf> {
    std::env::var_os(name).map(PathBuf::from)
}

fn home() -> Result<PathBuf> {
    env_path("HOME").or_else(|| env_path("USERPROFILE")).ok_or_else(|| "no home folder".into())
}

fn copy(from: &Path, to: &Path) -> Result<()> {
    write(to, &std::fs::read(from).map_err(|e| format!("{}: {e}", from.display()))?)
}

fn write(path: &Path, content: &[u8]) -> Result<()> {
    std::fs::create_dir_all(path.parent().expect("a file path")).map_err(|e| e.to_string())?;
    std::fs::write(path, content).map_err(|e| format!("{}: {e}", path.display()))?;
    println!("Wrote {}", path.display());
    Ok(())
}
