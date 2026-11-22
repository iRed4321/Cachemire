//! `cargo xtask e2e`: the end-to-end tests, against a throwaway Redis server started for
//! them: a `redis-server` (or Valkey, Memurai) on the PATH, else a Docker container, else
//! a `redis-server` in WSL. The tests read where it is from `CACHEMIRE_E2E_REDIS`.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use crate::tools::{Result, cargo, repo_root, run, works};

/// What the tests read the server's URL from (see cachemire-core's `test_support`).
const E2E_REDIS: &str = "CACHEMIRE_E2E_REDIS";

/// The Docker image used when there is no server to run on this machine.
const IMAGE: &str = "redis:7.4-alpine";

/// Each test takes a database of its own, so the server gets plenty of them.
const SERVER_ARGS: [&str; 6] = ["--databases", "10000", "--save", "", "--appendonly", "no"];

/// The tests' nextest filter: every test inside an `e2e` module.
const FILTER: &str = "test(/(^|::)e2e::/)";

/// A server started for the tests, stopped when dropped.
enum Server {
    Process(Child),
    Container(String),
}

impl Drop for Server {
    fn drop(&mut self) {
        match self {
            Server::Process(child) => {
                let _ = child.kill();
                let _ = child.wait();
            }
            Server::Container(name) => {
                let _ = Command::new("docker").args(["rm", "--force", name.as_str()]).stdout(Stdio::null()).stderr(Stdio::null()).status();
            }
        }
    }
}

pub fn run_task(extra: Vec<String>) -> Result<()> {
    let given = std::env::var(E2E_REDIS).ok().filter(|url| !url.is_empty());
    let (url, server) = match given {
        Some(url) => {
            println!("Using the Redis server {E2E_REDIS} names: {url}");
            (url, None)
        }
        None => {
            let port = free_port()?;
            let server = start(port)?;
            let url = format!("redis://127.0.0.1:{port}");
            wait_until_up(port)?;
            println!("Redis is up at {url}");
            (url, Some((server, port)))
        }
    };
    let outcome = run(test_command(extra).current_dir(repo_root()).env(E2E_REDIS, &url));
    if let Some((server, port)) = server {
        // asked first, since stopping WSL's launcher may leave the server itself running
        shutdown(port);
        drop(server);
    }
    outcome
}

/// The end-to-end tests with nextest, `extra` added to its arguments (`--profile ci`); with
/// `cargo test` when nextest isn't installed.
fn test_command(extra: Vec<String>) -> Command {
    let mut command = cargo();
    if nextest_installed() {
        command.args(["nextest", "run", "--run-ignored", "only", "-E", FILTER]).args(extra);
    } else {
        command.arg("test").args(extra).args(["--", "--ignored", "e2e::"]);
    }
    command
}

fn nextest_installed() -> bool {
    cargo().args(["nextest", "--version"]).output().is_ok_and(|o| o.status.success())
}

/// A port nothing listens on, from the system.
fn free_port() -> Result<u16> {
    let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| format!("can't find a free port: {e}"))?;
    listener.local_addr().map(|a| a.port()).map_err(|e| format!("can't find a free port: {e}"))
}

/// Starts a server on `port` the first way this machine allows.
fn start(port: u16) -> Result<Server> {
    let port_text = port.to_string();
    let native = ["--port", &port_text, "--bind", "127.0.0.1"];
    for program in ["redis-server", "valkey-server", "memurai"] {
        if works(program, "--version") {
            println!("Starting {program} on port {port}");
            let child = Command::new(program).args(native).args(SERVER_ARGS).stdout(Stdio::null()).spawn();
            return child.map(Server::Process).map_err(|e| format!("could not start {program}: {e}"));
        }
    }
    if docker_runs_linux() {
        let name = format!("cachemire-e2e-{port}");
        println!("Starting {IMAGE} in Docker on port {port} (container {name})");
        let published = format!("127.0.0.1:{port}:6379");
        run(Command::new("docker")
            .args(["run", "--detach", "--rm", "--name", &name, "--publish", &published, IMAGE, "redis-server"])
            .args(SERVER_ARGS))?;
        return Ok(Server::Container(name));
    }
    if cfg!(windows) && Command::new("wsl").args(["-e", "sh", "-c", "command -v redis-server"]).output().is_ok_and(|o| o.status.success()) {
        println!("Starting redis-server in WSL on port {port}");
        let child = Command::new("wsl").args(["-e", "redis-server"]).args(native).args(SERVER_ARGS).stdout(Stdio::null()).spawn();
        return child.map(Server::Process).map_err(|e| format!("could not start redis-server in WSL: {e}"));
    }
    Err(format!(
        "no Redis server to start: install redis-server (or valkey-server), or Docker{}, \
         or set {E2E_REDIS} to a server whose databases the tests may empty",
        if cfg!(windows) { ", or redis-server in WSL" } else { "" }
    ))
}

/// Whether Docker answers and runs Linux containers (on Windows it may run Windows ones).
fn docker_runs_linux() -> bool {
    let output = Command::new("docker").args(["info", "--format", "{{.OSType}}"]).output();
    output.is_ok_and(|o| o.status.success() && String::from_utf8_lossy(&o.stdout).trim() == "linux")
}

/// One command to the server on `port`, in Redis's inline form: its reply's first line.
fn ask(port: u16, command: &str) -> Option<String> {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(2))).ok()?;
    stream.write_all(format!("{command}\r\n").as_bytes()).ok()?;
    let mut reply = [0u8; 64];
    let read = stream.read(&mut reply).ok()?;
    Some(String::from_utf8_lossy(&reply[..read]).lines().next().unwrap_or_default().to_string())
}

/// Waits for the server to answer a PING (a first Docker run may pull the image before).
fn wait_until_up(port: u16) -> Result<()> {
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(60) {
        if ask(port, "PING").as_deref() == Some("+PONG") {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    Err(format!("the Redis server on port {port} didn't answer within a minute"))
}

fn shutdown(port: u16) {
    let _ = ask(port, "SHUTDOWN NOSAVE");
}
