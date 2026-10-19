//! An SSH tunnel to a Redis server: logs in, listens on a free local port,
//! and forwards connections through it to the Redis host — what
//! `ssh -L port:redis-host:6379` does, without an `ssh` binary.

use parking_lot::Mutex;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use russh::client::{self, Handle};
use russh::keys::known_hosts::learn_known_hosts_path;
use russh::keys::{HashAlg, PrivateKeyWithHashAlg, PublicKeyOrCertificate, check_known_hosts_path};
use tokio::net::TcpListener;
use tokio::task::{JoinHandle, JoinSet};

use crate::connection::SshSettings;

/// Why the tunnel couldn't be set up.
#[derive(Debug, thiserror::Error)]
pub enum TunnelError {
    #[error("couldn't remember the SSH server's host key in {}: {source}", file.display())]
    HostKeyNotSaved { file: PathBuf, source: russh::keys::Error },
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
    #[error("couldn't open a local port for the SSH tunnel: {0}")]
    LocalPort(#[source] std::io::Error),
    #[error(transparent)]
    Ssh(#[from] russh::Error),
}

/// Connecting to the SSH server and logging in must finish within this.
const SETUP_TIMEOUT: Duration = Duration::from_secs(20);

/// Checks the SSH server's host key against an OpenSSH-format `known_hosts`
/// file: a server seen for the first time is trusted and remembered; a
/// changed key is refused (it can mean someone is impersonating the server).
struct HostKeyCheck {
    host: String,
    port: u16,
    known_hosts: PathBuf,
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
        match check_known_hosts_path(&self.host, self.port, &key, &self.known_hosts) {
            Ok(true) => Ok(true),
            Ok(false) => {
                // first time this server is seen
                match learn_known_hosts_path(&self.host, self.port, &key, &self.known_hosts) {
                    Ok(()) => Ok(true),
                    Err(source) => refuse(TunnelError::HostKeyNotSaved { file: self.known_hosts.clone(), source }),
                }
            }
            Err(russh::keys::Error::KeyChanged { line }) => refuse(TunnelError::HostKeyChanged { line, file: self.known_hosts.clone() }),
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

/// Logs in with the private key when there is one, then with the password.
async fn authenticate(session: &mut Handle<HostKeyCheck>, ssh: &SshSettings) -> Result<(), TunnelError> {
    let (mut tried_key, mut tried_password) = (false, false);

    if !ssh.private_key.trim().is_empty() {
        let path = expand_home(ssh.private_key.trim());
        let passphrase = if ssh.passphrase.is_empty() { None } else { Some(ssh.passphrase.as_str()) };
        let key = russh::keys::load_secret_key(&path, passphrase).map_err(|source| {
            // an encrypted key with the wrong (or no) passphrase fails in the crypto layer
            let why = source.to_string().to_lowercase();
            if (passphrase.is_none() && why.contains("encrypted")) || why.contains("cryptographic") {
                TunnelError::KeyPassphrase { file: path.clone() }
            } else {
                TunnelError::KeyUnreadable { file: path.clone(), source }
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
async fn connect(ssh: &SshSettings, known_hosts: &Path) -> Result<Handle<HostKeyCheck>, TunnelError> {
    let refusal = Arc::new(Mutex::new(None));
    let handler = HostKeyCheck { host: ssh.host.clone(), port: ssh.port, known_hosts: known_hosts.to_path_buf(), refusal: refusal.clone() };
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
async fn forward_loop(listener: TcpListener, session: Handle<HostKeyCheck>, host: String, port: u16) {
    let session = Arc::new(session);
    let mut forwards = JoinSet::new();
    while let Ok((mut socket, peer)) = listener.accept().await {
        let session = session.clone();
        let host = host.clone();
        forwards.spawn(async move {
            let Ok(channel) = session.channel_open_direct_tcpip(host, u32::from(port), peer.ip().to_string(), u32::from(peer.port())).await
            else {
                return; // the client sees its connection closed
            };
            let mut stream = channel.into_stream();
            let _ = tokio::io::copy_bidirectional(&mut socket, &mut stream).await;
        });
    }
}

pub struct SshTunnel {
    local_port: u16,
    forwarding: JoinHandle<()>,
}

impl SshTunnel {
    /// Sets up a tunnel to `target_host:target_port` (as the SSH server sees
    /// it) and checks that the SSH server can reach it, so a wrong host or port
    /// is reported here rather than as a bare "connection closed" later.
    pub async fn open(ssh: &SshSettings, target_host: &str, target_port: u16, known_hosts: &Path) -> Result<SshTunnel, TunnelError> {
        let session = tokio::time::timeout(SETUP_TIMEOUT, connect(ssh, known_hosts))
            .await
            .map_err(|_| TunnelError::Timeout { host: ssh.host.clone(), port: ssh.port })??;

        let probe = session
            .channel_open_direct_tcpip(target_host, u32::from(target_port), "127.0.0.1", 0)
            .await
            .map_err(|source| TunnelError::Unreachable { host: target_host.to_string(), port: target_port, source })?;
        drop(probe);

        let listener = TcpListener::bind(("127.0.0.1", 0)).await.map_err(TunnelError::LocalPort)?;
        let local_port = listener.local_addr().map_err(TunnelError::LocalPort)?.port();
        let forwarding = tokio::spawn(forward_loop(listener, session, target_host.to_string(), target_port));
        Ok(SshTunnel { local_port, forwarding })
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
