//! Shareable links to a key: `cachemire://open?v=1&redis=host:6379&db=2&key=user:42&field=f`.
//! They name the server by what reaches it (never a login or a password), then the key, a
//! hash's field and a filter. Unknown parameters are ignored, so newer links still open.

use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, percent_decode_str, percent_encode};

use crate::connection::{CloudKey, ConnectionItem, SshSettings};
use crate::key_name::KeyName;

pub const SCHEME: &str = "cachemire";
/// The format version this one writes and reads; a link of a newer one is refused.
pub const VERSION: u32 = 1;
const ACTION_OPEN: &str = "open";

/// What a value keeps as it is: the unreserved characters, and those that read well in a key.
const VALUE: &AsciiSet =
    &NON_ALPHANUMERIC.remove(b'-').remove(b'.').remove(b'_').remove(b'~').remove(b':').remove(b'/').remove(b'@').remove(b'$');

/// A server's host and port, as a link writes it (`host:port`, `[v6]:port`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Address {
    pub host: String,
    pub port: u16,
}

impl Address {
    /// Reads `host`, `host:port` or `[v6]:port`; `port` when it has none.
    fn read(text: &str, port: u16) -> Option<Self> {
        let (host, given) = match text.strip_prefix('[') {
            Some(v6) => match v6.split_once(']')? {
                (host, "") => (host, None),
                (host, rest) => (host, Some(rest.strip_prefix(':')?)),
            },
            None => match text.split_once(':') {
                Some((host, port)) => (host, Some(port)),
                None => (text, None),
            },
        };
        let port = match given {
            None => port,
            Some(given) => given.parse().ok().filter(|p| *p > 0)?,
        };
        (!host.is_empty()).then(|| Self { host: host.to_string(), port })
    }

    fn write(&self) -> String {
        if self.host.contains(':') { format!("[{}]:{}", self.host, self.port) } else { format!("{}:{}", self.host, self.port) }
    }

    /// Whether `host`/`port` is this server: hosts compare ignoring case, loopback names as one.
    fn is(&self, host: &str, port: u16) -> bool {
        self.port == port && same_host(&self.host, host)
    }
}

fn is_loopback(host: &str) -> bool {
    let host = host.trim_start_matches('[').trim_end_matches(']');
    host.eq_ignore_ascii_case("localhost") || host == "::1" || host.starts_with("127.")
}

fn same_host(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b) || (is_loopback(a) && is_loopback(b))
}

/// The server a link names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Server {
    /// a Redis server by its address, reached directly or through the SSH server `via`
    Redis { address: Address, tls: bool, via: Option<Address> },
    /// an ElastiCache endpoint, as [`CloudKey`] names it
    Aws { account: String, region: String, name: String },
}

/// What a link opens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    pub server: Server,
    pub db: i64,
    /// what the server is called where the link was made: shown, never matched on
    pub name: Option<String>,
    pub key: KeyName,
    /// a hash's field, opened and scrolled to
    pub field: Option<String>,
    /// the key panel's filter (a JSON path)
    pub path: Option<String>,
}

/// Why a text isn't a link this version can open.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LinkError {
    #[error("not a cachemire:// link")]
    NotALink,
    #[error("made by a newer version (format {0})")]
    Newer(u32),
    #[error("unknown action {0:?}")]
    UnknownAction(String),
    #[error("names no server this version knows")]
    NoServer,
    #[error("names two servers")]
    TwoServers,
    #[error("names no key")]
    NoKey,
    #[error("its {0:?} can't be read")]
    Unreadable(&'static str),
}

/// A link's parameters, decoded; the first of a name counts.
struct Params(Vec<(String, Vec<u8>)>);

impl Params {
    fn read(query: &str) -> Self {
        let decode = |text: &str| percent_decode_str(text).collect::<Vec<u8>>();
        let pairs = query
            .split('&')
            .filter(|pair| !pair.is_empty())
            .map(|pair| pair.split_once('=').unwrap_or((pair, "")))
            .map(|(name, value)| (String::from_utf8_lossy(&decode(name)).to_ascii_lowercase(), decode(value)))
            .collect();
        Self(pairs)
    }

