//! The core's errors. Each holds what went wrong as data, with the error it comes from as its
//! `source()`; `Display` is English, for the log. The app shows them in the interface language,
//! and [`Error::kind`] says what the UI can make of one (point at the SSH tunnel, say).

use std::path::PathBuf;

use thiserror::Error;

use crate::aws::AwsError;
use crate::connection::ssh_tunnel::TunnelError;
use crate::json_path;
use crate::redisql::QueryError;
use crate::secret_vault::{KeyringError, VaultError};

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Anything the core can fail at.
#[derive(Debug, Error)]
pub enum Error {
    #[error(transparent)]
    Invalid(#[from] Invalid),
    #[error(transparent)]
    NotFound(#[from] NotFound),
    #[error(transparent)]
    Timeout(#[from] Timeout),
    #[error(transparent)]
    Redis(#[from] redis::RedisError),
    /// boxed: it is the largest by far
    #[error(transparent)]
    Tunnel(Box<TunnelError>),
    #[error(transparent)]
    Keyring(#[from] KeyringError),
    #[error(transparent)]
    File(#[from] FileError),
    #[error(transparent)]
    Cloud(#[from] AwsError),
    #[error(transparent)]
    Query(#[from] QueryError),
    #[error(transparent)]
    JsonPath(#[from] json_path::ParseError),
    #[error(transparent)]
    System(#[from] SystemError),
    #[error(
        "This server runs in cluster mode, which Cachemire doesn't support yet. Connect to a server or endpoint with cluster mode disabled."
    )]
    ClusterMode,
}

/// What the user gave can't be used.
#[derive(Debug, Error)]
pub enum Invalid {
    #[error("Connection name is required.")]
    ConnectionName,
    #[error("Connection URL is required.")]
    ConnectionUrl,
    #[error("Couldn't understand the connection URL.")]
    UnreadableUrl,
    #[error("The connection URL has no host.")]
    UrlWithoutHost,
    #[error("SSH host is required.")]
    SshHost,
    #[error("SSH port must be a number between 1 and 65535.")]
    SshPort,
    #[error("Provide an SSH password or a private key.")]
    SshLogin,
    #[error("This SSH connection has no username or password: fill them in, or make an SSH profile active in Settings.")]
    SshCredentials,
    #[error("The profile needs a name.")]
    ProfileName,
    #[error("The profile needs an SSH username.")]
    ProfileUsername,
    #[error("The profile needs an SSH password.")]
    ProfilePassword,
    #[error("The query needs a name.")]
    QueryName,
    #[error("The query needs some text to save.")]
    QueryText,
    #[error("That search depth isn't one of the choices.")]
    SearchDepth,
    #[error("The key parameter is required.")]
    KeyRequired,
}

/// What was asked for isn't there.
#[derive(Debug, Error)]
pub enum NotFound {
    #[error("Connection not found.")]
    Connection,
    #[error("No active connection.")]
    ActiveConnection,
    #[error("SSH profile not found.")]
    SshProfile,
    #[error("Connection profile not found.")]
    ConnectionProfile,
    #[error("Saved query not found.")]
    SavedQuery,
    #[error("Key not found.")]
    Key,
}

/// Something took longer than it is allowed to.
#[derive(Debug, Error)]
pub enum Timeout {
    #[error("Connection timed out after {seconds} seconds.")]
    Connect { seconds: u64 },
}

/// A file couldn't be read or written.
#[derive(Debug, Error)]
pub enum FileError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("{} has no file name", .0.display())]
    NoFileName(PathBuf),
}

/// The operating system failed us otherwise.
#[derive(Debug, Error)]
pub enum SystemError {
    #[error("A background task failed: {0}")]
    Task(#[from] tokio::task::JoinError),
}

/// What an error means for the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    /// what the user typed can't be used (a required field left empty, a bad URL)
    Invalid,
    /// what was asked for isn't there (a connection, a key, a profile)
    NotFound,
    /// the SSH tunnel to the server couldn't be set up
    Tunnel,
    /// Redis refused the username or password
    Auth,
    /// the user's ACL doesn't allow a command
    Denied,
    /// the server couldn't be reached, or the connection dropped
    Network,
    /// something took longer than it is allowed to
    Timeout,
    /// Redis answered with any other error
    Redis,
    /// a Redisql query or a filter path couldn't be understood
    Parse,
    /// the system keyring couldn't be used
    Keyring,
    /// a file couldn't be read or written
    File,
    /// AWS refused or failed a request
    Cloud,
    /// the operating system failed us otherwise (the browser, a task)
    System,
    /// the server is set up in a way the app doesn't handle (cluster mode)
    Unsupported,
}

impl Error {
    pub fn kind(&self) -> ErrorKind {
        match self {
            Error::Invalid(_) => ErrorKind::Invalid,
            Error::NotFound(_) => ErrorKind::NotFound,
            Error::Timeout(_) => ErrorKind::Timeout,
            Error::Redis(error) => redis_kind(error),
            Error::Tunnel(_) => ErrorKind::Tunnel,
            Error::Keyring(_) => ErrorKind::Keyring,
            Error::File(_) => ErrorKind::File,
            Error::Cloud(error) => error.kind(),
            Error::Query(error) => error.kind(),
            Error::JsonPath(_) => ErrorKind::Parse,
            Error::System(_) => ErrorKind::System,
            Error::ClusterMode => ErrorKind::Unsupported,
        }
    }
}

fn redis_kind(error: &redis::RedisError) -> ErrorKind {
    use redis::{ErrorKind as RedisKind, ServerErrorKind};
    if error.is_timeout() {
        return ErrorKind::Timeout;
    }
    if error.is_io_error() || error.is_connection_refusal() || error.is_connection_dropped() {
        return ErrorKind::Network;
    }
    match (error.kind(), error.code()) {
        (RedisKind::AuthenticationFailed, _) | (_, Some("WRONGPASS" | "NOAUTH")) => ErrorKind::Auth,
        (RedisKind::Server(ServerErrorKind::NoPerm), _) | (_, Some("NOPERM")) => ErrorKind::Denied,
        _ => ErrorKind::Redis,
    }
}

impl From<tokio::task::JoinError> for Error {
    fn from(error: tokio::task::JoinError) -> Self {
        Error::System(SystemError::Task(error))
    }
}

impl From<TunnelError> for Error {
    fn from(error: TunnelError) -> Self {
        Error::Tunnel(Box::new(error))
    }
}

impl From<VaultError> for Error {
    fn from(error: VaultError) -> Self {
        Error::Keyring(KeyringError::Vault(error))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;

    fn kind_of(error: redis::RedisError) -> ErrorKind {
        Error::from(error).kind()
    }

    #[test]
    fn redis_errors_are_sorted_by_what_the_user_can_do_about_them() {
        let server = |code: &str| redis::make_extension_error(code.to_string(), Some("detail".into()));
        assert_eq!(kind_of(server("WRONGPASS")), ErrorKind::Auth);
        assert_eq!(kind_of(server("NOAUTH")), ErrorKind::Auth);
        assert_eq!(kind_of(redis::RedisError::from((redis::ErrorKind::AuthenticationFailed, "bad password"))), ErrorKind::Auth);
        assert_eq!(kind_of(server("NOPERM")), ErrorKind::Denied);
        assert_eq!(kind_of(server("WRONGTYPE")), ErrorKind::Redis);
    }

    #[test]
    fn network_failures_and_timeouts_are_told_apart() {
        let io_error = |kind: io::ErrorKind| redis::RedisError::from(io::Error::new(kind, "x"));
        assert_eq!(kind_of(io_error(io::ErrorKind::TimedOut)), ErrorKind::Timeout);
        assert_eq!(kind_of(io_error(io::ErrorKind::ConnectionRefused)), ErrorKind::Network);
        assert_eq!(kind_of(io_error(io::ErrorKind::ConnectionReset)), ErrorKind::Network);
        assert_eq!(kind_of(io_error(io::ErrorKind::BrokenPipe)), ErrorKind::Network);
    }
}
