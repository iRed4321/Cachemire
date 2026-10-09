//! Development tasks, run as `cargo xtask <task>` (see `cargo xtask --help`): the
//! translation files, releases and the Windows and Linux packages. Pure Rust, so
//! they run the same on every platform; the external tools they need are installed on demand.

mod appimage;
mod deb;
mod i18n;
mod install;
mod linux;
mod report;
mod tools;
mod version;
mod windows;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(bin_name = "cargo xtask", about = "Development tasks for Cachemire")]
struct Cli {
    #[command(subcommand)]
    task: Task,
}

#[derive(Subcommand)]
enum Task {
    /// Extract the UI texts and merge them into each language's .po
    I18n,
    /// Release the next version after the latest tag (commit and tag), then restore the 0.0.0-dev placeholder
    Bump {
        /// major, minor or fix start the next version as -beta.1; beta (the default) raises the beta number; stable drops the suffix
        #[arg(default_value = "beta")]
        part: version::Part,
        /// With major, minor or fix: release the new version directly, with no beta
        #[arg(long)]
        stable: bool,
        /// Only edit Cargo.toml and Cargo.lock: no commit, no tag
        #[arg(long)]
        no_tag: bool,
    },
    /// Release exe, MSI installer and portable zip
    BuildWindows {
        /// Use the `fast` profile (no LTO, quicker link) and build the working tree, not a release; output in target/windows-fast
        #[arg(long)]
        fast: bool,
        /// Build the latest stable release instead of the latest one
        #[arg(long)]
        stable: bool,
        /// Run `cargo clean` first
        #[arg(long)]
        clean: bool,
        /// Skip the MSI installer
        #[arg(long)]
        skip_installer: bool,
        /// Also zip the package (the installer, .deb or AppImage)
        #[arg(long)]
        with_zip: bool,
        /// Build with `cargo --timings` and save it, with the packages, as the "Release build" step of the CI report (see `report`)
        #[arg(long = "report")]
        reported: bool,
    },
    /// Release build packed as a single-file AppImage in target/appimage
    BuildAppimage {
        /// Use the `fast` profile (no LTO, quicker link) and build the working tree, not a release; output in target/appimage-fast
        #[arg(long)]
        fast: bool,
        /// Build the latest stable release instead of the latest one
        #[arg(long)]
        stable: bool,
        /// Also zip the package (the installer, .deb or AppImage)
        #[arg(long)]
        with_zip: bool,
        /// Build with `cargo --timings` and save it, with the packages, as the "Release build" step of the CI report (see `report`)
        #[arg(long = "report")]
        reported: bool,
    },
    /// Release build packaged as a .deb in target/debian (needs cargo-deb)
    BuildDeb {
        /// Use the `fast` profile (no LTO, quicker link) and build the working tree, not a release
        #[arg(long)]
        fast: bool,
        /// Build the latest stable release instead of the latest one
        #[arg(long)]
        stable: bool,
        /// Also zip the package (the installer, .deb or AppImage)
        #[arg(long)]
        with_zip: bool,
    },
    /// The .deb and the AppImage from a single release build (needs cargo-deb)
    BuildLinux {
        /// Use the `fast` profile (no LTO, quicker link) and build the working tree, not a release
        #[arg(long)]
        fast: bool,
        /// Build the latest stable release instead of the latest one
        #[arg(long)]
        stable: bool,
        /// Also zip the package (the installer, .deb or AppImage)
        #[arg(long)]
        with_zip: bool,
        /// Build with `cargo --timings` and save it, with the packages, as the "Release build" step of the CI report (see `report`)
        #[arg(long = "report")]
        reported: bool,
    },
    /// Save what the last `cargo … --timings` build compiled as one step of the CI report, in target/reports
    ReportBuild {
        /// The step's name in the report (e.g. "Tests")
        step: String,
    },
    /// Merge the saved reports into one summary comparing the systems, also the GitHub job summary
    Report {
        /// The summary's heading
        title: String,
        /// Where the saved reports are (searched recursively), target/reports by default
        #[arg(long)]
        dir: Option<std::path::PathBuf>,
    },
    /// Cargo install plus the desktop entry and icons (Linux) or the Start menu shortcut (Windows)
    Install {
        /// Linux only: install just the desktop entry and icons
        #[arg(long)]
        skip_binary: bool,
    },
}

fn main() {
    let result = match Cli::parse().task {
        Task::I18n => i18n::run_task(),
        Task::Bump { part, stable, no_tag } => version::run_task(part, stable, no_tag),
        Task::BuildWindows { fast, stable, clean, skip_installer, with_zip, reported } => windows::run_task(fast, stable, clean, skip_installer, with_zip, reported),
        Task::BuildAppimage { fast, stable, with_zip, reported } => appimage::run_task(fast, stable, with_zip, reported),
        Task::BuildDeb { fast, stable, with_zip } => deb::run_task(fast, stable, with_zip),
        Task::BuildLinux { fast, stable, with_zip, reported } => linux::run_task(fast, stable, with_zip, reported),
        Task::ReportBuild { step } => report::save(&step, &[]),
        Task::Report { title, dir } => report::run_task(&title, dir),
        Task::Install { skip_binary } => install::run_task(skip_binary),
    };
    if let Err(e) = result {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}
