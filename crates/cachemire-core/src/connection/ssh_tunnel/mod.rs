//! An SSH tunnel to a Redis server: logs in, listens on a free local port,
//! and forwards connections through it to the Redis host — what
//! `ssh -L port:redis-host:6379` does, without an `ssh` binary.

use parking_lot::Mutex;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use russh::client::{self, Handle};
use russh::keys::known_hosts::learn_known_hosts_path;
use russh::keys::{HashAlg, PrivateKeyWithHashAlg, PublicKeyOrCertificate, check_known_hosts_path};
use tokio::net::TcpListener;
use tokio::task::{JoinHandle, JoinSet};

use crate::connection::SshSettings;
use crate::connection::tls;
use rustls::pki_types::ServerName;

/// Why the tunnel couldn't be set up.
#[derive(Debug, thiserror::Error)]
pub enum TunnelError {
    #[error("couldn't remember the SSH server's host key in {}: {source}", file.display())]
    HostKeyNotSaved { file: PathBuf, source: russh::keys::Error },
    #[error("the SSH server's host key isn't trusted yet: {0}")]
    UnknownHostKey(Box<UnknownHostKey>),
    #[error("the SSH server's host key has changed since it was first seen (recorded on line {line} of {}). If the server was reinstalled, delete that line; otherwise someone may be impersonating it.", file.display())]
    HostKeyChanged { line: usize, file: PathBuf },
    #[error("couldn't check the SSH server's host key: {0}")]
    HostKeyUnchecked(#[source] russh::keys::Error),
    #[error("couldn't decrypt the SSH private key {}: wrong or missing passphrase?", file.display())]
    KeyPassphrase { file: PathBuf },
    #[error("couldn't read the SSH private key {}: {source}", file.display())]
    KeyUnreadable { file: PathBuf, source: russh::keys::Error },
    #[error("no SSH password or private key to log in with")]
    NothingToLogInWith,
    #[error("the SSH server refused the login for {user} ({})", match (key, password) { (true, true) => "private key and password", (true, false) => "private key", _ => "password" })]
    LoginRefused { user: String, key: bool, password: bool },
    #[error("couldn't connect to the SSH server {host}:{port}: {source}")]
    Connect { host: String, port: u16, source: russh::Error },
    #[error("timed out connecting to the SSH server {host}:{port}")]
    Timeout { host: String, port: u16 },
    #[error("the SSH server couldn't reach {host}:{port}: {source}")]
    Unreachable { host: String, port: u16, source: russh::Error },
    #[error("couldn't make a secure connection to {host} through the SSH tunnel: {source}")]
    Tls { host: String, source: std::io::Error },
    #[error("couldn't open a local port for the SSH tunnel: {0}")]
    LocalPort(#[source] std::io::Error),
    #[error(transparent)]
    Ssh(#[from] russh::Error),
}

/// An SSH server whose host key is in no known-hosts file: what the user is shown to
/// decide on, and what [`HostKeys::trust`] records.
#[derive(Debug, Clone)]
pub struct UnknownHostKey {
    pub host: String,
    pub port: u16,
    /// the key's algorithm, as OpenSSH names it (`ssh-ed25519`)
    pub key_type: String,
    /// the key's SHA-256 fingerprint, OpenSSH style (`SHA256:...`)
    pub fingerprint: String,
    key: russh::keys::PublicKey,
}

impl UnknownHostKey {
    /// The key `host:port` presented, with its type and fingerprint.
    pub fn new(host: &str, port: u16, key: &russh::keys::PublicKey) -> Self {
        Self {
            host: host.to_string(),
            port,
            key_type: key.algorithm().to_string(),
            fingerprint: key.fingerprint(HashAlg::Sha256).to_string(),
            key: key.clone(),
        }
    }
}

impl std::fmt::Display for UnknownHostKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{} ({} {})", self.host, self.port, self.key_type, self.fingerprint)
    }
}

/// Where SSH servers' host keys are looked up and remembered: the app's own file, which
/// new keys are written to, and the user's `~/.ssh/known_hosts`, which is only read.
#[derive(Clone)]
pub struct HostKeys {
    app_file: PathBuf,
    user_file: Option<PathBuf>,
    accept_new: Arc<AtomicBool>,
}

impl HostKeys {
    /// Host keys kept in `app_file` alone, a new one needing the user's approval.
    pub fn new(app_file: PathBuf) -> Self {
        Self { app_file, user_file: None, accept_new: Arc::default() }
    }

