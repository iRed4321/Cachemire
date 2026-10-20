//! Looking for a newer release of the app on GitHub and fetching its installer. The stable channel
//! looks among the stable releases, the beta channel among the betas too; a dev build looks for none.

use std::time::Duration;

use semver::Version;
use serde_json::Value;

/// Why looking for a release, or fetching its installer, failed.
#[derive(Debug, thiserror::Error)]
pub enum UpdateError {
    #[error(transparent)]
    Http(#[from] ureq::Error),
    #[error("GitHub answered {status} to GET {url}{details}")]
    Status { status: u16, url: String, details: String },
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

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

const USER_AGENT: &str = "cachemire";

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(30))).user_agent(USER_AGENT).build().into()
}

/// GETs `url` as text; a non-2xx answer is an error carrying GitHub's message and rate-limit headers.
fn get_text(url: &str, accept: &str) -> Result<String, UpdateError> {
    let mut response = agent().get(url).header("Accept", accept).config().http_status_as_error(false).build().call()?;
    let status = response.status().as_u16();
    if (200..300).contains(&status) {
        return Ok(response.body_mut().read_to_string()?);
    }
    let headers = ["retry-after", "x-ratelimit-limit", "x-ratelimit-remaining", "x-ratelimit-reset", "x-ratelimit-resource"]
        .iter()
        .filter_map(|name| Some(format!("{name}={}", response.headers().get(*name)?.to_str().ok()?)))
        .collect::<Vec<_>>()
        .join(" ");
    let body = response.body_mut().read_to_string().unwrap_or_default();
    let body: String = body.chars().take(500).collect();
    Err(UpdateError::Status { status, url: url.to_string(), details: format!(" [{headers}] {body}") })
}

/// The newest release newer than `current` among those `channel` offers, if any. When the API
/// refuses (403/429, e.g. its per-IP rate limit), the releases feed is read instead.
pub fn newer_release(current: &Version, channel: Channel) -> Result<Option<Release>, UpdateError> {
    let url = format!("https://api.github.com/repos/{REPO}/releases?per_page=50");
    tracing::info!(%url, %current, channel = channel.code(), user_agent = USER_AGENT, "looking for a release");
    match get_text(&url, "application/vnd.github+json") {
        Ok(body) => {
            let listed: Value = serde_json::from_str(&body)?;
            Ok(newest_offered(listed.as_array().map(Vec::as_slice).unwrap_or_default(), current, channel))
        }
        Err(error @ UpdateError::Status { status: 403 | 429, .. }) => {
            tracing::warn!("the releases API refused, reading the releases feed instead: {error}");
            let feed = get_text(&format!("https://github.com/{REPO}/releases.atom"), "application/atom+xml")?;
            Ok(newest_offered(&releases_from_feed(&feed), current, channel))
        }
        Err(error) => Err(error),
    }
}

/// The releases of GitHub's Atom feed, shaped like the API's (the installer is named by convention).
fn releases_from_feed(feed: &str) -> Vec<Value> {
    let prefix = format!("https://github.com/{REPO}/releases/tag/");
    feed.split("href=\"")
        .skip(1)
        .filter_map(|rest| {
            let link = &rest[..rest.find('"')?];
            let tag = link.strip_prefix(&prefix)?;
            let version = tag.trim_start_matches('v');
            Some(serde_json::json!({
                "tag_name": tag,
                "html_url": link,
                "assets": [{
                    "name": format!("Cachemire-{version}-x64.msi"),
                    "browser_download_url": format!("https://github.com/{REPO}/releases/download/{tag}/Cachemire-{version}-x64.msi"),
                }],
            }))
        })
        .collect()
}

/// The newest of GitHub's `releases` that `channel` offers and is newer than `current`.
fn newest_offered(releases: &[Value], current: &Version, channel: Channel) -> Option<Release> {
    releases
        .iter()
        .filter_map(parse_release)
        .filter(|r| channel.offers(&r.version) && r.version > *current)
        .max_by(|a, b| a.version.cmp(&b.version))
}

fn parse_release(item: &Value) -> Option<Release> {
    if item["draft"].as_bool() == Some(true) {
        return None;
    }
    let version = Version::parse(item["tag_name"].as_str()?.trim_start_matches('v')).ok()?;
    let installer = item["assets"]
        .as_array()?
        .iter()
        .find(|a| a["name"].as_str().is_some_and(|n| n.ends_with(".msi")))
        .and_then(|a| a["browser_download_url"].as_str())
        .map(str::to_string);
    Some(Release { version, page: item["html_url"].as_str()?.to_string(), installer })
}

/// Downloads the installer to a temporary folder and returns its path.
#[cfg(windows)]
pub fn download_installer(url: &str) -> Result<std::path::PathBuf, UpdateError> {
    let dir = std::env::temp_dir().join("cachemire-update");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(url.rsplit('/').next().filter(|n| !n.is_empty()).unwrap_or("Cachemire.msi"));
    let mut response = ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(30)))
        .user_agent("cachemire")
        .build()
        .new_agent()
        .get(url)
        .call()?;
    let mut file = std::fs::File::create(&path)?;
    std::io::copy(&mut response.body_mut().as_reader(), &mut file)?;
    Ok(path)
}

/// Starts the Windows Installer on the MSI, detached from this process.
#[cfg(windows)]
pub fn run_installer(path: &std::path::Path) -> Result<(), UpdateError> {
    Ok(std::process::Command::new("msiexec").arg("/i").arg(path).spawn().map(drop)?)
}

#[cfg(test)]
mod tests;
