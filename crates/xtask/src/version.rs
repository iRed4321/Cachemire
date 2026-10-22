//! Release versions: worked out by the Release workflow from the latest `v<version>` tags and a
//! request (see `cargo ship`), then written into the checkout being built, never committed.

use semver::{Prerelease, Version};

use std::path::Path;

use crate::tools::{APP_MANIFEST, Result, git, repo_root};

/// The package's `version = "..."` line (the first one at the start of a line,
/// as dependency versions are indented or inline) and its value.
fn find_version(toml: &str) -> Result<(usize, Version)> {
    for (i, line) in toml.lines().enumerate() {
        let Some(rest) = line.strip_prefix("version = \"") else { continue };
        if let Ok(version) = Version::parse(rest.trim_end().trim_end_matches('"')) {
            return Ok((i, version));
        }
    }
    Err("no semver `version = \"x.y.z\"` line in the app's Cargo.toml".into())
}

/// The package version in `toml`, as "x.y.z" or "x.y.z-pre".
pub fn read_version(toml: &str) -> Result<String> {
    Ok(find_version(toml)?.1.to_string())
}

/// The MSI "x.y.z" or "x.y.z.n" of a version, `n` being the beta number (a stable release has none).
pub fn msi_version(version: &str) -> String {
    let numeric = version.split(['-', '+']).next().unwrap_or(version);
    match version.split('-').nth(1).and_then(|pre| pre.strip_prefix("beta.")) {
        Some(rest) => format!("{numeric}.{}", rest.split(['-', '+', '.']).next().unwrap_or("0")),
        None => numeric.to_string(),
    }
}

/// What to release, as the Release workflow is asked for it (`cargo xtask next-version`).
#[derive(Clone, PartialEq, Debug)]
pub enum Request {
    /// The next beta: the following one of those under way, else the first of the next minor.
    Beta,
    /// The first beta of the next minor or major, when no betas are under way.
    BetaMinor,
    BetaMajor,
    /// The betas under way, released as stable.
    Stable,
    /// The next minor or major released as stable at once, when no betas are under way.
    StableMinor,
    StableMajor,
    /// The latest stable release with its patch raised: a fix released from main.
    Fix,
    /// A stable version, checked to be one a stable release may take next.
    Exactly(Version),
}

impl Request {
    const NAMES: [(&str, Request); 7] = [
        ("beta", Request::Beta),
        ("beta-minor", Request::BetaMinor),
        ("beta-major", Request::BetaMajor),
        ("stable", Request::Stable),
        ("stable-minor", Request::StableMinor),
        ("stable-major", Request::StableMajor),
        ("fix", Request::Fix),
    ];

    /// The request named `name` (as a `release/<name>` branch carries it), or a version.
    pub fn parse(name: &str) -> Result<Self> {
        if let Some((_, request)) = Self::NAMES.iter().find(|(known, _)| *known == name) {
            return Ok(request.clone());
        }
        Version::parse(name).map(Self::Exactly).map_err(|_| {
            let names: Vec<&str> = Self::NAMES.iter().map(|(known, _)| *known).collect();
            format!("`{name}` is not a release request: {} or a version", names.join(", "))
        })
    }

    /// The request's name, as [`Request::parse`] reads it.
    pub fn name(&self) -> String {
        match self {
            Self::Exactly(version) => version.to_string(),
            _ => Self::NAMES.iter().find(|(_, request)| request == self).map_or_else(String::new, |(name, _)| name.to_string()),
        }
    }
}

/// The latest release of each kind, from the tags.
pub struct Latest {
    /// The highest version released, betas included.
    pub last: Version,
    /// The highest stable version released.
    pub stable: Option<Version>,
}

