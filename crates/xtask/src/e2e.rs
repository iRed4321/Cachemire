//! `cargo xtask e2e`: the end-to-end tests, against a throwaway Redis server and cluster started
//! for them: `redis-server` (or Valkey, Memurai) on the PATH, else Docker, else `redis-server` in
//! WSL. The tests read where they are from `CACHEMIRE_E2E_REDIS` and `CACHEMIRE_E2E_CLUSTER`.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use crate::tools::{Result, cargo, repo_root, run, works};

/// What the tests read the server's URL from (see cachemire-core's `test_support`).
const E2E_REDIS: &str = "CACHEMIRE_E2E_REDIS";

/// What the tests read the URL of one of the cluster's nodes from.
const E2E_CLUSTER: &str = "CACHEMIRE_E2E_CLUSTER";

/// The tests' cluster: this many primaries, no replicas.
const CLUSTER_NODES: usize = 3;

/// Every node of the tests' cluster has these, beside its ports and its nodes file.
const CLUSTER_ARGS: [&str; 10] =
    ["--cluster-enabled", "yes", "--cluster-announce-ip", "127.0.0.1", "--protected-mode", "no", "--save", "", "--appendonly", "no"];

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
            (url, Some((server, vec![port])))
        }
    };
    let given = std::env::var(E2E_CLUSTER).ok().filter(|url| !url.is_empty());
    let (cluster_url, cluster) = match given {
        Some(url) => {
            println!("Using the Redis cluster {E2E_CLUSTER} names: {url}");
            (url, None)
        }
        None => {
            let nodes = cluster_ports()?;
            let servers = start_cluster(&nodes)?;
            let ports: Vec<u16> = nodes.iter().map(|(port, _)| *port).collect();
            form_cluster(&nodes)?;
            let url = format!("redis://127.0.0.1:{}", ports[0]);
            println!("The Redis cluster is up at {url}");
            (url, Some((servers, ports)))
        }
    };
    let outcome = run(test_command(extra).current_dir(repo_root()).env(E2E_REDIS, &url).env(E2E_CLUSTER, &cluster_url));
    for (servers, ports) in server.map(|(server, ports)| (vec![server], ports)).into_iter().chain(cluster) {
        // asked first, since stopping WSL's launcher may leave the server itself running
        for port in ports {
            shutdown(port);
        }
        drop(servers);
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

/// The client and cluster bus ports of each node of the tests' cluster, all different.
fn cluster_ports() -> Result<Vec<(u16, u16)>> {
    let mut ports: Vec<u16> = Vec::new();
    while ports.len() < 2 * CLUSTER_NODES {
        let port = free_port()?;
        if !ports.contains(&port) {
            ports.push(port);
        }
    }
    Ok(ports.chunks(2).map(|pair| (pair[0], pair[1])).collect())
}

/// Starts each node of the tests' cluster, `(port, bus port)`, the first way this machine allows:
/// all of them in one Docker container, which publishes their ports.
fn start_cluster(nodes: &[(u16, u16)]) -> Result<Vec<Server>> {
    let node_args = |port: u16, bus: u16, nodes_file: String| {
        let mut args: Vec<String> = ["--port", &port.to_string(), "--cluster-port", &bus.to_string()].map(String::from).to_vec();
        args.extend(["--cluster-config-file".to_string(), nodes_file]);
        args.extend(CLUSTER_ARGS.map(String::from));
        args
    };
    for program in ["redis-server", "valkey-server", "memurai"] {
        if works(program, "--version") {
            let dir = repo_root().join("target").join("e2e-cluster");
            std::fs::create_dir_all(&dir).map_err(|e| format!("can't make {}: {e}", dir.display()))?;
            println!("Starting a cluster of {} {program} on ports {nodes:?}", nodes.len());
            let mut servers = Vec::new();
            for &(port, bus) in nodes {
                let nodes_file = dir.join(format!("nodes-{port}.conf")).display().to_string();
                let child = Command::new(program)
                    .args(["--bind", "127.0.0.1"])
                    .args(node_args(port, bus, nodes_file))
                    .stdout(Stdio::null())
                    .spawn();
                servers.push(child.map(Server::Process).map_err(|e| format!("could not start {program}: {e}"))?);
            }
            return Ok(servers);
        }
    }
    if docker_runs_linux() {
        let name = format!("cachemire-e2e-cluster-{}", nodes[0].0);
        println!("Starting a cluster of {} nodes in Docker ({IMAGE}) on ports {nodes:?} (container {name})", nodes.len());
        let mut script = String::new();
        for &(port, bus) in nodes {
            let args =
                node_args(port, bus, format!("nodes-{port}.conf")).iter().map(|arg| format!("'{arg}'")).collect::<Vec<_>>().join(" ");
            script.push_str(&format!("redis-server {args} --daemonize yes && "));
        }
        script.push_str("exec tail -f /dev/null");
        let mut docker = Command::new("docker");
        docker.args(["run", "--detach", "--rm", "--name", &name]);
        for &(port, _) in nodes {
            docker.args(["--publish", &format!("127.0.0.1:{port}:{port}")]);
        }
        run(docker.args([IMAGE, "sh", "-c", &script]))?;
        return Ok(vec![Server::Container(name)]);
    }
    if cfg!(windows) && Command::new("wsl").args(["-e", "sh", "-c", "command -v redis-server"]).output().is_ok_and(|o| o.status.success()) {
        println!("Starting a cluster of {} redis-server in WSL on ports {nodes:?}", nodes.len());
        let mut servers = Vec::new();
        for &(port, bus) in nodes {
            let nodes_file = format!("/tmp/cachemire-e2e-nodes-{port}.conf");
            let child =
                Command::new("wsl").args(["-e", "redis-server"]).args(node_args(port, bus, nodes_file)).stdout(Stdio::null()).spawn();
            servers.push(child.map(Server::Process).map_err(|e| format!("could not start redis-server in WSL: {e}"))?);
        }
        return Ok(servers);
    }
    Err(format!(
        "no Redis cluster to start: install redis-server (or valkey-server), or Docker{}, \
         or set {E2E_CLUSTER} to a node of a cluster the tests may write keys into",
        if cfg!(windows) { ", or redis-server in WSL" } else { "" }
    ))
}

/// Gives each node its share of the slots, has the first meet the others, then waits for
/// every node to see the whole cluster served.
fn form_cluster(nodes: &[(u16, u16)]) -> Result<()> {
    for &(port, _) in nodes {
        wait_until_up(port)?;
    }
    for (i, &(port, _)) in nodes.iter().enumerate() {
        let (start, end) = (i * 16384 / nodes.len(), (i + 1) * 16384 / nodes.len() - 1);
        let reply = ask(port, &format!("CLUSTER ADDSLOTSRANGE {start} {end}"));
        if reply.as_deref() != Some("+OK") {
            return Err(format!("the node on port {port} didn't take its slots ({reply:?}): the cluster needs Redis 7 or later"));
        }
    }
    let (first, _) = nodes[0];
    for &(port, bus) in &nodes[1..] {
        if ask(first, &format!("CLUSTER MEET 127.0.0.1 {port} {bus}")).as_deref() != Some("+OK") {
            return Err(format!("the node on port {first} couldn't meet the one on port {port}"));
        }
    }
    let start = Instant::now();
    let known = format!("cluster_known_nodes:{}", nodes.len());
    while start.elapsed() < Duration::from_secs(60) {
        let formed =
            |port: u16| ask_text(port, "CLUSTER INFO").is_some_and(|info| info.contains("cluster_state:ok") && info.contains(&known));
        if nodes.iter().all(|&(port, _)| formed(port)) {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    Err("the tests' Redis cluster didn't form within a minute".to_string())
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

/// One command to the server on `port`, in Redis's inline form: its whole bulk reply.
fn ask_text(port: u16, command: &str) -> Option<String> {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(2))).ok()?;
    stream.write_all(format!("{command}\r\n").as_bytes()).ok()?;
    let mut reply = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let read = stream.read(&mut chunk).ok().filter(|read| *read > 0)?;
        reply.extend_from_slice(&chunk[..read]);
        let text = String::from_utf8_lossy(&reply);
        let (header, body) = text.split_once("\r\n")?;
        let length: usize = header.strip_prefix('$')?.parse().ok()?;
        if body.len() >= length {
            return Some(body[..length].to_string());
        }
    }
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
