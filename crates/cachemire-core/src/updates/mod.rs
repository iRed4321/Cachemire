//! Looking for a newer release of the app on GitHub and fetching its installer. The stable channel
//! looks among the stable releases, the beta channel among the betas too; a dev build looks for none.

use std::time::Duration;

use semver::Version;
use serde_json::Value;

/// The GitHub repository the releases live in.
pub const REPO: &str = "iRed4321/Cachemire";

/// What kind of releases a running version follows.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Channel {
    Dev,
    Beta,
    Stable,
}

impl Channel {
    pub fn of(version: &Version) -> Self {
        match version.pre.as_str() {
            "" => Self::Stable,
            "dev" => Self::Dev,
            _ => Self::Beta,
        }
    }

    /// The channel's name in the settings file.
    pub fn code(self) -> &'static str {
        match self {
            Self::Dev => "dev",
            Self::Beta => "beta",
            Self::Stable => "stable",
        }
    }

    pub fn from_code(code: &str) -> Option<Self> {
        [Self::Dev, Self::Beta, Self::Stable].into_iter().find(|channel| channel.code() == code)
    }

    /// Whether a release of `version` is offered on this channel: the beta channel takes the
    /// stable releases too.
    pub fn offers(self, version: &Version) -> bool {
        match self {
            Self::Stable => version.pre.is_empty(),
            Self::Beta => matches!(Self::of(version), Self::Beta | Self::Stable),
            Self::Dev => false,
        }
    }
}

/// A published release.
#[derive(Clone, Debug)]
pub struct Release {
    pub version: Version,
    /// The release's page on GitHub.
    pub page: String,
    /// The MSI installer attached to it, when it has one.
    pub installer: Option<String>,
}

pub fn repo_url() -> String {
    format!("https://github.com/{REPO}")
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(30))).user_agent("cachemire").build().into()
}

/// The newest release newer than `current` among those `channel` offers, if any.
pub fn newer_release(current: &Version, channel: Channel) -> Result<Option<Release>, String> {
    let url = format!("https://api.github.com/repos/{REPO}/releases?per_page=50");
    let body = agent().get(&url).header("Accept", "application/vnd.github+json").call().map_err(|e| e.to_string())?.body_mut().read_to_string().map_err(|e| e.to_string())?;
    let listed: Value = serde_json::from_str(&body).map_err(|e| e.to_string())?;
    Ok(newest_offered(listed.as_array().map(Vec::as_slice).unwrap_or_default(), current, channel))
}

/// The newest of GitHub's `releases` that `channel` offers and is newer than `current`.
fn newest_offered(releases: &[Value], current: &Version, channel: Channel) -> Option<Release> {
    releases.iter().filter_map(parse_release).filter(|r| channel.offers(&r.version) && r.version > *current).max_by(|a, b| a.version.cmp(&b.version))
}

fn parse_release(item: &Value) -> Option<Release> {
    if item["draft"].as_bool() == Some(true) {
        return None;
    }
    let version = Version::parse(item["tag_name"].as_str()?.trim_start_matches('v')).ok()?;
    let installer = item["assets"].as_array()?.iter().find(|a| a["name"].as_str().is_some_and(|n| n.ends_with(".msi"))).and_then(|a| a["browser_download_url"].as_str()).map(str::to_string);
    Some(Release { version, page: item["html_url"].as_str()?.to_string(), installer })
}

/// Downloads the installer to a temporary folder and returns its path.
#[cfg(windows)]
pub fn download_installer(url: &str) -> Result<std::path::PathBuf, String> {
    let dir = std::env::temp_dir().join("cachemire-update");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join(url.rsplit('/').next().filter(|n| !n.is_empty()).unwrap_or("Cachemire.msi"));
    let mut response = ureq::Agent::config_builder().timeout_connect(Some(Duration::from_secs(30))).user_agent("cachemire").build().new_agent().get(url).call().map_err(|e| e.to_string())?;
    let mut file = std::fs::File::create(&path).map_err(|e| e.to_string())?;
    std::io::copy(&mut response.body_mut().as_reader(), &mut file).map_err(|e| e.to_string())?;
    Ok(path)
}

/// Starts the Windows Installer on the MSI, detached from this process.
#[cfg(windows)]
pub fn run_installer(path: &std::path::Path) -> Result<(), String> {
    std::process::Command::new("msiexec").arg("/i").arg(path).spawn().map(drop).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests;
