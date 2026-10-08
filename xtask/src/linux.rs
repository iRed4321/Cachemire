//! `cargo xtask build-linux`: the .deb and the AppImage from a single release build
//! (see `build-deb` and `build-appimage`), in `target/debian/` and `target/appimage/`.

use crate::tools::{Result, summary_packages};
use crate::version::in_release;
use crate::{appimage, deb};

pub fn run_task(fast: bool, stable: bool, with_zip: bool, timed: bool) -> Result<()> {
    if !cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        return Err("build-linux only runs on x86_64 Linux".into());
    }
    in_release(fast, stable, || {
        appimage::compile(fast, timed)?;
        let appimage = appimage::package(fast, with_zip)?;
        summary_packages(&[&appimage, &deb::build(fast, with_zip, true)?])
    })
}
