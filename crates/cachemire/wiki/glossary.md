# Glossary

The project's terms, in alphabetical order, each with the chapter or item that explains it.

**Active connection**
: The saved connection the store records as in use ([`ConnectionStore`](cachemire_core::connection::store::ConnectionStore)),
  persisted. It changes only when a connect succeeds or the user disconnects; at startup it is
  cleared on purpose ([`keep_active_in_step`](crate::connections::keep_active_in_step)). Core calls with no
  explicit connection id use it. See [state](crate::wiki::state).

**App**
: [`App`](crate::app::App): the UI thread's shared state, a `thread_local` `Rc`. Reached with
  [`app::with`](crate::app::with). See [runtime](crate::wiki::runtime).

**`app.yml`**
: `resources/lang/app.yml`: the French text of the messages built in Rust (`t!`), keyed by the English
  text. See [errors](crate::wiki::errors) and [i18n](crate::wiki::i18n).

**`AppState`**
: The core's shared state: the three stores, the pool cache and the attempt counter.
  [`AppState`](cachemire_core::state::AppState). See [state](crate::wiki::state).

**Attempt (connect attempt)**
: One try at connecting, numbered by
  [`connect_attempt`](cachemire_core::state::AppState::connect_attempt); an outcome that is not the
  latest is dropped. See [state](crate::wiki::state).

**Backend (Slint backend)**
: The windowing and rendering choice made by `select_backend` in `main.rs` (software renderer on
  Windows). See [platform](crate::wiki::platform).

**Background connection**
: The pool connection reserved for long reads (the walk, searches, exports, queries), so clicks are
  not queued behind them. See [`Session::open_background`](cachemire_core::session::Session::open_background) and
  [core connection](cachemire_core::wiki::connection).

**Callback**
: A Slint event that Rust answers, declared in a global of `globals.slint` and attached with an
  `on_*` setter in a module's `init`. See [ui](crate::wiki::ui) and [architecture](crate::wiki::architecture).

**Changelog**
: `CHANGELOG.md`, with an `## Unreleased` section for user-visible changes. A repository rule, see
  `CLAUDE.md`.

**Cloud connection**
: A connection found through an AWS account (ElastiCache) rather than saved by hand; its id starts
  with [`aws::ID_PREFIX`](cachemire_core::aws::ID_PREFIX). See [connections](crate::wiki::connections) and
  [core aws](cachemire_core::wiki::aws).

**Connection (saved)**
: A [`ConnectionItem`](cachemire_core::connection::ConnectionItem) in `databases.json`: URL, optional SSH
  tunnel, profile, read-only flag. See [core connection](cachemire_core::wiki::connection).

**Connection pool**
: [`ConnectionPool`](cachemire_core::connection::pool::ConnectionPool): three physical connections to one
  server plus its SSH tunnel; cached by [`ConnectionCache`](cachemire_core::connection::pool::ConnectionCache).
  See [state](crate::wiki::state).

**Connection profile**
: A named set of options (colour tint, read-only) applied to the connections that pick it.
  [`ConnectionProfile`](cachemire_core::settings::ConnectionProfile). See [settings](crate::wiki::settings).

**Core (cachemire-core)**
: The UI-free library crate. See [`cachemire_core`].

**Delimiter**
: The character that cuts key names into folders in the tree (`:` by default; the tree's
  `DELIMITER`). See [keys_and_tabs](crate::wiki::keys_and_tabs).

**Epoch / generation**
: Counters that let a result know it is stale (`view_epoch` in the key panel, `generation` of a
  keyspace walk). See [state](crate::wiki::state).

**`ErrorKind`**
: What an error means for the UI. [`ErrorKind`](cachemire_core::error::ErrorKind). See
  [errors](crate::wiki::errors) and [core errors](cachemire_core::wiki::errors).

**Explorer**
: The core module that walks and searches the keyspace and reads a key's details.
  [`explorer`](cachemire_core::explorer). See [core explorer](cachemire_core::wiki::explorer).

**Global (Slint)**
: A Slint `global` in `globals.slint` holding one feature's state and callbacks (`ConnectionState`,
  `KeyTree`, `KeyPanel`, `Preferences`...). See [ui](crate::wiki::ui) and [state](crate::wiki::state).

**Harness**
: The headless Slint test window of `ui_tests`. See [testing](crate::wiki::testing).

**Host key**
: The public key an SSH server presents. A key seen for the first time asks the user; a changed key is an
  error. [`HostKeys`](cachemire_core::connection::ssh_tunnel::HostKeys),
  [`UnknownHostKey`](cachemire_core::connection::ssh_tunnel::UnknownHostKey),
  [`host_key_dialog`](crate::host_key_dialog). See [connections](crate::wiki::connections).

**Instance lock**
: The exclusive lock on `cachemire.lock` that keeps two processes from saving over each other.
  [`InstanceLock`](cachemire_core::state::InstanceLock). See [state](crate::wiki::state).

**Key (Redis key)**
: A name and a value of some type. A name is bytes, not always UTF-8:
  [`KeyName`](cachemire_core::key_name::KeyName). See [core utilities](cachemire_core::wiki::utilities).

