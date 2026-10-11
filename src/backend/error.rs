use rust_i18n::t;

/// Declares [`ErrorKind`] and, for each kind, the [`AppError`] constructor of
/// that name (`File` → `AppError::file(message)`).
macro_rules! error_kinds {
    ($($(#[$doc:meta])* $kind:ident => $constructor:ident,)*) => {
        /// What went wrong, so the UI can react to each kind (point at the SSH
        /// tunnel, suggest checking a password).
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum ErrorKind {
            $($(#[$doc])* $kind,)*
        }

        impl AppError {
            $(
                pub fn $constructor(message: impl Into<String>) -> Self {
                    Self { kind: ErrorKind::$kind, message: message.into() }
                }
            )*
        }
    };
}

error_kinds! {
    /// what the user typed can't be used (a required field left empty, a bad URL)
    Invalid => invalid,
    /// what was asked for isn't there (a connection, a key, a profile)
    NotFound => not_found,
    /// the SSH tunnel to the server couldn't be set up
    Tunnel => tunnel,
    /// Redis refused the username or password
    Auth => auth,
    /// the user's ACL doesn't allow a command
    Denied => denied,
    /// the server couldn't be reached, or the connection dropped
    Network => network,
    /// something took longer than it is allowed to
    Timeout => timeout,
    /// Redis answered with any other error
    Redis => redis,
    /// a Redisql query or a filter path couldn't be understood
    Parse => parse,
    /// the system keyring couldn't be used
    Keyring => keyring,
    /// a file couldn't be read or written
    File => file,
    /// AWS refused or failed a request
    Cloud => cloud,
    /// the operating system failed us otherwise (clipboard, browser, a task)
    System => system,
    /// the server is set up in a way the app doesn't handle (cluster mode)
    Unsupported => unsupported,
}

/// An error of some [`ErrorKind`]; `Display` gives the message to show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppError {
    pub kind: ErrorKind,
    message: String,
}

impl AppError {
    pub fn message(&self) -> &str {
        &self.message
    }

    /// The same kind of error with `prefix` put before its message.
    pub fn context(mut self, prefix: &str) -> Self {
        self.message.insert_str(0, prefix);
        self
    }
}

impl std::fmt::Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for AppError {}

impl From<redis::RedisError> for AppError {
    fn from(error: redis::RedisError) -> Self {
        use redis::{ErrorKind as RedisKind, ServerErrorKind};
        let message = error.to_string();
        if error.is_timeout() {
            return Self::timeout(message);
        }
        if error.is_io_error() || error.is_connection_refusal() || error.is_connection_dropped() {
            return Self::network(message);
        }
        match (error.kind(), error.code()) {
            (RedisKind::AuthenticationFailed, _) | (_, Some("WRONGPASS" | "NOAUTH")) => Self::auth(message),
            (RedisKind::Server(ServerErrorKind::NoPerm), _) | (_, Some("NOPERM")) => Self::denied(message),
            _ => Self::redis(message),
        }
    }
}

impl From<tokio::task::JoinError> for AppError {
    fn from(error: tokio::task::JoinError) -> Self {
        Self::system(t!("A background task failed: %{error}", error = error.to_string()))
    }
}
