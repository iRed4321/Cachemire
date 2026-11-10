//! Constructors shared by the tests of more than one module, the app's included
//! (through the `test-support` feature).

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

/// A new empty directory under the system's temp directory, unique to this call.
pub fn temp_dir(name: &str) -> PathBuf {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("cachemire-test-{name}-{}-{}-{n}", std::process::id(), crate::now_ms()));
    std::fs::create_dir_all(&dir).expect("create the test directory");
    dir
}

/// Runs `future` to its end on a runtime of its own.
pub fn block_on<F: std::future::Future>(future: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread().enable_all().build().expect("test runtime").block_on(future)
}

/// The end-to-end tests' Redis server, `redis://host:port` (`cargo xtask e2e` starts one and sets it).
pub const E2E_REDIS: &str = "CACHEMIRE_E2E_REDIS";

/// The key of database 0 that numbers the databases handed out, across test processes.
const E2E_COUNTER: &str = "cachemire:e2e:databases";

/// A database of the end-to-end tests' server for one test alone, emptied, with a connection
/// to it for setting up and checking what the test does. Panics when there is no server.
pub struct TestRedis {
    /// `redis://host:port/db`: what a saved connection to this database holds
    pub url: String,
    con: redis::Connection,
}

impl TestRedis {
    pub fn claim() -> Self {
        let server = std::env::var(E2E_REDIS).unwrap_or_default();
        assert!(!server.is_empty(), "the end-to-end tests need a Redis server: run them with `cargo xtask e2e`");
        let server = server.trim_end_matches('/');
        let open = |url: &str| {
            redis::Client::open(url).and_then(|client| client.get_connection()).unwrap_or_else(|e| panic!("can't reach {url}: {e}"))
        };
        let mut control = open(&format!("{server}/0"));
        let databases: Vec<String> = redis::cmd("CONFIG").arg("GET").arg("databases").query(&mut control).unwrap_or_default();
        let databases: i64 = databases.get(1).and_then(|n| n.parse().ok()).unwrap_or(16);
        let claimed: i64 = redis::cmd("INCR").arg(E2E_COUNTER).query(&mut control).expect("number a database");
        // database 0 holds the counter; the others are handed out in turn
        let url = format!("{server}/{}", 1 + (claimed - 1) % (databases - 1));
        let mut con = open(&url);
        redis::cmd("FLUSHDB").exec(&mut con).expect("empty the database");
        Self { url, con }
    }

    /// Runs one command, such as `["HSET", "user:1", "name", "Ada"]`; panics on an error.
    pub fn query<T: redis::FromRedisValue>(&mut self, args: &[&str]) -> T {
        self.query_bytes(&args.iter().map(|arg| arg.as_bytes()).collect::<Vec<_>>())
    }

    /// [`TestRedis::query`] with arguments that needn't be UTF-8 (binary key names).
    pub fn query_bytes<T: redis::FromRedisValue>(&mut self, args: &[&[u8]]) -> T {
        let mut cmd = redis::cmd(&String::from_utf8_lossy(args[0]));
        for arg in &args[1..] {
            cmd.arg(*arg);
        }
        let shown = || args.iter().map(|arg| String::from_utf8_lossy(arg).chars().take(40).collect::<String>()).collect::<Vec<_>>();
        cmd.query(&mut self.con).unwrap_or_else(|e| panic!("{:?} failed: {e}", shown()))
    }

    /// [`TestRedis::query`] for a command whose reply doesn't matter.
    pub fn run(&mut self, args: &[&str]) {
        self.query::<redis::Value>(args);
    }

    /// This database's URL logging in as `username` (an ACL user the test made).
    pub fn url_as(&self, username: &str, password: &str) -> String {
        self.url.replacen("redis://", &format!("redis://{username}:{password}@"), 1)
    }

    /// A state in a directory of its own (see `AppState::for_tests`) whose active
    /// connection, `"e2e"`, is to this database.
    pub fn state(&self) -> crate::state::AppState {
        e2e_state(&self.url)
    }
}

/// A state in a directory of its own whose active connection, `"e2e"`, is to `url`.
pub fn e2e_state(url: &str) -> crate::state::AppState {
    let state = crate::state::AppState::for_tests(&temp_dir("e2e"));
    let payload = crate::connection::ConnectionPayloadInput {
        name: Some("End-to-end".into()),
        connection_url: Some(url.to_string()),
        ..Default::default()
    };
    state.connections.lock().create_with_id("e2e", payload).expect("save the connection");
    state.connections.lock().set_active("e2e").expect("make it active");
    state
}