    /// Also trusts the keys of `~/.ssh/known_hosts`, when the home directory is known.
    pub fn with_user_known_hosts(mut self) -> Self {
        self.user_file = dirs::home_dir().map(|home| home.join(".ssh").join("known_hosts"));
        self
    }

    /// Whether a key seen for the first time is trusted and remembered without asking.
    pub fn accepts_new(&self) -> bool {
        self.accept_new.load(Ordering::Relaxed)
    }

    pub fn set_accepts_new(&self, accept: bool) {
        self.accept_new.store(accept, Ordering::Relaxed);
    }

    /// Remembers `server`'s key in the app's file, so that it is trusted from now on.
    pub fn trust(&self, server: &UnknownHostKey) -> Result<(), TunnelError> {
        learn_known_hosts_path(&server.host, server.port, &server.key, &self.app_file)
            .map_err(|source| TunnelError::HostKeyNotSaved { file: self.app_file.clone(), source })
    }
}

/// Connecting to the SSH server and logging in must finish within this.
const SETUP_TIMEOUT: Duration = Duration::from_secs(20);

/// Checks the SSH server's host key against the known-hosts files: a listed key is trusted,
/// a changed one refused (someone may be impersonating the server), and a new one refused
/// until trusted, or remembered at once when new keys are accepted.
struct HostKeyCheck {
    host: String,
    port: u16,
    keys: HostKeys,
    // why the key was refused, for the error the caller reports
    refusal: Arc<Mutex<Option<TunnelError>>>,
}

impl client::Handler for HostKeyCheck {
    type Error = russh::Error;

