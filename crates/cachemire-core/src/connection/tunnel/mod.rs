//! A tunnel to a Redis server: a free local port whose connections a transport (an SSH
//! session, a Kubernetes cluster) carries to the server, and, for a cluster, one more
//! local port per node, opened on demand through the same transport.

use parking_lot::Mutex;
use rustc_hash::FxHashMap;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::{Arc, Weak};

use rustls::pki_types::ServerName;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpListener;
use tokio::task::{JoinHandle, JoinSet};

use crate::connection::tls;

/// A byte stream to the far end, whatever carries it.
pub trait TunnelStream: AsyncRead + AsyncWrite + Send + Unpin {}
impl<T: AsyncRead + AsyncWrite + Send + Unpin> TunnelStream for T {}

pub type BoxedStream = Box<dyn TunnelStream>;
pub type DialFuture<'a> = Pin<Box<dyn Future<Output = std::io::Result<BoxedStream>> + Send + 'a>>;

/// A transport: opens a stream to `host:port` as the far end of the tunnel sees it.
pub trait Dialer: Send + Sync + 'static {
    fn dial<'a>(&'a self, host: &'a str, port: u16) -> DialFuture<'a>;

    /// False once the transport is gone for good (an SSH session the server dropped).
    fn is_alive(&self) -> bool {
        true
    }
}

/// Accepts connections on the local end and forwards each through its own stream of
/// the transport, secured with `tls` when given. Owns every forwarding task, so
/// dropping this future (aborting the task running it) closes them all.
async fn forward_loop(listener: TcpListener, dialer: Arc<dyn Dialer>, host: String, port: u16, tls: Option<ServerName<'static>>) {
    let mut forwards = JoinSet::new();
    while let Ok((mut socket, _)) = listener.accept().await {
        let dialer = dialer.clone();
        let host = host.clone();
        let tls = tls.clone();
        forwards.spawn(async move {
            let mut stream = match dialer.dial(&host, port).await {
                Ok(stream) => stream,
                Err(error) => {
                    tracing::warn!(%error, target = %format_args!("{host}:{port}"), "couldn't open a stream through the tunnel");
                    return; // the client sees its connection closed
                }
            };
            let _ = match tls {
                Some(name) => match tls::connector().connect(name, stream).await {
                    Ok(mut secured) => tokio::io::copy_bidirectional(&mut socket, &mut secured).await,
                    Err(error) => {
                        tracing::warn!(%error, "TLS handshake through the tunnel failed");
                        return;
                    }
                },
                None => tokio::io::copy_bidirectional(&mut socket, &mut stream).await,
            };
        });
    }
}

/// The local ports opened on demand for other targets of the same transport (a
/// cluster's nodes), plain TCP, one per `host:port`, with the tasks serving them.
struct Forwards {
    dialer: Arc<dyn Dialer>,
    ports: Mutex<FxHashMap<(String, u16), u16>>,
    tasks: Mutex<Vec<JoinHandle<()>>>,
}

impl Forwards {
    /// The local port leading to `host:port` (as the far end sees it), opened on first use.
    async fn port_to(&self, host: &str, port: u16) -> std::io::Result<u16> {
        if let Some(&local) = self.ports.lock().get(&(host.to_string(), port)) {
            return Ok(local);
        }
        let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
        let local = listener.local_addr()?.port();
        let mut ports = self.ports.lock();
        // another caller may have opened one meanwhile: theirs is kept, this listener dropped
        if let Some(&theirs) = ports.get(&(host.to_string(), port)) {
            return Ok(theirs);
        }
        ports.insert((host.to_string(), port), local);
        self.tasks.lock().push(tokio::spawn(forward_loop(listener, self.dialer.clone(), host.to_string(), port, None)));
        tracing::debug!(target = %format_args!("{host}:{port}"), local_port = local, "tunnel forward opened");
        Ok(local)
    }
}

/// Resolves a cluster node's address to a local port forwarded through the tunnel, so
/// the Redis client reaches every node through it; the TLS handshake, if any, stays
/// the client's, with the node's real name.
pub struct TunnelResolver(Weak<Forwards>);

impl redis::io::AsyncDNSResolver for TunnelResolver {
    fn resolve<'a, 'b: 'a>(&'a self, host: &'b str, port: u16) -> redis::RedisFuture<'a, Box<dyn Iterator<Item = SocketAddr> + Send + 'a>> {
        Box::pin(async move {
            let closed = || std::io::Error::new(std::io::ErrorKind::NotConnected, "the tunnel is closed");
            let forwards = self.0.upgrade().ok_or_else(closed)?;
            let local = forwards.port_to(host, port).await?;
            let addresses: Box<dyn Iterator<Item = SocketAddr> + Send> =
                Box::new(std::iter::once(SocketAddr::from(([127, 0, 0, 1], local))));
            Ok(addresses)
        })
    }
}

pub struct Tunnel {
    local_port: u16,
    forwarding: JoinHandle<()>,
    forwards: Arc<Forwards>,
}

impl Tunnel {
    /// Listens on a free local port and forwards what connects to it to `host:port`
    /// through `dialer`, each connection secured and checked against `tls` when given.
    pub(crate) async fn start(dialer: Arc<dyn Dialer>, host: &str, port: u16, tls: Option<ServerName<'static>>) -> std::io::Result<Tunnel> {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
        let local_port = listener.local_addr()?.port();
        let forwarding = tokio::spawn(forward_loop(listener, dialer.clone(), host.to_string(), port, tls));
        let forwards = Arc::new(Forwards { dialer, ports: Mutex::default(), tasks: Mutex::default() });
        Ok(Tunnel { local_port, forwarding, forwards })
    }

    /// Whether the transport still stands and the local end still forwards.
    pub fn is_alive(&self) -> bool {
        !self.forwarding.is_finished() && self.forwards.dialer.is_alive()
    }

    /// The port on this machine (`127.0.0.1`) that leads to the Redis server.
    pub fn local_port(&self) -> u16 {
        self.local_port
    }

    /// What a cluster client resolves its nodes' addresses with, to reach each of them
    /// through this tunnel (plain TCP: the client secures it). Useless once it is dropped.
    pub fn resolver(&self) -> TunnelResolver {
        TunnelResolver(Arc::downgrade(&self.forwards))
    }
}

impl Drop for Tunnel {
    fn drop(&mut self) {
        self.forwarding.abort();
        for task in self.forwards.tasks.lock().drain(..) {
            task.abort();
        }
    }
}
