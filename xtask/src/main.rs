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
    /// Release HEAD: pushes it as the release/<request> branch, from which the Release workflow tags and publishes it
    Release {
        /// beta (the default) raises the beta number; stable drops the suffix; major, minor or fix start the next version as -beta.1
        #[arg(default_value = "beta")]
        part: version::Part,
        /// With major, minor or fix: release the new version at once, with no beta
        #[arg(long)]
        stable: bool,
    },
    /// Print the version a release/<request> branch releases, from the latest tag (what the Release workflow runs)
    NextVersion {
        /// The branch name without `release/`: a part of `release`, with `-stable` for its --stable
        request: String,
    },
    /// Release exe, MSI installer and portable zip
    BuildWindows {
        /// Use the `fast` profile (no LTO, quicker link) and build the working tree, not a release; output in target/windows-fast
        #[arg(long)]
        fast: bool,
        /// Build the latest stable release instead of the latest one
        #[arg(long)]
        stable: bool,
        /// Build the working tree in the release profile as this version, as the Release workflow does
        #[arg(long, value_name = "VERSION", conflicts_with_all = ["fast", "stable"])]
        as_version: Option<String>,
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
        /// Build the working tree in the release profile as this version, as the Release workflow does
        #[arg(long, value_name = "VERSION", conflicts_with_all = ["fast", "stable"])]
        as_version: Option<String>,
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
        /// Build the working tree in the release profile as this version, as the Release workflow does
        #[arg(long, value_name = "VERSION", conflicts_with_all = ["fast", "stable"])]
        as_version: Option<String>,
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
        /// Build the working tree in the release profile as this version, as the Release workflow does
        #[arg(long, value_name = "VERSION", conflicts_with_all = ["fast", "stable"])]
        as_version: Option<String>,
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
        Task::Release { part, stable } => version::request_release(part, stable),
        Task::NextVersion { request } => version::print_next_version(&request),
        Task::BuildWindows { fast, stable, as_version, clean, skip_installer, with_zip, reported } => {
            windows::run_task(fast, stable, as_version.as_deref(), clean, skip_installer, with_zip, reported)
        }
        Task::BuildAppimage { fast, stable, as_version, with_zip, reported } => appimage::run_task(fast, stable, as_version.as_deref(), with_zip, reported),
        Task::BuildDeb { fast, stable, as_version, with_zip } => deb::run_task(fast, stable, as_version.as_deref(), with_zip),
        Task::BuildLinux { fast, stable, as_version, with_zip, reported } => linux::run_task(fast, stable, as_version.as_deref(), with_zip, reported),
        Task::ReportBuild { step } => report::save(&step, &[]),
        Task::Report { title, dir } => report::run_task(&title, dir),
        Task::Install { skip_binary } => install::run_task(skip_binary),
    };
    if let Err(e) = result {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}
