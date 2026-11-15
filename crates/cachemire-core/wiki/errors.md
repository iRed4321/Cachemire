# Errors

The core's error system: one top-level enum, [`Error`](crate::error::Error), made of small
sub-enums grouped by *what went wrong*, with an English `Display` and a coarse
[`ErrorKind`](crate::error::ErrorKind) the UI can act on. The application turns each error into the
interface language (see the app's *errors* chapter; this crate never translates).

Where to look:

- `crates/cachemire-core/src/error.rs`: `Error`, `Invalid`, `NotFound`, `Timeout`, `FileError`,
  `SystemError`, `ErrorKind`, `Error::kind`.
- Sub-enums defined next to their feature: [`TunnelError`](crate::connection::ssh_tunnel::TunnelError),
  [`AwsError`](crate::aws::AwsError), [`QueryError`](crate::redisql::QueryError),
  [`ParseError`](crate::json_path::ParseError), [`KeyringError`](crate::secret_vault::KeyringError) and
  [`VaultError`](crate::secret_vault::VaultError).

## Shape

```text
 Error
  |-- Invalid(Invalid)          what the user gave can't be used            (many variants)
  |-- NotFound(NotFound)        what was asked for isn't there
  |-- Timeout(Timeout)          something took too long                     (Connect { seconds })
  |-- Redis(redis::RedisError)  the server or the wire failed               (as the client reports it)
  |-- Tunnel(Box<TunnelError>)  SSH tunnel                                  (boxed: the largest)
  |-- Keyring(KeyringError)     OS credential store           --- Vault(VaultError) | ...
  |-- File(FileError)           Io | Json | NoFileName
  |-- Cloud(AwsError)           AWS sign-in and listing
  |-- Query(QueryError)         a Redisql query
  |-- JsonPath(ParseError)      the key panel's filter path
  |-- System(SystemError)       Task(JoinError) | Panicked(String)
  |-- ClusterMode               cluster mode is not supported
  |-- ReadOnly                  a write through a read-only connection
  `-- BinaryData                data that isn't UTF-8 text can't be edited
```

[`error::Result<T, E = Error>`](crate::error::Result) is the alias used across the crate.

### The sub-enums

| Enum | Meaning | Examples | Produced by |
|---|---|---|---|
| [`Invalid`](crate::error::Invalid) | the input can't be used | `ConnectionName`, `UnknownScheme(String)`, `SshPort`, `SearchDepth`, `KeyRequired` | URL parsing, the connection store, profile and saved-query stores, the explorer |
| [`NotFound`](crate::error::NotFound) | a lookup found nothing | `Connection`, `ActiveConnection`, `SshProfile`, `SavedQuery`, `Key` | [`state`](crate::state) lookups, stores, the explorer |
| [`Timeout`](crate::error::Timeout) | too slow | `Connect { seconds }` | the pool's open (30 s limit) |
| [`FileError`](crate::error::FileError) | a file couldn't be read or written | `Io`, `Json`, `NoFileName(PathBuf)` | [`persist`](crate::persist) and stores |
| [`SystemError`](crate::error::SystemError) | the OS or runtime failed otherwise | `Task(JoinError)`, `Panicked(String)` | a joined task that failed; a blocking job that panicked |
| `TunnelError` | the tunnel couldn't be set up | `UnknownHostKey`, `HostKeyChanged`, `LoginRefused`, `Connect`, `Unreachable`, `Tls` | [`ssh_tunnel`](crate::connection::ssh_tunnel) |
| `AwsError` | AWS refused or failed | `NoProfile`, `SignInTimedOut`, `RoleRefused`, `Service` | [`aws`](crate::aws) |
| `QueryError` | a query can't be understood or run | `UnterminatedQuote`, `MissingFrom`, `KeyNotFound`, `StringTooBig` | [`redisql`](crate::redisql) |
| `ParseError` | a filter path is malformed | `ExpectedKey`, `UnclosedQuote`, `IndexTooBig` | [`json_path`](crate::json_path) |
| `KeyringError` / `VaultError` | the credential store failed | `CantStore`, `ConnectionPasswords`, `Unavailable` | [`secret_vault`](crate::secret_vault) |

The three variants without a payload enum are cases that deserve a message of their own:
[`ClusterMode`](crate::error::Error::ClusterMode) (the pool refuses a cluster server up front),
[`ReadOnly`](crate::error::Error::ReadOnly) (raised by [`ensure_writable`](crate::state::ensure_writable), through
which every write passes) and [`BinaryData`](crate::error::Error::BinaryData) (an edit of a non-text value).

## `Display` is English, and for the log

Every variant has a `thiserror` `#[error("...")]` message in English. It serves the log and the tests.
The user sees the same sentence translated by the app; the two must read identically in English
(the app's tests enforce it). Consequences for the messages:

- Wrapping variants use `#[error(transparent)]` so there is one sentence, not a chain of prefixes.
- A variant carries **data**, not a pre-built string, so the app can put the data in a translated
  template (`UnknownScheme(String)`, `Timeout::Connect { seconds }`).
- `Display` text of the backend errors (`TunnelError`, `QueryError`, `AwsError`) differs in style:
  `Invalid`, `NotFound`, `QueryError` and `AwsError` use sentences that start with a capital and end with
  a full stop or example; `TunnelError` and the keyring messages are lower-case clauses meant to
  follow a prefix. The app mirrors each exactly.

## `source()` and conversions

- A variant holding another error exposes it as `source()` (`#[source]` or `#[from]`).
- `#[from]` conversions let `?` lift a sub-error into `Error`: `Invalid`, `NotFound`, `Timeout`,
  `RedisError`, `KeyringError`, `FileError`, `AwsError`, `QueryError`, `ParseError`, `SystemError`.
- Written by hand: `JoinError -> Error::System(SystemError::Task)`; `TunnelError -> Error::Tunnel(Box)`
  (the box keeps `Error` small); `VaultError -> Error::Keyring(KeyringError::Vault)`.
- [`VaultError`](crate::secret_vault::VaultError) is `Clone` (its causes are in `Arc`s) because
  [`staged`](crate::secret_vault::staged) replays the vault's answers; see the
  [secrets](crate::wiki::secrets) chapter.
- A Redisql failure carries a position: [`Located`](crate::redisql::Located) holds an `Error` and the byte
  span of the statement it is about.

## `ErrorKind`: what the UI can make of it

[`Error::kind`](crate::error::Error::kind) maps any error to one of fourteen
[`ErrorKind`](crate::error::ErrorKind)s.

| Kind | From |
|---|---|
| `Invalid` | `Invalid`, `BinaryData`, and `QueryError` about values that can't be queried |
| `NotFound` | `NotFound`, `QueryError::KeyNotFound`, `AwsError::NoProfile` / `ProfileNotFound` |
| `Tunnel` | any `TunnelError` |
| `Auth` | a Redis `WRONGPASS` / `NOAUTH` or authentication failure |
| `Denied` | a Redis `NOPERM`, and `ReadOnly` |
| `Network` | an I/O error, a refused or dropped connection |
| `Timeout` | `Timeout`, a timed-out Redis call, `AwsError::SignInTimedOut` |
| `Redis` | any other server reply |
| `Parse` | `JsonPath`, other `QueryError`s |
| `Keyring`, `File`, `Cloud`, `System`, `Unsupported` | the variant of the same name (`Unsupported`: `ClusterMode`) |

Sub-enums with several kinds (`AwsError`, `QueryError`) have their own `kind()`; Redis errors are
sorted by `redis_kind` (timeout first, then network, then auth, then permission).

The kind also serves the core itself: `is_script_refused` (a server that has no scripting, or an ACL
that denies it) lets the search fall back, and `is_unknown_option` (an old server's `syntax error`)
lets the session stop using `HSCAN ... NOVALUES`. These two helpers look at the reply's code and
detail words and apply only to a `Redis` error that is not a network or timeout one.

## Conventions

1. **A new failure is a variant, not a string.** Pick the sub-enum by meaning, or add one.
2. **Write the English `#[error]` as the final user text**; do not rely on the app to rephrase it.
3. **Keep data in fields**, derive `Debug`, and give wrapped errors a `source`.
4. **Add the `kind`** when the new variant isn't covered by the existing mapping (`Error::kind` is an
   exhaustive match for the top level).
5. **Update the app**: a new variant needs a `Localize` arm and an `app.yml` entry (the build fails on
   the missing arm), see the app's *errors* chapter.
6. **No `unwrap` outside tests**: the workspace lints warn on it. Use `?` and let the error reach the UI.
7. **Never put a secret in an error.** Messages name files, hosts and users, never passwords or
   tokens ([`Secret`](crate::secret::Secret) prints `***` in `Debug` anyway).

## See also

[connection](crate::wiki::connection) (tunnel and pool failures), [secrets](crate::wiki::secrets)
(keyring errors), [aws](crate::wiki::aws), [redisql](crate::wiki::redisql),
[utilities](crate::wiki::utilities) (filter path errors), and the application's *errors* chapter for
`Localize`, toasts and dialogs.
