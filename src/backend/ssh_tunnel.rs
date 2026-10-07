//! An SSH tunnel to a Redis server: logs in, listens on a free local port,
//! and forwards connections through it to the Redis host — what
//! `ssh -L port:redis-host:6379` does, without an `ssh` binary.

use super::LockExt;
use rust_i18n::t;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use russh::client::{self, Handle};
use russh::keys::known_hosts::learn_known_hosts_path;
use russh::keys::{check_known_hosts_path, HashAlg, PrivateKeyWithHashAlg, PublicKeyOrCertificate};
use tokio::net::TcpListener;
use tokio::task::{JoinHandle, JoinSet};

use super::error::AppError;
use super::models::SshSettings;

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
    refusal: Arc<Mutex<Option<String>>>,
}

impl client::Handler for HostKeyCheck {
    type Error = russh::Error;

    async fn check_server_key(&mut self, key: &PublicKeyOrCertificate) -> Result<bool, Self::Error> {
        let key = key.public_key();
        let refuse = |why: String| {
            *self.refusal.lock_recover() = Some(why);
            Ok(false)
        };
        match check_known_hosts_path(&self.host, self.port, &key, &self.known_hosts) {
            Ok(true) => Ok(true),
            Ok(false) => {
                // first time this server is seen
                match learn_known_hosts_path(&self.host, self.port, &key, &self.known_hosts) {
                    Ok(()) => Ok(true),
                    Err(e) => refuse(
                        t!("couldn't remember the SSH server's host key in %{file}: %{error}", file = self.known_hosts.display(), error = e).into_owned(),
                    ),
                }
            }
            Err(russh::keys::Error::KeyChanged { line }) => refuse(
                t!(
                    "the SSH server's host key has changed since it was first seen (recorded on line %{line} of %{file}). If the server was reinstalled, delete that line; otherwise someone may be impersonating it.",
                    line = line,
                    file = self.known_hosts.display()
                )
                .into_owned(),
            ),
            Err(e) => refuse(t!("couldn't check the SSH server's host key: %{error}", error = e).into_owned()),
        }
    }
}

/// `~` or `~/…` at the start of a path stands for the home directory.
fn expand_home(path: &str) -> PathBuf {
    let home = || std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from);
    match path.strip_prefix('~') {
        Some("") => home().unwrap_or_else(|| PathBuf::from(path)),
        Some(rest) if rest.starts_with(['/', '\\']) => home().map_or_else(|| PathBuf::from(path), |h| h.join(&rest[1..])),
        _ => PathBuf::from(path),
    }
}

fn tunnel_error(error: russh::Error) -> AppError {
    AppError::Tunnel(error.to_string())
}

/// Logs in with the private key when there is one, then with the password.
async fn authenticate(session: &mut Handle<HostKeyCheck>, ssh: &SshSettings) -> Result<(), AppError> {
    let mut tried = Vec::new();

    if !ssh.private_key.trim().is_empty() {
        let path = expand_home(ssh.private_key.trim());
        let passphrase = if ssh.passphrase.is_empty() { None } else { Some(ssh.passphrase.as_str()) };
        let key = russh::keys::load_secret_key(&path, passphrase).map_err(|e| {
            let why = e.to_string();
            // an encrypted key with the wrong (or no) passphrase fails in the crypto layer
            let why_lower = why.to_lowercase();
            AppError::Tunnel(if (passphrase.is_none() && why_lower.contains("encrypted")) || why_lower.contains("cryptographic") {
                t!("couldn't decrypt the SSH private key %{file}: wrong or missing passphrase?", file = path.display()).into_owned()
            } else {
                t!("couldn't read the SSH private key %{file}: %{error}", file = path.display(), error = why).into_owned()
            })
        })?;
        // RSA keys sign with whichever hash the server supports
        let hash: Option<HashAlg> = session.best_supported_rsa_hash().await.map_err(tunnel_error)?.flatten();
        let result = session
            .authenticate_publickey(&ssh.username, PrivateKeyWithHashAlg::new(Arc::new(key), hash))
            .await
            .map_err(tunnel_error)?;
        if result.success() {
            return Ok(());
        }
        tried.push(t!("private key").into_owned());
    }

    if !ssh.password.is_empty() {
        let result = session.authenticate_password(&ssh.username, &ssh.password).await.map_err(tunnel_error)?;
        if result.success() {
            return Ok(());
        }
        tried.push(t!("password").into_owned());
    }

    if tried.is_empty() {
        return Err(AppError::Tunnel(t!("no SSH password or private key to log in with").into_owned()));
    }
    Err(AppError::Tunnel(
        t!("the SSH server refused the login for %{user} (%{tried})", user = ssh.username, tried = tried.join(&format!(" {} ", t!("and")))).into_owned(),
    ))
}

/// Opens the SSH session and logs in.
async fn connect(ssh: &SshSettings, known_hosts: &Path) -> Result<Handle<HostKeyCheck>, AppError> {
    let refusal = Arc::new(Mutex::new(None));
    let handler = HostKeyCheck { host: ssh.host.clone(), port: ssh.port, known_hosts: known_hosts.to_path_buf(), refusal: refusal.clone() };
    let config = Arc::new(client::Config {
        // the tunnel is long-lived and mostly idle: keep it from being dropped
        keepalive_interval: Some(Duration::from_secs(30)),
        inactivity_timeout: None,
        ..Default::default()
    });

    let mut session = client::connect(config, (ssh.host.as_str(), ssh.port), handler).await.map_err(|e| {
        // a refused host key surfaces as a generic key error: say why instead
        AppError::Tunnel(match refusal.lock_recover().take() {
            Some(why) => why,
            None => t!("couldn't connect to the SSH server %{host}:%{port}: %{error}", host = ssh.host, port = ssh.port, error = e).into_owned(),
        })
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
            let Ok(channel) = session.channel_open_direct_tcpip(host, u32::from(port), peer.ip().to_string(), u32::from(peer.port())).await else {
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
    pub async fn open(ssh: &SshSettings, target_host: &str, target_port: u16, known_hosts: &Path) -> Result<SshTunnel, AppError> {
        let session = tokio::time::timeout(SETUP_TIMEOUT, connect(ssh, known_hosts))
            .await
            .map_err(|_| AppError::Tunnel(t!("timed out connecting to the SSH server %{host}:%{port}", host = ssh.host, port = ssh.port).into_owned()))??;

        let probe = session
            .channel_open_direct_tcpip(target_host, u32::from(target_port), "127.0.0.1", 0)
            .await
            .map_err(|e| AppError::Tunnel(t!("the SSH server couldn't reach %{host}:%{port}: %{error}", host = target_host, port = target_port, error = e).into_owned()))?;
        drop(probe);

        let listener = TcpListener::bind(("127.0.0.1", 0)).await.map_err(|e| AppError::Tunnel(t!("couldn't open a local port for the SSH tunnel: %{error}", error = e).into_owned()))?;
        let local_port = listener.local_addr().map_err(|e| AppError::Tunnel(e.to_string()))?.port();
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
mod tests {
    use super::*;

    #[test]
    fn a_leading_tilde_is_the_home_directory() {
        let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from).unwrap();
        assert_eq!(expand_home("~"), home);
        assert_eq!(expand_home("~/.ssh/id_ed25519"), home.join(".ssh/id_ed25519"));
        assert_eq!(expand_home("/etc/key"), PathBuf::from("/etc/key"));
        assert_eq!(expand_home("~other/key"), PathBuf::from("~other/key"));
        assert_eq!(expand_home("relative/key"), PathBuf::from("relative/key"));
    }
}
