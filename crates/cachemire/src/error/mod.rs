//! The app's own errors, and how every error reaches the user: [`Localize`] gives its message
//! in the interface language, while `Display` stays English for the log. What comes from
//! outside (Redis, the OS, a library) is shown as it was given.

use rust_i18n::t;

use cachemire_core::aws::AwsError;
use cachemire_core::connection::ssh_tunnel::TunnelError;
use cachemire_core::error::{Error as CoreError, FileError, Invalid, NotFound, SystemError, Timeout};
use cachemire_core::json_path::{Found, ParseError};
use cachemire_core::redisql::QueryError;
use cachemire_core::secret_vault::{KeyringError, VaultError};

/// What can go wrong in the app beyond the core.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Core(#[from] CoreError),
    #[error("Couldn't read the file: {0}")]
    ReadFile(#[source] std::io::Error),
    #[error(transparent)]
    NotJson(#[from] serde_json::Error),
    #[error("This file has no connections.")]
    NoConnections,
    #[error(transparent)]
    Clipboard(#[from] arboard::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

impl From<tokio::task::JoinError> for Error {
    fn from(error: tokio::task::JoinError) -> Self {
        Error::Core(error.into())
    }
}

/// An error's message in the interface language.
pub trait Localize {
    fn localized(&self) -> String;
}

impl Localize for Error {
    fn localized(&self) -> String {
        match self {
            Error::Core(error) => error.localized(),
            Error::ReadFile(error) => t!("Couldn't read the file: %{error}", error = error).into_owned(),
            Error::NoConnections => t!("This file has no connections.").into_owned(),
            Error::NotJson(_) | Error::Clipboard(_) | Error::Io(_) => self.to_string(),
        }
    }
}

impl Localize for CoreError {
    fn localized(&self) -> String {
        match self {
            CoreError::Invalid(error) => error.localized(),
            CoreError::NotFound(error) => error.localized(),
            CoreError::Timeout(error) => error.localized(),
            CoreError::Tunnel(error) => error.localized(),
            CoreError::Keyring(error) => error.localized(),
            CoreError::File(error) => error.localized(),
            CoreError::Cloud(error) => error.localized(),
            CoreError::Query(error) => error.localized(),
            CoreError::JsonPath(error) => error.localized(),
            CoreError::System(error) => error.localized(),
            CoreError::ClusterMode => t!("This server runs in cluster mode, which Cachemire doesn't support yet. Connect to a server or endpoint with cluster mode disabled.").into_owned(),
            CoreError::Redis(_) => self.to_string(),
        }
    }
}

impl Localize for Invalid {
    fn localized(&self) -> String {
        match self {
            Invalid::ConnectionName => t!("Connection name is required."),
            Invalid::ConnectionUrl => t!("Connection URL is required."),
            Invalid::UnreadableUrl => t!("Couldn't understand the connection URL."),
            Invalid::UrlWithoutHost => t!("The connection URL has no host."),
            Invalid::SshHost => t!("SSH host is required."),
            Invalid::SshPort => t!("SSH port must be a number between 1 and 65535."),
            Invalid::SshLogin => t!("Provide an SSH password or a private key."),
            Invalid::SshCredentials => {
                t!("This SSH connection has no username or password: fill them in, or make an SSH profile active in Settings.")
            }
            Invalid::ProfileName => t!("The profile needs a name."),
            Invalid::ProfileUsername => t!("The profile needs an SSH username."),
            Invalid::ProfilePassword => t!("The profile needs an SSH password."),
            Invalid::QueryName => t!("The query needs a name."),
            Invalid::QueryText => t!("The query needs some text to save."),
            Invalid::SearchDepth => t!("That search depth isn't one of the choices."),
            Invalid::KeyRequired => t!("The key parameter is required."),
        }
        .into_owned()
    }
}

impl Localize for NotFound {
    fn localized(&self) -> String {
        match self {
            NotFound::Connection => t!("Connection not found."),
            NotFound::ActiveConnection => t!("No active connection."),
            NotFound::SshProfile => t!("SSH profile not found."),
            NotFound::ConnectionProfile => t!("Connection profile not found."),
            NotFound::SavedQuery => t!("Saved query not found."),
            NotFound::Key => t!("Key not found."),
        }
        .into_owned()
    }
}

impl Localize for Timeout {
    fn localized(&self) -> String {
        match self {
            Timeout::Connect { seconds } => t!("Connection timed out after %{seconds} seconds.", seconds = seconds).into_owned(),
        }
    }
}

impl Localize for FileError {
    fn localized(&self) -> String {
        self.to_string()
    }
}

impl Localize for SystemError {
    fn localized(&self) -> String {
        match self {
            SystemError::Task(error) => t!("A background task failed: %{error}", error = error).into_owned(),
        }
    }
}

impl Localize for VaultError {
    fn localized(&self) -> String {
        match self {
            VaultError::Json(error) => t!("the saved secrets can't be read: %{error}", error = error).into_owned(),
            VaultError::Keyring(_) | VaultError::Unavailable => self.to_string(),
        }
    }
}

impl Localize for KeyringError {
    fn localized(&self) -> String {
        match self {
            KeyringError::Vault(error) => error.localized(),
            KeyringError::ConnectionPasswords(error) => {
                t!("Couldn't read the saved passwords from the system keyring: %{error}", error = error.localized()).into_owned()
            }
            KeyringError::SshProfilePassword(error) => {
                t!("Couldn't read the SSH profile's password from the system keyring: %{error}", error = error.localized()).into_owned()
            }
            KeyringError::TunnelSecrets(error) => {
                t!("Couldn't read the SSH tunnel's secrets from the system keyring: %{error}", error = error.localized()).into_owned()
            }
        }
    }
}

impl Localize for TunnelError {
    fn localized(&self) -> String {
        match self {
            TunnelError::HostKeyNotSaved { file, source } => t!("couldn't remember the SSH server's host key in %{file}: %{error}", file = file.display(), error = source),
            TunnelError::HostKeyChanged { line, file } => t!(
                "the SSH server's host key has changed since it was first seen (recorded on line %{line} of %{file}). If the server was reinstalled, delete that line; otherwise someone may be impersonating it.",
                line = line,
                file = file.display()
            ),
            TunnelError::HostKeyUnchecked(error) => t!("couldn't check the SSH server's host key: %{error}", error = error),
            TunnelError::KeyPassphrase { file } => t!("couldn't decrypt the SSH private key %{file}: wrong or missing passphrase?", file = file.display()),
            TunnelError::KeyUnreadable { file, source } => t!("couldn't read the SSH private key %{file}: %{error}", file = file.display(), error = source),
            TunnelError::NothingToLogInWith => t!("no SSH password or private key to log in with"),
            TunnelError::LoginRefused { user, key, password } => {
                let tried = match (key, password) {
                    (true, true) => format!("{} {} {}", t!("private key"), t!("and"), t!("password")),
                    (true, false) => t!("private key").into_owned(),
                    _ => t!("password").into_owned(),
                };
                t!("the SSH server refused the login for %{user} (%{tried})", user = user, tried = tried)
            }
            TunnelError::Connect { host, port, source } => t!("couldn't connect to the SSH server %{host}:%{port}: %{error}", host = host, port = port, error = source),
            TunnelError::Timeout { host, port } => t!("timed out connecting to the SSH server %{host}:%{port}", host = host, port = port),
            TunnelError::Unreachable { host, port, source } => t!("the SSH server couldn't reach %{host}:%{port}: %{error}", host = host, port = port, error = source),
            TunnelError::LocalPort(error) => t!("couldn't open a local port for the SSH tunnel: %{error}", error = error),
            TunnelError::Ssh(_) => return self.to_string(),
        }
        .into_owned()
    }
}

impl Localize for AwsError {
    fn localized(&self) -> String {
        match self {
            AwsError::Incomplete => t!("Incomplete answer from AWS."),
            AwsError::Service { .. } => return self.to_string(),
            AwsError::NoUsableCredentials { profile, reason } => {
                let reason = reason.clone().unwrap_or_else(|| t!("No credentials.").into_owned());
                t!("No usable credentials for the profile %{name}: %{error}", name = profile, error = reason)
            }
            AwsError::RoleRefused { profile, role, account, source } => t!(
                "The profile %{name} can't take the role %{role} in the account %{account}: %{error}",
                name = profile,
                role = role,
                account = account,
                error = source.localized()
            ),
            AwsError::NoCredentials => t!("No credentials."),
            AwsError::NoProfile => t!("No AWS profile found in the .aws folder."),
            AwsError::ProfileNotFound(name) => t!("AWS profile not found: %{name}", name = name),
            AwsError::NoRegion(name) => t!("The profile %{name} has no region.", name = name),
            AwsError::SignInTimedOut => t!("The sign-in in the browser took too long."),
            AwsError::Browser(error) => t!("Couldn't open the browser: %{error}", error = error),
            AwsError::SignInRefused(reason) => t!("The sign-in was refused in the browser: %{error}", error = reason),
            AwsError::Callback(error) => t!("Couldn't wait for the browser's answer: %{error}", error = error),
        }
        .into_owned()
    }
}

impl Localize for QueryError {
    fn localized(&self) -> String {
        match self {
            QueryError::UnterminatedQuote(text) => t!("Unterminated quote: %{text}", text = text),
            QueryError::UnexpectedText(text) => t!("Unexpected text: %{text}", text = text),
            QueryError::UnknownVariable(name) => t!("Unknown variable: &%{name}", name = name),
            QueryError::InvalidFrom => t!("Invalid FROM clause. Example: FROM KEY 'my:key' AS src"),
            QueryError::InvalidJoin => t!("Invalid JOIN clause. Example: JOIN KEY 'my:key' AS b ON a.id = b.refId"),
            QueryError::JoinOnItself => t!("A JOIN's ON must link two different aliases."),
            QueryError::OnWithoutOwnAlias(alias) => t!("The ON clause must name the JOIN's own alias (%{alias}).", alias = alias),
            QueryError::InvalidWhere(condition) => t!("Invalid WHERE condition: %{condition}", condition = condition),
            QueryError::InWithoutList(text) => t!("IN needs a list in [] or (): %{text}", text = text),
            QueryError::EmptyInList => t!("IN's list is empty."),
            QueryError::InvalidSelect => t!("Invalid SELECT clause."),
            QueryError::InvalidLimit => t!("LIMIT must be a whole number."),
            QueryError::EmptyQuery => t!("The query is empty."),
            QueryError::RepeatedClause(clause) => t!("%{clause} can only appear once.", clause = clause),
            QueryError::UnknownClause(clause) => t!("Unrecognized clause: %{clause}", clause = clause),
            QueryError::MissingFrom => t!("Missing FROM clause."),
            QueryError::DuplicateAlias(alias) => t!("Duplicate alias: %{alias}", alias = alias),
            QueryError::UnknownJoinAlias { alias, other } => {
                t!("JOIN %{alias} references an unknown alias: %{other}", alias = alias, other = other)
            }
            QueryError::UnknownWhereAlias(alias) => t!("Unknown alias in WHERE: %{alias}", alias = alias),
            QueryError::UnknownSelectAlias(alias) => t!("Unknown alias in SELECT: %{alias}", alias = alias),
            QueryError::InvalidDeclaration => t!("Invalid variable declaration. Example: var companyId = 255;"),
            QueryError::KeyNotFound(key) => t!("Key not found: %{key}", key = key),
            QueryError::NotQueryable { key, kind } => {
                t!("%{key} is a %{kind}: only a hash of JSON objects, or a string holding one, can be queried.", key = key, kind = kind)
            }
            QueryError::NonObjectField { key, field, kind } => {
                t!("A non-object value was found in %{key} (field %{field}): %{kind}", key = key, field = field, kind = kind)
            }
            QueryError::FieldJson { key, field, source } => {
                t!("Invalid JSON in %{key} (field %{field}): %{error}", key = key, field = field, error = source)
            }
            QueryError::NonObjectValue { key, kind } => {
                t!("%{key} holds JSON, but not an object or an array of objects: %{kind}", key = key, kind = kind)
            }
            QueryError::ValueJson { key, source } => t!("Invalid JSON in %{key}: %{error}", key = key, error = source),
        }
        .into_owned()
    }
}

impl Localize for ParseError {
    fn localized(&self) -> String {
        match self {
            ParseError::ExpectedKey(found) => t!("expected a key, found %{found}", found = found_text(found)),
            ParseError::UnclosedQuote => t!("a quoted key is missing its closing quote"),
            ParseError::IndexTooBig(digits) => t!("'%{digits}' is too big an index", digits = digits),
            ParseError::ExpectedInBrackets(found) => {
                t!("expected *, an index or a quoted key after '[', found %{found}", found = found_text(found))
            }
            ParseError::ExpectedClosingBracket(found) => t!("expected ']', found %{found}", found = found_text(found)),
            ParseError::ExpectedSeparator(found) => t!("expected '.' or '[', found %{found}", found = found_text(found)),
        }
        .into_owned()
    }
}

fn found_text(found: &Found) -> String {
    match found {
        Found::Char { c, position } => t!("'%{c}' at position %{position}", c = c, position = position),
        Found::End => t!("the end of the filter"),
    }
    .into_owned()
}

#[cfg(test)]
mod tests;