    async fn check_server_key(&mut self, key: &PublicKeyOrCertificate) -> Result<bool, Self::Error> {
        let key = key.public_key();
        let refuse = |why: TunnelError| {
            *self.refusal.lock() = Some(why);
            Ok(false)
        };
        let mut listed = false;
        // (the user's file may hold lines this can't read: they count as not listing the server)
        if let Some(user_file) = &self.keys.user_file {
            match check_known_hosts_path(&self.host, self.port, &key, user_file) {
                Ok(found) => listed = found,
                Err(russh::keys::Error::KeyChanged { line }) => {
                    return refuse(TunnelError::HostKeyChanged { line, file: user_file.clone() });
                }
                Err(e) => tracing::debug!(file = %user_file.display(), "couldn't check the host key against it: {e}"),
            }
        }
        match check_known_hosts_path(&self.host, self.port, &key, &self.keys.app_file) {
            Ok(true) => Ok(true),
            Ok(false) if listed => Ok(true),
            Ok(false) => {
                let server = UnknownHostKey::new(&self.host, self.port, &key);
                if !self.keys.accepts_new() {
                    return refuse(TunnelError::UnknownHostKey(Box::new(server)));
                }
                tracing::info!(server = %format_args!("{}:{}", server.host, server.port), fingerprint = %server.fingerprint, "trusting a new SSH host key");
                match self.keys.trust(&server) {
                    Ok(()) => Ok(true),
                    Err(why) => refuse(why),
                }
            }
            Err(russh::keys::Error::KeyChanged { line }) => refuse(TunnelError::HostKeyChanged { line, file: self.keys.app_file.clone() }),
            Err(e) => refuse(TunnelError::HostKeyUnchecked(e)),
        }
    }
}

/// `~` or `~/…` at the start of a path stands for the home directory.
fn expand_home(path: &str) -> PathBuf {
    match path.strip_prefix('~') {
        Some("") => dirs::home_dir().unwrap_or_else(|| PathBuf::from(path)),
        Some(rest) if rest.starts_with(['/', '\\']) => dirs::home_dir().map_or_else(|| PathBuf::from(path), |h| h.join(&rest[1..])),
        _ => PathBuf::from(path),
    }
}

/// Whether the private key in `text` is protected by a passphrase, whatever its format
/// (OpenSSH, PKCS#8, PEM with encryption headers, PuTTY), judged without decrypting it.
fn key_is_encrypted(text: &str) -> bool {
    let text = text.trim_start();
    if text.starts_with("PuTTY-User-Key-File-") {
        return text.lines().any(|line| line.strip_prefix("Encryption:").is_some_and(|how| how.trim() != "none"));
    }
    if text.contains("-----BEGIN ENCRYPTED PRIVATE KEY-----") {
        return true;
    }
    if text.contains("-----BEGIN OPENSSH PRIVATE KEY-----") {
        return russh::keys::ssh_key::PrivateKey::from_openssh(text).is_ok_and(|key| key.is_encrypted());
    }
    text.lines().any(|line| line.starts_with("DEK-Info:") || (line.starts_with("Proc-Type:") && line.contains("ENCRYPTED")))
}

/// Logs in with the private key when there is one, then with the password.
async fn authenticate(session: &mut Handle<HostKeyCheck>, ssh: &SshSettings) -> Result<(), TunnelError> {
    let (mut tried_key, mut tried_password) = (false, false);

    if !ssh.private_key.trim().is_empty() {
        let path = expand_home(ssh.private_key.trim());
        let passphrase = if ssh.passphrase.is_empty() { None } else { Some(ssh.passphrase.as_str()) };
        let unreadable = |source| TunnelError::KeyUnreadable { file: path.clone(), source };
        let text = std::fs::read_to_string(&path).map_err(|e| unreadable(e.into()))?;
        let key = russh::keys::decode_secret_key(&text, passphrase).map_err(|source| {
            // a key that is encrypted and won't open needs its (right) passphrase
            if matches!(source, russh::keys::Error::KeyIsEncrypted) || key_is_encrypted(&text) {
                TunnelError::KeyPassphrase { file: path.clone() }
            } else {
                unreadable(source)
            }
        })?;
        // RSA keys sign with whichever hash the server supports
        let hash: Option<HashAlg> = session.best_supported_rsa_hash().await?.flatten();
        let result = session.authenticate_publickey(&ssh.username, PrivateKeyWithHashAlg::new(Arc::new(key), hash)).await?;
        if result.success() {
            return Ok(());
        }
        tried_key = true;
    }

    if !ssh.password.is_empty() {
        let result = session.authenticate_password(&ssh.username, &ssh.password).await?;
        if result.success() {
            return Ok(());
        }
        tried_password = true;
    }

    if !tried_key && !tried_password {
        return Err(TunnelError::NothingToLogInWith);
    }
    Err(TunnelError::LoginRefused { user: ssh.username.clone(), key: tried_key, password: tried_password })
}

/// Opens the SSH session and logs in.
async fn connect(ssh: &SshSettings, keys: &HostKeys) -> Result<Handle<HostKeyCheck>, TunnelError> {
    let refusal = Arc::new(Mutex::new(None));
    let handler = HostKeyCheck { host: ssh.host.clone(), port: ssh.port, keys: keys.clone(), refusal: refusal.clone() };
    let config = Arc::new(client::Config {
        // the tunnel is long-lived and mostly idle: keep it from being dropped
        keepalive_interval: Some(Duration::from_secs(30)),
        inactivity_timeout: None,
        ..Default::default()
    });

    let mut session = client::connect(config, (ssh.host.as_str(), ssh.port), handler).await.map_err(|source| {
        // a refused host key surfaces as a generic key error: say why instead
        refusal.lock().take().unwrap_or_else(|| TunnelError::Connect { host: ssh.host.clone(), port: ssh.port, source })
    })?;
    authenticate(&mut session, ssh).await?;
    Ok(session)
}

/// Accepts connections on the local end and forwards each through its own
/// channel of the SSH session. Owns the session and every forwarding task, so
/// dropping this future (aborting the task running it) closes them all.
async fn forward_loop(
    listener: TcpListener,
    session: Arc<Handle<HostKeyCheck>>,
    host: String,
    port: u16,
    tls: Option<ServerName<'static>>,
) {
    let mut forwards = JoinSet::new();
    while let Ok((mut socket, peer)) = listener.accept().await {
        let session = session.clone();
        let host = host.clone();
        let tls = tls.clone();
        forwards.spawn(async move {
            let Ok(channel) = session.channel_open_direct_tcpip(host, u32::from(port), peer.ip().to_string(), u32::from(peer.port())).await
            else {
                return; // the client sees its connection closed
            };
            let mut stream = channel.into_stream();
            let _ = match tls {
                Some(name) => match tls::connector().connect(name, stream).await {
                    Ok(mut secured) => tokio::io::copy_bidirectional(&mut socket, &mut secured).await,
                    Err(error) => {
                        tracing::warn!(%error, "TLS handshake through the SSH tunnel failed");
                        return;
                    }
                },
                None => tokio::io::copy_bidirectional(&mut socket, &mut stream).await,
            };
        });
    }
}

pub struct SshTunnel {
    local_port: u16,
    forwarding: JoinHandle<()>,
    session: Arc<Handle<HostKeyCheck>>,
}

impl SshTunnel {
    /// Sets up a tunnel to `target_host:target_port` (as the SSH server sees
    /// it) and checks that the SSH server can reach it, so a wrong host or port
    /// is reported here rather than as a bare "connection closed" later. With a `tls`
    /// name, each forwarded connection is secured and checked against that name.
    pub async fn open(
        ssh: &SshSettings,
        target_host: &str,
        target_port: u16,
        tls: Option<ServerName<'static>>,
        keys: &HostKeys,
    ) -> Result<SshTunnel, TunnelError> {
        let session = tokio::time::timeout(SETUP_TIMEOUT, connect(ssh, keys))
            .await
            .map_err(|_| TunnelError::Timeout { host: ssh.host.clone(), port: ssh.port })??;

        let probe = session
            .channel_open_direct_tcpip(target_host, u32::from(target_port), "127.0.0.1", 0)
            .await
            .map_err(|source| TunnelError::Unreachable { host: target_host.to_string(), port: target_port, source })?;
        match &tls {
            Some(name) => {
                tls::connector()
                    .connect(name.clone(), probe.into_stream())
                    .await
                    .map_err(|source| TunnelError::Tls { host: target_host.to_string(), source })?;
            }
            None => drop(probe),
        }

        let listener = TcpListener::bind(("127.0.0.1", 0)).await.map_err(TunnelError::LocalPort)?;
        let local_port = listener.local_addr().map_err(TunnelError::LocalPort)?.port();
        let session = Arc::new(session);
        let forwarding = tokio::spawn(forward_loop(listener, session.clone(), target_host.to_string(), target_port, tls));
        Ok(SshTunnel { local_port, forwarding, session })
    }

    /// Whether the SSH session still stands: once the server or the network drops it
    /// (its keepalives unanswered), nothing goes through this tunnel any more.
    pub fn is_alive(&self) -> bool {
        !self.session.is_closed() && !self.forwarding.is_finished()
    }

    /// The port on this machine (`127.0.0.1`) that leads to the Redis server.
    pub fn local_port(&self) -> u16 {
        self.local_port
    }
}

impl Drop for SshTunnel {
    fn drop(&mut self) {
        self.forwarding.abort();
    }
}

#[cfg(test)]
mod tests;
