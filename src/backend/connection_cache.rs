use super::LockExt;
use rust_i18n::t;
use rustc_hash::FxHashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use redis::aio::ConnectionManager;
use tokio::sync::OnceCell;
use tokio::task::JoinSet;

use super::error::AppError;
use super::models::ConnectionItem;
use super::redis_client::{create_connection, target_of};
use super::session::{cluster_enabled, ping, Session};
use super::ssh_tunnel::SshTunnel;

/// Physical connections kept per saved connection id. Redis serves one
/// connection's commands strictly in order, so a single shared connection
/// lets a big reply head-of-line-block everything queued behind it.
const POOL_SIZE: usize = 3;

/// A pooled connection: a cheap-to-clone handle that reopens its socket
/// when it drops (see `create_connection`).
pub type RedisConnection = ConnectionManager;

pub struct ConnectionPool {
    connections: Vec<RedisConnection>,
    next: AtomicUsize,
    // the server takes `HSCAN ... NOVALUES` (until it is found not to)
    pub(super) hscan_novalues: AtomicBool,
    // the SSH tunnel the connections go through, when there is one: it lives
    // exactly as long as the pool
    _tunnel: Option<SshTunnel>,
}

impl ConnectionPool {
    /// The first connection is opened alone so bad credentials or an
    /// unreachable host fail once, quickly; the rest then open in parallel.
    async fn open(connection: &ConnectionItem, known_hosts: &Path) -> Result<Self, AppError> {
        let tunnel = match &connection.ssh {
            Some(ssh) => {
                let (host, port) = target_of(connection)?;
                Some(SshTunnel::open(ssh, &host, port, known_hosts).await?)
            }
            None => None,
        };
        let via = tunnel.as_ref().map(SshTunnel::local_port);

        let mut connections = Vec::with_capacity(POOL_SIZE);
        let mut first = create_connection(connection, via).await?;
        // a cluster spreads its keys over several servers: talking to one of them would
        // show part of the keyspace and fail on the rest, so it is refused up front
        if cluster_enabled(&mut first).await {
            return Err(AppError::Unsupported(
                t!("This server runs in cluster mode, which Cachemire doesn't support yet. Connect to a server or endpoint with cluster mode disabled.").into_owned(),
            ));
        }
        connections.push(first);

        let mut rest = JoinSet::new();
        for _ in 1..POOL_SIZE {
            let connection = connection.clone();
            rest.spawn(async move { create_connection(&connection, via).await });
        }
        while let Some(opened) = rest.join_next().await {
            connections.push(opened??);
        }
        Ok(Self { connections, next: AtomicUsize::new(0), hscan_novalues: AtomicBool::new(true), _tunnel: tunnel })
    }

    pub(super) fn pick(&self) -> RedisConnection {
        let index = self.next.fetch_add(1, Ordering::Relaxed) % self.connections.len();
        self.connections[index].clone()
    }
}

/// Caches a small pool of multiplexed connections per saved connection id,
/// so browsing/searching/querying don't pay for a fresh handshake each
/// click.
pub struct ConnectionCache {
    // one cell per id: callers racing to open the same pool share a single
    // open instead of each building their own
    entries: Mutex<FxHashMap<String, Arc<OnceCell<Arc<ConnectionPool>>>>>,
    // where SSH tunnels remember the host keys they've seen
    known_hosts: PathBuf,
}

impl ConnectionCache {
    pub fn new(known_hosts: PathBuf) -> Self {
        Self { entries: Mutex::new(FxHashMap::default()), known_hosts }
    }

    /// The connection's pool, opened on first use (see `Session::open`).
    pub(super) async fn pool(&self, connection: &ConnectionItem) -> Result<Arc<ConnectionPool>, AppError> {
        let cell = self.entries.lock_recover().entry(connection.id.clone()).or_default().clone();
        let pool = cell.get_or_try_init(|| async { ConnectionPool::open(connection, &self.known_hosts).await.map(Arc::new) }).await?;
        Ok(pool.clone())
    }

    /// Whether a pool is already open for this connection id, i.e. it was
    /// successfully connected to during this session. Never opens one.
    pub fn is_connected(&self, connection_id: &str) -> bool {
        self.entries.lock_recover().get(connection_id).is_some_and(|cell| cell.initialized())
    }

    /// Whether every connection of the id's open pool answers a PING within
    /// `timeout`; false when no pool is open.
    pub async fn responds(&self, connection_id: &str, timeout: Duration) -> bool {
        let cell = self.entries.lock_recover().get(connection_id).cloned();
        let Some(connections) = cell.as_ref().and_then(|cell| cell.get()).map(|pool| pool.connections.clone()) else {
            return false;
        };
        let mut pings = JoinSet::new();
        for mut con in connections {
            pings.spawn(async move { matches!(tokio::time::timeout(timeout, ping(&mut con)).await, Ok(Ok(()))) });
        }
        while let Some(answered) = pings.join_next().await {
            if !matches!(answered, Ok(true)) {
                return false;
            }
        }
        true
    }

    /// Opens the connection (through its SSH tunnel, if any) and pings it, keeping nothing.
    pub async fn test(&self, connection: &ConnectionItem) -> Result<(), AppError> {
        let pool = ConnectionPool::open(connection, &self.known_hosts).await?;
        Session::on(Arc::new(pool)).ping().await
    }

    pub fn invalidate(&self, connection_id: &str) {
        self.entries.lock_recover().remove(connection_id);
    }
}
