use parking_lot::Mutex;
use rustc_hash::FxHashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use redis::aio::ConnectionManager;
use redis::cluster_async::ClusterConnection;
use tokio::sync::OnceCell;
use tokio::task::JoinSet;

use crate::connection::ConnectionItem;
use crate::connection::client::{build_cluster_client, create_cluster_connection, create_connection, target_of, tls_name_of};
use crate::connection::kube_tunnel;
use crate::connection::ssh_tunnel::{self, HostKeys};
use crate::connection::tunnel::Tunnel;
use crate::error::{Error, Timeout};
use crate::session::{Session, cluster_enabled, ping};

/// Physical connections kept per saved connection id: the first for long reads, the
/// others for what clicks ask. Redis serves one connection's commands strictly in
/// order, so a big reply on a shared one would hold up everything queued behind it.
const POOL_SIZE: usize = 3;

/// Opening a pool (its SSH tunnel, then its connections) must finish within this.
const OPEN_TIMEOUT: Duration = Duration::from_secs(30);

/// A pooled connection: a cheap-to-clone handle on one server, or on every node of a
/// cluster, which reopens its sockets when they drop (see `connection::client`).
#[derive(Clone)]
pub enum RedisConnection {
    Server(ConnectionManager),
    Cluster(ClusterConnection),
}

impl redis::aio::ConnectionLike for RedisConnection {
    fn req_packed_command<'a>(&'a mut self, cmd: &'a redis::Cmd) -> redis::RedisFuture<'a, redis::Value> {
        match self {
            RedisConnection::Server(con) => con.req_packed_command(cmd),
            RedisConnection::Cluster(con) => con.req_packed_command(cmd),
        }
    }

    fn req_packed_commands<'a>(
        &'a mut self,
        pipeline: &'a redis::Pipeline,
        offset: usize,
        count: usize,
    ) -> redis::RedisFuture<'a, Vec<redis::Value>> {
        match self {
            RedisConnection::Server(con) => con.req_packed_commands(pipeline, offset, count),
            RedisConnection::Cluster(con) => con.req_packed_commands(pipeline, offset, count),
        }
    }

    fn get_db(&self) -> i64 {
        match self {
            RedisConnection::Server(con) => con.get_db(),
            RedisConnection::Cluster(con) => con.get_db(),
        }
    }
}

pub struct ConnectionPool {
    connections: Vec<RedisConnection>,
    next: AtomicUsize,
    // the connections are to a cluster's nodes, not to one server
    cluster: bool,
    // the server takes `HSCAN ... NOVALUES` (until it is found not to)
    pub(crate) hscan_novalues: AtomicBool,
    // the tunnel the connections go through, when there is one: it lives
    // exactly as long as the pool
    _tunnel: Option<Tunnel>,
}

impl ConnectionPool {
    /// The first connection is opened alone so bad credentials or an unreachable host
    /// fail once, quickly; the rest then open in parallel. A cluster gets cluster connections.
    async fn open(connection: &ConnectionItem, host_keys: &HostKeys) -> Result<Self, Error> {
        let tunnel = match (&connection.kubernetes, &connection.ssh) {
            (Some(kube), _) => {
                let (host, port) = target_of(connection)?;
                Some(kube_tunnel::open(kube, &host, port, tls_name_of(connection)?).await?)
            }
            (None, Some(ssh)) => {
                let (host, port) = target_of(connection)?;
                let tunnel = ssh_tunnel::open(ssh, &host, port, tls_name_of(connection)?, host_keys).await?;
                tracing::info!(ssh_server = %format_args!("{}:{}", ssh.host, ssh.port), local_port = tunnel.local_port(), "SSH tunnel opened");
                Some(tunnel)
            }
            (None, None) => None,
        };
        let via = tunnel.as_ref().map(Tunnel::local_port);

        let mut first = create_connection(connection, via).await?;
        let cluster = cluster_enabled(&mut first).await;
        let connections = if cluster {
            drop(first);
            let client = build_cluster_client(connection, tunnel.as_ref().map(Tunnel::resolver))?;
            let first = create_cluster_connection(&client).await?;
            tracing::info!(name = %connection.name, "connected to a cluster");
            open_rest(RedisConnection::Cluster(first), move || {
                let client = client.clone();
                async move { create_cluster_connection(&client).await.map(RedisConnection::Cluster) }
            })
            .await?
        } else {
            let connection = connection.clone();
            open_rest(RedisConnection::Server(first), move || {
                let connection = connection.clone();
                async move { create_connection(&connection, via).await.map(RedisConnection::Server) }
            })
            .await?
        };
        Ok(Self { connections, next: AtomicUsize::new(0), cluster, hscan_novalues: AtomicBool::new(true), _tunnel: tunnel })
    }

