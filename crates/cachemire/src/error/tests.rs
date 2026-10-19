use std::path::PathBuf;
use std::sync::Arc;

use super::*;

fn io() -> std::io::Error {
    std::io::Error::other("disk full")
}

fn json() -> serde_json::Error {
    serde_json::from_str::<serde_json::Value>("{").unwrap_err()
}

/// The error of a task that was cancelled.
fn join_error() -> tokio::task::JoinError {
    let runtime = tokio::runtime::Builder::new_current_thread().build().unwrap();
    runtime.block_on(async {
        let task = tokio::spawn(std::future::pending::<()>());
        task.abort();
        task.await.unwrap_err()
    })
}

fn service() -> AwsError {
    AwsError::Service { message: "Access denied (ForbiddenException)".into(), source: Box::new(io()) }
}

/// One of each error the core can give.
fn core_errors() -> Vec<CoreError> {
    let file = || PathBuf::from("/home/me/.ssh/id_ed25519");
    let invalid = [
        Invalid::ConnectionName,
        Invalid::ConnectionUrl,
        Invalid::UnreadableUrl,
        Invalid::UrlWithoutHost,
        Invalid::SshHost,
        Invalid::SshPort,
        Invalid::SshLogin,
        Invalid::SshCredentials,
        Invalid::ProfileName,
        Invalid::ProfileUsername,
        Invalid::ProfilePassword,
        Invalid::QueryName,
        Invalid::QueryText,
        Invalid::SearchDepth,
        Invalid::KeyRequired,
    ];
    let not_found = [
        NotFound::Connection,
        NotFound::ActiveConnection,
        NotFound::SshProfile,
        NotFound::ConnectionProfile,
        NotFound::SavedQuery,
        NotFound::Key,
    ];
    let tunnel = [
        TunnelError::HostKeyNotSaved { file: file(), source: russh::keys::Error::CouldNotReadKey },
        TunnelError::HostKeyChanged { line: 3, file: file() },
        TunnelError::HostKeyUnchecked(russh::keys::Error::CouldNotReadKey),
        TunnelError::KeyPassphrase { file: file() },
        TunnelError::KeyUnreadable { file: file(), source: russh::keys::Error::CouldNotReadKey },
        TunnelError::NothingToLogInWith,
        TunnelError::LoginRefused { user: "app".into(), key: true, password: true },
        TunnelError::LoginRefused { user: "app".into(), key: true, password: false },
        TunnelError::LoginRefused { user: "app".into(), key: false, password: true },
        TunnelError::Connect { host: "bastion".into(), port: 22, source: russh::Error::KexInit },
        TunnelError::Timeout { host: "bastion".into(), port: 22 },
        TunnelError::Unreachable { host: "cache".into(), port: 6379, source: russh::Error::KexInit },
        TunnelError::LocalPort(io()),
        TunnelError::Ssh(russh::Error::KexInit),
    ];
    let aws = [
        AwsError::Incomplete,
        service(),
        AwsError::NoUsableCredentials { profile: "prod".into(), reason: Some("the token expired".into()) },
        AwsError::NoUsableCredentials { profile: "prod".into(), reason: None },
        AwsError::RoleRefused { profile: "prod".into(), role: "Admin".into(), account: "123456789012".into(), source: Box::new(service()) },
        AwsError::NoCredentials,
        AwsError::NoProfile,
        AwsError::ProfileNotFound("prod".into()),
        AwsError::NoRegion("prod".into()),
        AwsError::SignInTimedOut,
        AwsError::Browser(io()),
        AwsError::SignInRefused("access_denied".into()),
        AwsError::Callback(io()),
    ];
    let query = [
        QueryError::UnterminatedQuote("'users".into()),
        QueryError::UnexpectedText("??".into()),
        QueryError::UnknownVariable("id".into()),
        QueryError::InvalidFrom,
        QueryError::InvalidJoin,
        QueryError::JoinOnItself,
        QueryError::OnWithoutOwnAlias("b".into()),
        QueryError::InvalidWhere("a.x ==".into()),
        QueryError::InWithoutList("1, 2".into()),
        QueryError::EmptyInList,
        QueryError::InvalidSelect,
        QueryError::InvalidLimit,
        QueryError::EmptyQuery,
        QueryError::RepeatedClause("WHERE".into()),
        QueryError::UnknownClause("ORDER BY x".into()),
        QueryError::MissingFrom,
        QueryError::DuplicateAlias("a".into()),
        QueryError::UnknownJoinAlias { alias: "b".into(), other: "c".into() },
        QueryError::UnknownWhereAlias("c".into()),
        QueryError::UnknownSelectAlias("c".into()),
        QueryError::InvalidDeclaration,
        QueryError::KeyNotFound("users".into()),
        QueryError::NotQueryable { key: "users".into(), kind: "list".into() },
        QueryError::NonObjectField { key: "users".into(), field: "u1".into(), kind: "array" },
        QueryError::FieldJson { key: "users".into(), field: "u1".into(), source: json() },
        QueryError::NonObjectValue { key: "users".into(), kind: "number" },
        QueryError::ValueJson { key: "users".into(), source: json() },
    ];
    let json_path = [
        ParseError::ExpectedKey(Found::Char { c: '.', position: 4 }),
        ParseError::UnclosedQuote,
        ParseError::IndexTooBig("99999999999999999999".into()),
        ParseError::ExpectedInBrackets(Found::End),
        ParseError::ExpectedClosingBracket(Found::Char { c: 'x', position: 2 }),
        ParseError::ExpectedSeparator(Found::End),
    ];
    let keyring = [
        KeyringError::Vault(VaultError::Unavailable),
        KeyringError::ConnectionPasswords(VaultError::Json(Arc::new(json()))),
        KeyringError::SshProfilePassword(VaultError::Unavailable),
        KeyringError::TunnelSecrets(VaultError::Unavailable),
    ];

    let mut errors: Vec<CoreError> = Vec::new();
    errors.extend(invalid.map(CoreError::from));
    errors.extend(not_found.map(CoreError::from));
    errors.extend(tunnel.map(CoreError::from));
    errors.extend(aws.map(CoreError::from));
    errors.extend(query.map(CoreError::from));
    errors.extend(json_path.map(CoreError::from));
    errors.extend(keyring.map(CoreError::from));
    errors.extend([
        Timeout::Connect { seconds: 10 }.into(),
        FileError::Io(io()).into(),
        FileError::Json(json()).into(),
        FileError::NoFileName(PathBuf::from("/")).into(),
        CoreError::from(join_error()),
        CoreError::ClusterMode,
    ]);
    errors
}

/// In English, what the user reads is what the log says: a `Localize` arm whose
/// text differs from its error's `Display` would lose that message's translation.
#[test]
fn every_error_reads_in_english_as_its_display() {
    assert_eq!(&*rust_i18n::locale(), "en", "the tests run in English");
    for error in core_errors() {
        assert_eq!(error.localized(), error.to_string(), "{error:?}");
    }
    let app_errors =
        [Error::ReadFile(io()), Error::NotJson(json()), Error::NoConnections, Error::Io(io()), Error::Core(CoreError::ClusterMode)];
    for error in app_errors {
        assert_eq!(error.localized(), error.to_string(), "{error:?}");
    }
}
