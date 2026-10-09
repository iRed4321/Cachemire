//! `cargo xtask build-linux`: the .deb and the AppImage from a single release build
//! (see `build-deb` and `build-appimage`), in `target/debian/` and `target/appimage/`.

use crate::tools::Result;
use crate::version::in_release;
use crate::{appimage, deb, report};

pub fn run_task(fast: bool, stable: bool, with_zip: bool, reported: bool) -> Result<()> {
    if !cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        return Err("build-linux only runs on x86_64 Linux".into());
    }
    in_release(fast, stable, || {
        appimage::compile(fast, reported)?;
        let appimage = appimage::package(fast, with_zip)?;
        let deb = deb::build(fast, with_zip, true)?;
        if reported {
            report::save("Release build", &[&appimage, &deb])?;
        }
        Ok(())
    })
}