/// The version `request` releases after `latest`. Betas of one version at a time: while they're under
/// way, only the next beta or their stable release apply (and fixes of the stable, below their version).
fn target_version(latest: &Latest, request: &Request) -> Result<Version> {
    let last = &latest.last;
    let base = Version::new(last.major, last.minor, last.patch);
    let raised = |major: bool| if major { Version::new(last.major + 1, 0, 0) } else { Version::new(last.major, last.minor + 1, 0) };
    let beta = |mut version: Version, number: u64| -> Result<Version> {
        version.pre = Prerelease::new(&format!("beta.{number}")).map_err(|e| e.to_string())?;
        Ok(version)
    };
    let in_betas = !last.pre.is_empty();
    let under_way = || format!("the betas of {base} are under way ({last}): `beta` releases the next one, `stable` releases them");
    match request {
        Request::Beta | Request::BetaMinor | Request::BetaMajor if !in_betas => beta(raised(*request == Request::BetaMajor), 1),
        Request::Beta => {
            let number = last.pre.as_str().strip_prefix("beta.").and_then(|n| n.parse::<u64>().ok());
            beta(base.clone(), number.ok_or_else(|| format!("{last} is not a beta.<number> version"))? + 1)
        }
        Request::BetaMinor | Request::BetaMajor => Err(under_way()),
        Request::Stable if in_betas => Ok(base),
        Request::Stable => Err(format!("{last} is already stable, with no betas since: name the new version with --minor or --major")),
        Request::StableMinor | Request::StableMajor if in_betas => Err(under_way()),
        Request::StableMinor | Request::StableMajor => Ok(raised(*request == Request::StableMajor)),
        Request::Fix => {
            let stable = latest.stable.as_ref().ok_or("no stable release to fix")?;
            let fix = Version::new(stable.major, stable.minor, stable.patch + 1);
            if in_betas && fix >= base {
                return Err(format!("{fix}, the next fix of {stable}, would come after the betas under way ({last})"));
            }
            Ok(fix)
        }
        Request::Exactly(version) => {
            let allowed = if in_betas { vec![base] } else { vec![raised(false), raised(true)] };
            if !version.pre.is_empty() || !allowed.contains(version) {
                let names: Vec<String> = allowed.iter().map(Version::to_string).collect();
                return Err(format!("after {last}, the next stable release is {}, not {version}", names.join(" or ")));
            }
            Ok(version.clone())
        }
    }
}

/// `toml` with the package version line `line` set to `version`, keeping each line's own ending (LF or CRLF).
fn with_version(toml: &str, line: usize, version: &Version) -> String {
    toml.split_inclusive('\n')
        .enumerate()
        .map(|(i, text)| if i == line { format!("version = \"{version}\"{}", &text[text.trim_end().len()..]) } else { text.to_string() })
        .collect()
}

/// `lock` with the app's own `[[package]]` entry at `version`, keeping each line's own ending.
fn with_locked_version(lock: &str, version: &Version) -> Result<String> {
    let mut out = String::with_capacity(lock.len());
    let (mut in_app, mut found) = (false, false);
    for text in lock.split_inclusive('\n') {
        let line = text.trim_end();
        if in_app && line.starts_with("version = \"") {
            out += &format!("version = \"{version}\"{}", &text[line.len()..]);
            (in_app, found) = (false, true);
            continue;
        }
        in_app = line == "name = \"cachemire\"";
        out += text;
    }
    found.then_some(out).ok_or_else(|| "no cachemire package in Cargo.lock".into())
}

/// Writes `version` into the app's Cargo.toml and Cargo.lock, building nothing.
fn write_version(root: &Path, version: &Version) -> Result<()> {
    let read = |name: &str| std::fs::read_to_string(root.join(name)).map_err(|e| format!("reading {name}: {e}"));
    let write = |name: &str, text: String| std::fs::write(root.join(name), text).map_err(|e| format!("writing {name}: {e}"));
    let toml = read(APP_MANIFEST)?;
    let (line, _) = find_version(&toml)?;
    let lock = with_locked_version(&read("Cargo.lock")?, version)?;
    write(APP_MANIFEST, with_version(&toml, line, version))?;
    write("Cargo.lock", lock)
}

/// The app's Cargo.toml and Cargo.lock written at a release's version, put back as committed when dropped.
struct Stamped<'a> {
    root: &'a Path,
}

impl<'a> Stamped<'a> {
    fn new(root: &'a Path, version: &Version) -> Result<Self> {
        if !git(root, &["status", "--porcelain", "--", APP_MANIFEST, "Cargo.lock"])?.is_empty() {
            return Err(format!("{APP_MANIFEST} or Cargo.lock has uncommitted changes: commit or stash them first"));
        }
        write_version(root, version)?;
        println!("Building as version {version}.");
        Ok(Self { root })
    }
}

impl Drop for Stamped<'_> {
    fn drop(&mut self) {
        if let Err(e) = git(self.root, &["checkout", "--quiet", "--", APP_MANIFEST, "Cargo.lock"]) {
            eprintln!("could not put {APP_MANIFEST} and Cargo.lock back: {e}");
        }
    }
}