**Key panel**
: The right-hand panel showing the selected key's value (fields, table, JSON filter).
  [`KeyDetail`](crate::key_detail::KeyDetail). See [keys_and_tabs](crate::wiki::keys_and_tabs).

**Keyspace tree**
: The left-hand tree of keys grouped into folders, filled by the walk.
  [`Keyspace`](crate::keyspace::Keyspace). See [keys_and_tabs](crate::wiki::keys_and_tabs).

**Keyring**
: The OS credential store holding saved passwords. See [`secret_vault`](cachemire_core::secret_vault) and
  [core secrets](cachemire_core::wiki::secrets).

**Language**
: The interface language setting (automatic, English, French).
  [`Language`](cachemire_core::i18n::Language). See [i18n](crate::wiki::i18n).

**`later`**
: [`app::later`](crate::app::later): runs a closure on the UI thread from any thread. See
  [runtime](crate::wiki::runtime).

**Last session**
: The connection, tabs and open folders kept at exit and restored at the next start when the
  setting is on. [`last_session`](crate::last_session). See [platform](crate::wiki::platform).

**Link (`cachemire://`)**
: A shareable URL to a key. [`Link`](cachemire_core::link::Link),
  [`links`](crate::links). See [platform](crate::wiki::platform).

**`Localize`**
: The trait that gives an error's message in the interface language.
  [`Localize`](crate::error::Localize). See [errors](crate::wiki::errors).

**Parked tabs**
: The tabs of a connection that is not the one shown, kept in memory to come back to.
  [`Tabs`](crate::tabs::Tabs). See [state](crate::wiki::state).

**Portable mode**
: A `portable` file beside the executable makes the data live in a `data` folder there.
  [`resolve_data_file`](cachemire_core::state::resolve_data_file).

**Query tab / search tab**
: Tabs that are not keys: a Redisql query ([`QueryTabs`](crate::redisql::QueryTabs)) or a search
  ([`SearchTabs`](crate::search::SearchTabs)). See [keys_and_tabs](crate::wiki::keys_and_tabs).

**Read-only**
: A connection (or its profile) that refuses writes. The core enforces it with
  [`ensure_writable`](cachemire_core::state::ensure_writable). See [state](crate::wiki::state).

**Redisql**
: The small query language that joins JSON held in Redis keys.
  [`redisql`](cachemire_core::redisql). See [redisql](crate::wiki::redisql) and
  [core redisql](cachemire_core::wiki::redisql).

**Runtime**
: The Tokio runtime on the `redis-io` thread. [`runtime::spawn`](cachemire_core::runtime::spawn). See
  [runtime](crate::wiki::runtime).

**Secret**
: A password, passphrase or token: zeroized on drop, `Debug` prints `***`.
  [`Secret`](cachemire_core::secret::Secret). See [core secrets](cachemire_core::wiki::secrets).

**`SecretsPolicy`**
: What a save does when the keyring will not take a secret: fail, keep it in clear text, or leave it out.
  [`SecretsPolicy`](cachemire_core::secret_vault::SecretsPolicy).

**Session**
: A connection to one saved connection's server taken from its pool; every Redis command goes through
  it. [`Session`](cachemire_core::session::Session). Not to be confused with the *last session*.
  See [core session](cachemire_core::wiki::session).

**`spawn` / `spawn_blocking`**
: [`app::spawn`](crate::app::spawn), [`app::spawn_blocking`](crate::app::spawn_blocking): run work off the UI
  thread, then a closure on it. See [runtime](crate::wiki::runtime).

**`staged`**
: [`staged`](cachemire_core::secret_vault::staged): runs a store operation with its keyring calls made
  while the store's lock is free. See [core secrets](cachemire_core::wiki::secrets).

**SSH profile**
: A saved username and password a tunnel without its own login uses. See
  [settings](crate::wiki::settings).

**SSH tunnel**
: A port forward to the Redis host through an SSH server, made with `russh`, no `ssh` binary.
  [`TunnelError`](cachemire_core::connection::ssh_tunnel::TunnelError). See
  [core connection](cachemire_core::wiki::connection).

**Tab**
: An entry of the tab strip: a key, a search or a query. [`Tabs`](crate::tabs::Tabs). See
  [keys_and_tabs](crate::wiki::keys_and_tabs).

**Theme**
: `ui/components/theme.slint`: the `Theme` global Slint reads its colours from, filled by `src/theme/` from the
  theme picked in the settings (four colours, the rest derived). See
  [ui](crate::wiki::ui).

**Tint**
: The colour of a connection profile, shown on the connections that use it (`ProfileTint`). See [connections](crate::wiki::connections).

**Toast**
: A transient message at the pointer ("Copied"). [`toast`](crate::toast). See [errors](crate::wiki::errors).

**Walk**
: The paged scan of the keyspace that fills the tree's index. See [keys_and_tabs](crate::wiki::keys_and_tabs),
  [state](crate::wiki::state) and [core explorer](cachemire_core::wiki::explorer).

**Wiki**
: These chapters; built with `cargo wiki --open`. See the [front page](crate).

**`xtask`**
: The tooling crate (`cargo xtask i18n`, `cargo ship`, end-to-end). See [testing](crate::wiki::testing) and
  [i18n](crate::wiki::i18n).
