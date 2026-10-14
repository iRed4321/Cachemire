use rust_i18n::t;

use super::error::AppError;
use super::redis_url;

/// How to reach the Redis server through an SSH server, when a connection
/// uses a tunnel: log in with a password, a private key, or both (the key is
/// tried first).
#[derive(Debug, Clone)]
pub struct SshSettings {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: String,
    /// path of a private key file (`~` stands for the home directory)
    pub private_key: String,
    pub passphrase: String,
}

/// Identifies a cloud endpoint across runs: the account and region it lives in
/// and its name (the replication group or cluster id). None of it is a secret.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloudKey {
    pub account: String,
    pub region: String,
    pub name: String,
}

impl CloudKey {
    /// How the endpoint is filed in the settings.
    pub fn endpoint_id(&self) -> String {
        format!("{}/{}/{}", self.account, self.region, self.name)
    }
}

#[derive(Debug, Clone)]
pub struct ConnectionItem {
    pub id: String,
    pub name: String,
    /// where the server is, which database, and TLS (`rediss://`): see `endpoint`
    pub connection_url: String,
    /// used for whatever the connection URL doesn't say itself
    pub username: String,
    pub password: String,
    /// `Some` when the connection goes through an SSH tunnel
    pub ssh: Option<SshSettings>,
    /// the connection profile (see Settings) it belongs to; "" for none
    pub profile_id: String,
}

/// Everything needed to reach a Redis server, as a connection's URL and its own
/// username/password fields say together.
#[derive(Debug, Clone, PartialEq)]
pub struct Endpoint {
    pub host: String,
    pub port: u16,
    pub db: i64,
    pub tls: bool,
    pub username: Option<String>,
    pub password: Option<String>,
}

impl ConnectionItem {
    /// The server the URL names; the URL's own user/password win over the
    /// connection's fields, which fill in what it leaves out.
    pub fn endpoint(&self) -> Result<Endpoint, AppError> {
        let non_empty = |value: String| if value.is_empty() { None } else { Some(value) };
        let parts = redis_url::parse(&self.connection_url)
            .filter(|parts| !parts.host.is_empty())
            .ok_or_else(|| AppError::invalid(t!("Couldn't understand the connection URL.")))?;
        Ok(Endpoint {
            host: parts.host,
            port: parts.port.filter(|p| *p > 0).unwrap_or(6379),
            db: parts.db.unwrap_or(0),
            tls: parts.tls,
            username: non_empty(parts.username).or_else(|| non_empty(self.username.trim().to_string())),
            password: non_empty(parts.password).or_else(|| non_empty(self.password.clone())),
        })
    }

    /// `host:port` of the server, or the URL as typed when it can't be read.
    pub fn address(&self) -> String {
        self.endpoint().map_or_else(|_| self.connection_url.clone(), |e| format!("{}:{}", e.host, e.port))
    }
}

#[derive(Debug, Clone)]
pub struct ConnectionsState {
    pub active_connection_id: String,
    pub connections: Vec<ConnectionItem>,
}

/// Input payload for add/update. All fields optional so the same type can
/// represent a partial update payload; a field left `None` keeps its current
/// value (the way a blank password box means "keep the stored one").
#[derive(Debug, Clone, Default)]
pub struct ConnectionPayloadInput {
    pub name: Option<String>,
    pub connection_url: Option<String>,
    pub username: Option<String>,
    pub password: Option<String>,
    pub ssh_enabled: Option<bool>,
    pub ssh_host: Option<String>,
    pub ssh_port: Option<u16>,
    pub ssh_username: Option<String>,
    pub ssh_password: Option<String>,
    pub ssh_private_key: Option<String>,
    pub ssh_passphrase: Option<String>,
    /// the connection profile to use; `Some("")` for none
    pub profile_id: Option<String>,
}

/// The tints a connection profile can take: a fixed set, so every one has been
/// checked against the app's background.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ProfileColor {
    #[default]
    Red,
    Orange,
    Yellow,
    Green,
    Cyan,
    Blue,
    Purple,
    Pink,
}

impl ProfileColor {
    /// How a color is saved.
    pub fn code(self) -> &'static str {
        match self {
            ProfileColor::Red => "red",
            ProfileColor::Orange => "orange",
            ProfileColor::Yellow => "yellow",
            ProfileColor::Green => "green",
            ProfileColor::Cyan => "cyan",
            ProfileColor::Blue => "blue",
            ProfileColor::Purple => "purple",
            ProfileColor::Pink => "pink",
        }
    }

    /// The color a saved code stands for; red for one we don't know (a file
    /// from a newer version, say).
    pub fn from_code(code: &str) -> ProfileColor {
        [
            ProfileColor::Red,
            ProfileColor::Orange,
            ProfileColor::Yellow,
            ProfileColor::Green,
            ProfileColor::Cyan,
            ProfileColor::Blue,
            ProfileColor::Purple,
            ProfileColor::Pink,
        ]
        .into_iter()
        .find(|color| color.code() == code)
        .unwrap_or_default()
    }
}

/// A connection profile: a name ("dev", "Production"...) and the tint the app's
/// background takes while a connection using it is the one in use.
#[derive(Debug, Clone, PartialEq)]
pub struct ConnectionProfile {
    pub id: String,
    pub name: String,
    pub color: ProfileColor,
}

/// A Redisql query saved from a query tab, under a name the user gave it.
#[derive(Debug, Clone, PartialEq)]
pub struct SavedQuery {
    pub id: String,
    pub name: String,
    pub text: String,
    /// the saved connection it belongs to; empty for a global query
    pub connection_id: String,
}