/// The highest version among the `v*` tags in `tags` (one per line), skipping betas when `stable`.
fn highest(tags: &str, stable: bool) -> Option<Version> {
    let versions = tags.lines().filter_map(|tag| Version::parse(tag.trim().strip_prefix('v')?).ok());
    versions.filter(|v| !stable || v.pre.is_empty()).max()
}

/// Puts HEAD back where it was (the branch, else the commit) when dropped.
struct Restore<'a> {
    root: &'a Path,
    origin: String,
}

impl Drop for Restore<'_> {
    fn drop(&mut self) {
        match git(self.root, &["checkout", "--quiet", &self.origin]) {
            Ok(_) => println!("Back on {}.", self.origin),
            Err(e) => eprintln!("could not go back to {}: {e}", self.origin),
        }
    }
}

/// Runs `build` on a release, with its version written into the app's Cargo.toml meanwhile. `as_version` builds the
/// working tree as that version (what the Release workflow does); otherwise the tag on HEAD if any, else the
/// latest tag (the latest stable one with `stable`), checked out meanwhile. `fast` builds the working tree as is.
pub fn in_release(fast: bool, stable: bool, as_version: Option<&str>, build: impl FnOnce() -> Result<()>) -> Result<()> {
    let root = repo_root();
    if let Some(version) = as_version {
        if fast || stable {
            return Err("--as-version builds the working tree in the release profile: it can't go with --fast or --stable".into());
        }
        let version = Version::parse(version).map_err(|e| format!("{version} is not a version: {e}"))?;
        let _stamped = Stamped::new(&root, &version)?;
        return build();
    }
    if fast {
        return if stable { Err("--fast builds the working tree: it can't go with --stable".into()) } else { build() };
    }
    if !git(&root, &["status", "--porcelain", "--untracked-files=no"])?.is_empty() {
        return Err("the working tree has uncommitted changes: commit or stash them first".into());
    }
    let at_head = highest(&git(&root, &["tag", "--points-at", "HEAD", "--list", "v*"])?, false);
    let version = match at_head.clone() {
        Some(version) if stable && !version.pre.is_empty() => {
            println!("HEAD is on the beta tag v{version}: nothing to build with --stable.");
            return Ok(());
        }
        Some(version) => version,
        None => highest(&git(&root, &["tag", "--list", "v*"])?, stable).ok_or("no release tag to build from")?,
    };
    let tag = format!("v{version}");
    if at_head.is_some() {
        println!("Building {tag}, the tag on HEAD.");
        let _stamped = Stamped::new(&root, &version)?;
        return build();
    }
    let origin = git(&root, &["symbolic-ref", "--quiet", "--short", "HEAD"]).or_else(|_| git(&root, &["rev-parse", "HEAD"]))?;
    println!("Checking out {tag}...");
    git(&root, &["checkout", "--quiet", &tag])?;
    // dropped in reverse: the files are put back before HEAD is
    let _restore = Restore { root: &root, origin };
    let _stamped = Stamped::new(&root, &version)?;
    build()
}

/// The latest releases of the repository, from its `v*` tags.
pub fn latest(root: &Path) -> Result<Latest> {
    let tags = git(root, &["tag", "--list", "v*"])?;
    let last = highest(&tags, false).ok_or("no release tag to start from (are the tags fetched?)")?;
    Ok(Latest { last, stable: highest(&tags, true) })
}

/// The version `request` releases, after the latest tags of the repository.
pub fn next_version(root: &Path, request: &Request) -> Result<Version> {
    target_version(&latest(root)?, request)
}

/// The version of try build `run` of what releases as `version`: just below it, and above the beta
/// before it (`3.2.0-beta.4` → `3.2.0-beta.3.try.<run>`, `3.1.1` → `3.1.1-try.<run>`).
fn try_version(version: &Version, run: u64) -> Result<Version> {
    let beta = version.pre.as_str().strip_prefix("beta.").and_then(|n| n.parse::<u64>().ok());
    let pre = match beta {
        Some(number) => format!("beta.{}.try.{run}", number.saturating_sub(1)),
        None => format!("try.{run}"),
    };
    let mut built = Version::new(version.major, version.minor, version.patch);
    built.pre = Prerelease::new(&pre).map_err(|e| e.to_string())?;
    Ok(built)
}

/// `cargo xtask next-version`: prints the version `request` releases, or that of try build `try_run` of it.
pub fn print_next_version(request: &str, try_run: Option<u64>) -> Result<()> {
    let version = next_version(&repo_root(), &Request::parse(request)?)?;
    println!("{}", try_run.map_or(Ok(version.clone()), |run| try_version(&version, run))?);
    Ok(())
}