    fn bytes(&self, name: &str) -> Option<&[u8]> {
        self.0.iter().find(|(n, _)| n == name).map(|(_, value)| value.as_slice())
    }

    /// The parameter as text: an error if it isn't UTF-8.
    fn text(&self, name: &'static str) -> Result<Option<&str>, LinkError> {
        self.bytes(name).map(|bytes| std::str::from_utf8(bytes).map_err(|_| LinkError::Unreadable(name))).transpose()
    }

    /// The parameter as text, `None` for an empty one.
    fn non_empty(&self, name: &'static str) -> Result<Option<String>, LinkError> {
        Ok(self.text(name)?.filter(|text| !text.is_empty()).map(str::to_string))
    }
}

impl Link {
    pub fn parse(text: &str) -> Result<Self, LinkError> {
        let text = text.trim();
        let (scheme, rest) = text.split_once(':').ok_or(LinkError::NotALink)?;
        if !scheme.eq_ignore_ascii_case(SCHEME) {
            return Err(LinkError::NotALink);
        }
        let rest = rest.strip_prefix("//").unwrap_or(rest);
        let rest = rest.split_once('#').map_or(rest, |(rest, _)| rest);
        let (action, query) = rest.split_once('?').unwrap_or((rest, ""));
        let action = action.trim_end_matches('/');
        if !action.eq_ignore_ascii_case(ACTION_OPEN) {
            return Err(LinkError::UnknownAction(action.to_string()));
        }
        let params = Params::read(query);
        if let Some(version) = params.text("v")? {
            let version: u32 = version.parse().map_err(|_| LinkError::Unreadable("v"))?;
            if version > VERSION {
                return Err(LinkError::Newer(version));
            }
        }

        let server = match (params.text("redis")?, params.text("aws")?) {
            (Some(_), Some(_)) => return Err(LinkError::TwoServers),
            (Some(redis), None) => Server::Redis {
                address: Address::read(redis, 6379).ok_or(LinkError::Unreadable("redis"))?,
                tls: match params.text("tls")? {
                    None | Some("" | "0" | "false") => false,
                    Some("1" | "true") => true,
                    Some(_) => return Err(LinkError::Unreadable("tls")),
                },
                via: params.text("via")?.map(|via| Address::read(via, 22).ok_or(LinkError::Unreadable("via"))).transpose()?,
            },
            (None, Some(aws)) => match aws.split('/').collect::<Vec<_>>()[..] {
                [account, region, name] if !account.is_empty() && !region.is_empty() && !name.is_empty() => {
                    Server::Aws { account: account.to_string(), region: region.to_string(), name: name.to_string() }
                }
                _ => return Err(LinkError::Unreadable("aws")),
            },
            (None, None) => return Err(LinkError::NoServer),
        };
        let db = match params.text("db")? {
            None | Some("") => 0,
            Some(db) => db.parse::<i64>().ok().filter(|db| *db >= 0).ok_or(LinkError::Unreadable("db"))?,
        };
        // any bytes: a key's name needn't be UTF-8
        let key = params.bytes("key").filter(|key| !key.is_empty()).ok_or(LinkError::NoKey)?;
        Ok(Self {
            server,
            db,
            name: params.non_empty("name")?,
            key: KeyName::from(key.to_vec()),
            field: params.non_empty("field")?,
            path: params.non_empty("path")?,
        })
    }