    /// Whether the connections are to a cluster's nodes.
    pub(crate) fn is_cluster(&self) -> bool {
        self.cluster
    }

    /// False once its tunnel is down: its connections can't reach the server again.
    fn is_alive(&self) -> bool {
        self._tunnel.as_ref().is_none_or(Tunnel::is_alive)
    }

    /// [`ConnectionPool::open`] within [`OPEN_TIMEOUT`], plus the time a temporary relay pod may take to start.
    async fn open_in_time(connection: &ConnectionItem, host_keys: &HostKeys) -> Result<Self, Error> {
        let starts_relay = connection.kubernetes.as_ref().is_some_and(|kube| kube.temporary_relay);
        let limit = if starts_relay { OPEN_TIMEOUT + kube_tunnel::RELAY_START_TIMEOUT } else { OPEN_TIMEOUT };
        tokio::time::timeout(limit, Self::open(connection, host_keys)).await.map_err(|_| Timeout::Connect { seconds: limit.as_secs() })?
    }

    /// The next connection for a short read or write, in turn.
    pub(crate) fn pick(&self) -> RedisConnection {
        let index = 1 + self.next.fetch_add(1, Ordering::Relaxed) % (self.connections.len() - 1);
        self.connections[index].clone()
    }

    /// The connection for long reads (the walk, searches, exports, queries).
    pub(crate) fn background(&self) -> RedisConnection {
        self.connections[0].clone()
    }
}

/// The pool's connections: `first`, then the others `open` makes, all at once.
async fn open_rest<F, O>(first: RedisConnection, open: F) -> Result<Vec<RedisConnection>, Error>
where
    F: Fn() -> O,
    O: Future<Output = Result<RedisConnection, Error>> + Send + 'static,
{
    let mut connections = Vec::with_capacity(POOL_SIZE);
    connections.push(first);
    let mut rest = JoinSet::new();
    for _ in 1..POOL_SIZE {
        rest.spawn(open());
    }
    while let Some(opened) = rest.join_next().await {
        connections.push(opened??);
    }
    Ok(connections)
}

/// Caches a small pool of multiplexed connections per saved connection id,
/// so browsing/searching/querying don't pay for a fresh handshake each
/// click.
pub struct ConnectionCache {
    // one cell per id: callers racing to open the same pool share a single
    // open instead of each building their own
    entries: Mutex<FxHashMap<String, Arc<OnceCell<Arc<ConnectionPool>>>>>,
    // where SSH tunnels look up and remember the host keys they see
    host_keys: HostKeys,
}

impl ConnectionCache {
    pub fn new(host_keys: HostKeys) -> Self {
        Self { entries: Mutex::new(FxHashMap::default()), host_keys }
    }

    pub fn host_keys(&self) -> &HostKeys {
        &self.host_keys
    }

    /// The connection's pool, opened on first use (see `Session::open`), and opened
    /// again once its SSH tunnel is down.
    pub(crate) async fn pool(&self, connection: &ConnectionItem) -> Result<Arc<ConnectionPool>, Error> {
        let cell = {
            let mut entries = self.entries.lock();
            let cell = entries.entry(connection.id.clone()).or_default();
            if cell.get().is_some_and(|pool| !pool.is_alive()) {
                tracing::info!(name = %connection.name, "the SSH tunnel is down; opening it again");
                *cell = Arc::default();
            }
            cell.clone()
        };
        let pool = cell.get_or_try_init(|| async { ConnectionPool::open_in_time(connection, &self.host_keys).await.map(Arc::new) }).await?;
        Ok(pool.clone())
    }

    /// Whether a pool is already open for this connection id, i.e. it was
    /// successfully connected to during this session. Never opens one.
    pub fn is_connected(&self, connection_id: &str) -> bool {
        self.entries.lock().get(connection_id).is_some_and(|cell| cell.initialized())
    }

    /// Whether every connection of the id's open pool answers a PING within
    /// `timeout`; false when no pool is open.
    pub async fn responds(&self, connection_id: &str, timeout: Duration) -> bool {
        let cell = self.entries.lock().get(connection_id).cloned();
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
    pub async fn test(&self, connection: &ConnectionItem) -> Result<(), Error> {
        let pool = ConnectionPool::open_in_time(connection, &self.host_keys).await?;
        Session::on(Arc::new(pool)).ping().await
    }

    pub fn invalidate(&self, connection_id: &str) {
        self.entries.lock().remove(connection_id);
    }
}
