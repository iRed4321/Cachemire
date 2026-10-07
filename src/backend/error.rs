use rust_i18n::t;

/// What went wrong, by kind, so the UI can react to each one (point at the SSH
/// tunnel, suggest checking a password); `Display` gives the message to show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppError {
    /// what the user typed can't be used (a required field left empty, a bad URL)
    Invalid(String),
    /// what was asked for isn't there (a connection, a key, a profile)
    NotFound(String),
    /// the SSH tunnel to the server couldn't be set up
    Tunnel(String),
    /// Redis refused the username or password
    Auth(String),
    /// the user's ACL doesn't allow a command
    Denied(String),
    /// the server couldn't be reached, or the connection dropped
    Network(String),
    /// something took longer than it is allowed to
    Timeout(String),
    /// Redis answered with any other error
    Redis(String),
    /// a Redisql query or a filter path couldn't be understood
    Parse(String),
    /// the system keyring couldn't be used
    Keyring(String),
    /// a file couldn't be read or written
    File(String),
    /// AWS refused or failed a request
    Cloud(String),
    /// the operating system failed us otherwise (clipboard, browser, a task)
    System(String),
    /// the server is set up in a way the app doesn't handle (cluster mode)
    Unsupported(String),
}

impl AppError {
    pub fn invalid(message: impl Into<String>) -> Self {
        Self::Invalid(message.into())
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::NotFound(message.into())
    }

    pub fn parse(message: impl Into<String>) -> Self {
        Self::Parse(message.into())
    }

    pub fn message(&self) -> &str {
        match self {
            Self::Invalid(m)
            | Self::NotFound(m)
            | Self::Tunnel(m)
            | Self::Auth(m)
            | Self::Denied(m)
            | Self::Network(m)
            | Self::Timeout(m)
            | Self::Redis(m)
            | Self::Parse(m)
            | Self::Keyring(m)
            | Self::File(m)
            | Self::Cloud(m)
            | Self::System(m)
            | Self::Unsupported(m) => m,
        }
    }

    /// The same kind of error with `prefix` put before its message.
    pub fn context(self, prefix: &str) -> Self {
        self.map_message(|m| format!("{prefix}{m}"))
    }

    fn map_message(self, f: impl FnOnce(String) -> String) -> Self {
        match self {
            Self::Invalid(m) => Self::Invalid(f(m)),
            Self::NotFound(m) => Self::NotFound(f(m)),
            Self::Tunnel(m) => Self::Tunnel(f(m)),
            Self::Auth(m) => Self::Auth(f(m)),
            Self::Denied(m) => Self::Denied(f(m)),
            Self::Network(m) => Self::Network(f(m)),
            Self::Timeout(m) => Self::Timeout(f(m)),
            Self::Redis(m) => Self::Redis(f(m)),
            Self::Parse(m) => Self::Parse(f(m)),
            Self::Keyring(m) => Self::Keyring(f(m)),
            Self::File(m) => Self::File(f(m)),
            Self::Cloud(m) => Self::Cloud(f(m)),
            Self::System(m) => Self::System(f(m)),
            Self::Unsupported(m) => Self::Unsupported(f(m)),
        }
    }
}

impl std::fmt::Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.message())
    }
}

impl std::error::Error for AppError {}

impl From<redis::RedisError> for AppError {
    fn from(error: redis::RedisError) -> Self {
        use redis::{ErrorKind, ServerErrorKind};
        let message = error.to_string();
        if error.is_timeout() {
            return Self::Timeout(message);
        }
        if error.is_io_error() || error.is_connection_refusal() || error.is_connection_dropped() {
            return Self::Network(message);
        }
        match (error.kind(), error.code()) {
            (ErrorKind::AuthenticationFailed, _) | (_, Some("WRONGPASS" | "NOAUTH")) => Self::Auth(message),
            (ErrorKind::Server(ServerErrorKind::NoPerm), _) | (_, Some("NOPERM")) => Self::Denied(message),
            _ => Self::Redis(message),
        }
    }
}

impl From<tokio::task::JoinError> for AppError {
    fn from(error: tokio::task::JoinError) -> Self {
        Self::System(t!("A background task failed: %{error}", error = error.to_string()).into_owned())
    }
}