    /// The link as text, its parameters always in the same order.
    pub fn to_uri(&self) -> String {
        let mut uri = format!("{SCHEME}://{ACTION_OPEN}?v={VERSION}");
        let mut push = |name: &str, value: &[u8]| {
            uri.push('&');
            uri.push_str(name);
            uri.push('=');
            uri.extend(percent_encode(value, VALUE));
        };
        match &self.server {
            Server::Redis { address, tls, via } => {
                push("redis", address.write().as_bytes());
                if *tls {
                    push("tls", b"1");
                }
                if let Some(via) = via {
                    push("via", via.write().as_bytes());
                }
            }
            Server::Aws { account, region, name } => push("aws", format!("{account}/{region}/{name}").as_bytes()),
        }
        if self.db != 0 {
            push("db", self.db.to_string().as_bytes());
        }
        if let Some(name) = &self.name {
            push("name", name.as_bytes());
        }
        push("key", self.key.as_bytes());
        if let Some(field) = &self.field {
            push("field", field.as_bytes());
        }
        if let Some(path) = &self.path {
            push("path", path.as_bytes());
        }
        uri
    }

    /// The link to `key` of `connection`, a cloud endpoint's when `cloud` says where it lives;
    /// `None` when its address can't be read. Its login and passwords are left out.
    pub fn to_key(connection: &ConnectionItem, cloud: Option<&CloudKey>, key: KeyName) -> Option<Self> {
        let endpoint = connection.endpoint().ok()?;
        let named = |name: &str| Some(name.to_string()).filter(|name| !name.is_empty());
        let (server, name) = match cloud {
            Some(cloud) => (
                Server::Aws { account: cloud.account.clone(), region: cloud.region.clone(), name: cloud.name.clone() },
                named(&connection.name).filter(|name| *name != cloud.name),
            ),
            None => (
                Server::Redis {
                    address: Address { host: endpoint.host, port: endpoint.port },
                    tls: endpoint.tls,
                    via: connection.ssh.as_ref().map(|ssh| Address { host: ssh.host.clone(), port: ssh.port }),
                },
                named(&connection.name),
            ),
        };
        Some(Self { server, db: endpoint.db, name, key, field: None, path: None })
    }

    /// The URL of a new connection to the server, with no login: `None` for a cloud endpoint.
    pub fn connection_url(&self) -> Option<String> {
        let Server::Redis { address, tls, .. } = &self.server else { return None };
        let scheme = if *tls { "rediss" } else { "redis" };
        let db = if self.db == 0 { String::new() } else { format!("/{}", self.db) };
        Some(format!("{scheme}://{}{db}", address.write()))
    }

    /// The server as a person reads it: `host:port` (db N), or the endpoint and its account.
    pub fn server_label(&self) -> String {
        let server = match &self.server {
            Server::Redis { address, .. } => address.write(),
            Server::Aws { account, region, name } => format!("{name} ({account}, {region})"),
        };
        if self.db == 0 { server } else { format!("{server} (db {})", self.db) }
    }

    /// The saved connection that reaches the link's server and database: preferably through
    /// the same SSH server, then the one of the link's name, then `current`, then the first.
    pub fn find_saved<'a>(&self, connections: &'a [ConnectionItem], current: &str) -> Option<&'a ConnectionItem> {
        let Server::Redis { address, via, .. } = &self.server else { return None };
        let through_via = |ssh: Option<&SshSettings>| match (via, ssh) {
            (None, None) => true,
            (Some(via), Some(ssh)) => via.is(&ssh.host, ssh.port),
            _ => false,
        };
        // a loopback address only means the same server through the same tunnel
        let needs_via = via.is_some() && is_loopback(&address.host);
        connections
            .iter()
            .enumerate()
            .filter(|(_, c)| c.endpoint().is_ok_and(|e| address.is(&e.host, e.port) && e.db == self.db))
            .filter(|(_, c)| !needs_via || through_via(c.ssh.as_ref()))
            .max_by_key(|(i, c)| {
                let named = self.name.as_deref().is_some_and(|name| name == c.name);
                (through_via(c.ssh.as_ref()), named, c.id == current, std::cmp::Reverse(*i))
            })
            .map(|(_, c)| c)
    }
}

#[cfg(test)]
mod tests;
