//! A tunnel into a Kubernetes cluster through its API server, logged in with the user's
//! kubeconfig: what `kubectl port-forward` does (to the pod behind a Service, a pod name
//! or a pod IP), or, through a relay pod, `kubectl exec` of a command that connects.

use std::net::IpAddr;
use std::sync::Arc;

use k8s_openapi::api::core::v1::{Pod, Service};
use k8s_openapi::apimachinery::pkg::util::intstr::IntOrString;
use kube::api::{Api, AttachParams, ListParams, Portforwarder};
use kube::config::KubeConfigOptions;
use kube::{Client, Config};
use parking_lot::Mutex;
use rustc_hash::FxHashMap;
use rustls::pki_types::ServerName;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, BufReader, DuplexStream, ReadHalf, SimplexStream, WriteHalf};

use crate::connection::KubeSettings;
use crate::connection::tls;
use crate::connection::tunnel::{BoxedStream, DialFuture, Dialer, Tunnel};

/// Why the tunnel into the Kubernetes cluster couldn't be set up.
#[derive(Debug, thiserror::Error)]
pub enum KubeError {
    #[error("couldn't read the Kubernetes configuration: {0}")]
    Config(#[source] kube::config::KubeconfigError),
    #[error("couldn't reach the Kubernetes cluster: {0}")]
    Request(#[source] kube::Error),
    #[error("Kubernetes refused: {message}")]
    Refused { message: String },
    #[error("the Service {service} has no pods behind it (no selector): name a relay pod in the connection")]
    NoSelector { service: String },
    #[error("no pod behind the Service {service} is ready")]
    NoReadyPod { service: String },
    #[error("no pod in the namespace {namespace} answers to {host}: name a relay pod in the connection")]
    NoPod { namespace: String, host: String },
    #[error("the relay pod {pod} couldn't reach {host}:{port}: {message}")]
    RelayFailed { pod: String, host: String, port: u16, message: String },
    #[error("couldn't make a secure connection to {host} through the Kubernetes tunnel: {source}")]
    Tls { host: String, source: std::io::Error },
    #[error("couldn't open a local port for the Kubernetes tunnel: {0}")]
    LocalPort(#[source] std::io::Error),
}

impl KubeError {
    /// What the API server answered, when it answered with an error; how it failed otherwise.
    fn from_request(error: kube::Error) -> Self {
        match error {
            kube::Error::Api(status) => KubeError::Refused { message: status.message },
            other => KubeError::Request(other),
        }
    }
}

/// Run by `sh` in the relay pod as `sh -c SCRIPT sh MODE HOST PORT`: connects to `HOST:PORT`
/// and relays stdin and stdout (MODE `relay`), or only tries to connect (`probe`), with the
/// first of socat, nc and bash's `/dev/tcp` it has. nc goes through a fifo so it stops on stdin's end.
const RELAY_SCRIPT: &str = r#"m=$1 h=$2 p=$3
if command -v socat >/dev/null 2>&1; then
  if [ "$m" = probe ]; then exec socat -u OPEN:/dev/null "TCP:$h:$p"; fi
  exec socat - "TCP:$h:$p"
fi
if command -v nc >/dev/null 2>&1; then
  if [ "$m" = probe ]; then
    nc -z -w 5 "$h" "$p" 2>/dev/null || nc -w 2 "$h" "$p" </dev/null >/dev/null || { echo "nothing answers at $h:$p" >&2; exit 1; }
    exit 0
  fi
  d=$(mktemp -d 2>/dev/null) && mkfifo "$d/in" 2>/dev/null || exec nc "$h" "$p"
  nc "$h" "$p" <"$d/in" & n=$!
  cat >"$d/in"
  kill "$n" 2>/dev/null; rm -rf "$d"; exit 0
fi
if command -v bash >/dev/null 2>&1; then
  exec bash -c 'exec 3<>"/dev/tcp/$1/$2" || exit 1; [ "$3" = probe ] && exit 0; cat <&3 & c=$!; cat >&3; kill $c' bash "$h" "$p" "$m"
fi
echo "it has none of socat, nc or bash to connect with" >&2
exit 127
"#;

/// The size of the buffers between a relayed command and this end, each way.
const RELAY_BUFFER: usize = 1 << 20;

/// Where a `host:port` the Redis client asks for is served: a pod's port, in a namespace.
#[derive(Clone)]
struct Target {
    namespace: String,
    pod: String,
    port: u16,
}

/// The Kubernetes transport: each stream is a port-forward to the pod serving the address
/// asked for or, with a relay pod, a command run in it that connects there.
struct KubeDialer {
    client: Client,
    namespace: String,
    relay: Option<String>,
    // the pod found for each address asked for, until a port-forward to it fails
    targets: Mutex<FxHashMap<(String, u16), Target>>,
}

impl Dialer for KubeDialer {
    fn dial<'a>(&'a self, host: &'a str, port: u16) -> DialFuture<'a> {
        Box::pin(async move { self.connect(host, port).await.map_err(std::io::Error::other) })
    }
}

impl KubeDialer {
    fn pods(&self, namespace: &str) -> Api<Pod> {
        Api::namespaced(self.client.clone(), namespace)
    }

    async fn connect(&self, host: &str, port: u16) -> Result<BoxedStream, KubeError> {
        match &self.relay {
            Some(pod) => self.relay(pod, host, port).await,
            None => self.forward(host, port).await,
        }
    }

    /// A port-forward to the pod serving `host:port`.
    async fn forward(&self, host: &str, port: u16) -> Result<BoxedStream, KubeError> {
        let target = self.target(host, port).await?;
        let mut forwarder = match self.pods(&target.namespace).portforward(&target.pod, &[target.port]).await {
            Ok(forwarder) => forwarder,
            Err(error) => {
                self.targets.lock().remove(&(host.to_string(), port));
                return Err(KubeError::from_request(error));
            }
        };
        let stream = forwarder
            .take_stream(target.port)
            .ok_or_else(|| KubeError::NoPod { namespace: target.namespace.clone(), host: host.to_string() })?;
        if let Some(failed) = forwarder.take_error(target.port) {
            let pod = target.pod.clone();
            tokio::spawn(async move {
                if let Some(error) = failed.await {
                    tracing::warn!(%pod, %error, "Kubernetes port-forward failed");
                }
            });
        }
        Ok(Box::new(Forwarded { stream, forwarder }))
    }

    /// The pod serving `host:port`, looked up once.
    async fn target(&self, host: &str, port: u16) -> Result<Target, KubeError> {
        let key = (host.to_string(), port);
        if let Some(found) = self.targets.lock().get(&key) {
            return Ok(found.clone());
        }
        let found = self.find_target(host, port).await?;
        tracing::debug!(target = %format_args!("{host}:{port}"), pod = %found.pod, port = found.port, "Kubernetes target found");
        self.targets.lock().insert(key, found.clone());
        Ok(found)
    }

    /// A pod IP is the pod that has it; `service`, `service.namespace[.svc…]` is a ready pod
    /// behind that Service; `pod.subdomain[.namespace.svc…]` and a bare name are that pod.
    async fn find_target(&self, host: &str, port: u16) -> Result<Target, KubeError> {
        let not_found = |namespace: &str| KubeError::NoPod { namespace: namespace.to_string(), host: host.to_string() };
        if let Ok(ip) = host.trim_start_matches('[').trim_end_matches(']').parse::<IpAddr>() {
            let found = self.pods(&self.namespace).list(&ListParams::default().fields(&format!("status.podIP={ip}"))).await;
            let pod = found.map_err(KubeError::from_request)?.items.into_iter().find_map(|pod| pod.metadata.name);
            return pod.map(|pod| Target { namespace: self.namespace.clone(), pod, port }).ok_or_else(|| not_found(&self.namespace));
        }

        let labels: Vec<&str> = host.trim_end_matches('.').split('.').collect();
        let (name, service_namespace, pod_namespace) = match labels.as_slice() {
            [name] => (*name, Some(self.namespace.as_str()), self.namespace.as_str()),
            [name, namespace] => (*name, Some(*namespace), self.namespace.as_str()),
            [name, namespace, "svc", ..] => (*name, Some(*namespace), self.namespace.as_str()),
            [name, _, namespace, "svc", ..] => (*name, None, *namespace),
            [name, ..] => (*name, None, self.namespace.as_str()),
            [] => return Err(not_found(&self.namespace)),
        };
        if let Some(namespace) = service_namespace {
            let services: Api<Service> = Api::namespaced(self.client.clone(), namespace);
            if let Some(service) = services.get_opt(name).await.map_err(KubeError::from_request)? {
                return self.behind(service, namespace, port).await;
            }
        }
        match self.pods(pod_namespace).get_opt(name).await.map_err(KubeError::from_request)? {
            Some(_) => Ok(Target { namespace: pod_namespace.to_string(), pod: name.to_string(), port }),
            None => Err(not_found(pod_namespace)),
        }
    }

    /// A ready pod behind `service` and the pod's port its `port` leads to.
    async fn behind(&self, service: Service, namespace: &str, port: u16) -> Result<Target, KubeError> {
        let name = service.metadata.name.unwrap_or_default();
        let spec = service.spec.unwrap_or_default();
        let selector =
            spec.selector.filter(|selector| !selector.is_empty()).ok_or_else(|| KubeError::NoSelector { service: name.clone() })?;
        let labels = selector.iter().map(|(key, value)| format!("{key}={value}")).collect::<Vec<_>>().join(",");
        let pods = self.pods(namespace).list(&ListParams::default().labels(&labels)).await.map_err(KubeError::from_request)?;
        let pod = pods.items.into_iter().find(is_ready).ok_or_else(|| KubeError::NoReadyPod { service: name.clone() })?;
        let target_port = spec.ports.unwrap_or_default().into_iter().find(|p| p.port == i32::from(port)).and_then(|p| p.target_port);
        let port = match target_port {
            Some(IntOrString::Int(number)) => u16::try_from(number).unwrap_or(port),
            Some(IntOrString::String(named)) => container_port(&pod, &named).unwrap_or(port),
            None => port,
        };
        Ok(Target { namespace: namespace.to_string(), pod: pod.metadata.name.unwrap_or_default(), port })
    }

    /// Runs [`RELAY_SCRIPT`] in the relay pod, in `mode`, with stdin when relaying.
    async fn run_relay(&self, pod: &str, mode: &str, host: &str, port: u16) -> Result<kube::api::AttachedProcess, KubeError> {
        let command = ["sh", "-c", RELAY_SCRIPT, "sh", mode, host, &port.to_string()].map(str::to_string);
        let mut params = AttachParams::default().stdin(mode == "relay").stdout(true).stderr(true);
        params.max_stdin_buf_size = Some(RELAY_BUFFER);
        params.max_stdout_buf_size = Some(RELAY_BUFFER);
        self.pods(&self.namespace).exec(pod, command, &params).await.map_err(KubeError::from_request)
    }

    /// A stream to `host:port` made by a command run in the relay pod; what it says on
    /// stderr goes to the log.
    async fn relay(&self, pod: &str, host: &str, port: u16) -> Result<BoxedStream, KubeError> {
        let mut process = self.run_relay(pod, "relay", host, port).await?;
        let failed = || KubeError::RelayFailed { pod: pod.to_string(), host: host.to_string(), port, message: "no stdin or stdout".into() };
        let (reader, writer) = (process.stdout().ok_or_else(failed)?, process.stdin().ok_or_else(failed)?);
        if let Some(stderr) = process.stderr() {
            let pod = pod.to_string();
            tokio::spawn(async move {
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    tracing::warn!(%pod, %line, "the relay pod's command said");
                }
            });
        }
        Ok(Box::new(Relayed { reader, writer, _process: process }))
    }

    /// Whether the relay pod can connect to `host:port`, with what it said when it can't.
    async fn probe_relay(&self, pod: &str, host: &str, port: u16) -> Result<(), KubeError> {
        let mut process = self.run_relay(pod, "probe", host, port).await?;
        let status = process.take_status();
        let mut said = String::new();
        if let Some(mut stderr) = process.stderr() {
            let _ = stderr.read_to_string(&mut said).await;
        }
        let status = match status {
            Some(status) => status.await,
            None => None,
        };
        let succeeded = status.as_ref().is_some_and(|s| s.status.as_deref() == Some("Success"));
        if succeeded {
            return Ok(());
        }
        let said = said.lines().map(str::trim).filter(|line| !line.is_empty()).collect::<Vec<_>>().join("; ");
        let message = if said.is_empty() { status.and_then(|s| s.message).unwrap_or_default() } else { said };
        Err(KubeError::RelayFailed { pod: pod.to_string(), host: host.to_string(), port, message })
    }
}

fn is_ready(pod: &Pod) -> bool {
    let conditions = pod.status.as_ref().and_then(|status| status.conditions.as_ref());
    conditions.is_some_and(|conditions| conditions.iter().any(|c| c.type_ == "Ready" && c.status == "True"))
}

/// The number of the port named `name` in one of the pod's containers.
fn container_port(pod: &Pod, name: &str) -> Option<u16> {
    let containers = pod.spec.as_ref().map(|spec| spec.containers.iter()).into_iter().flatten();
    let mut ports = containers.flat_map(|container| container.ports.iter().flatten());
    ports.find(|p| p.name.as_deref() == Some(name)).and_then(|p| u16::try_from(p.container_port).ok())
}

/// A port-forward's stream, which stops the forward when dropped.
struct Forwarded {
    stream: DuplexStream,
    forwarder: Portforwarder,
}

impl Drop for Forwarded {
    fn drop(&mut self) {
        self.forwarder.abort();
    }
}

/// A relayed command's stdout and stdin, kept running as long as this lives.
struct Relayed {
    reader: ReadHalf<SimplexStream>,
    writer: WriteHalf<SimplexStream>,
    _process: kube::api::AttachedProcess,
}

macro_rules! delegate_io {
    ($stream:ty, $read:ident, $write:ident) => {
        impl AsyncRead for $stream {
            fn poll_read(
                mut self: std::pin::Pin<&mut Self>,
                cx: &mut std::task::Context<'_>,
                buf: &mut tokio::io::ReadBuf<'_>,
            ) -> std::task::Poll<std::io::Result<()>> {
                std::pin::Pin::new(&mut self.$read).poll_read(cx, buf)
            }
        }

        impl AsyncWrite for $stream {
            fn poll_write(
                mut self: std::pin::Pin<&mut Self>,
                cx: &mut std::task::Context<'_>,
                buf: &[u8],
            ) -> std::task::Poll<std::io::Result<usize>> {
                std::pin::Pin::new(&mut self.$write).poll_write(cx, buf)
            }

            fn poll_flush(mut self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<std::io::Result<()>> {
                std::pin::Pin::new(&mut self.$write).poll_flush(cx)
            }

            fn poll_shutdown(mut self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<std::io::Result<()>> {
                std::pin::Pin::new(&mut self.$write).poll_shutdown(cx)
            }
        }
    };
}

delegate_io!(Forwarded, stream, stream);
delegate_io!(Relayed, reader, writer);

/// Signs in to the cluster of the kubeconfig's context and sets up a tunnel to
/// `host:port` as seen from inside it, checking it can be reached. With a `tls` name,
/// each forwarded connection is secured against it.
pub async fn open(kube: &KubeSettings, host: &str, port: u16, tls: Option<ServerName<'static>>) -> Result<Tunnel, KubeError> {
    let non_empty = |value: &str| Some(value.trim().to_string()).filter(|v| !v.is_empty());
    let options = KubeConfigOptions { context: non_empty(&kube.context), ..Default::default() };
    let config = Config::from_kubeconfig(&options).await.map_err(KubeError::Config)?;
    let namespace = non_empty(&kube.namespace).unwrap_or_else(|| config.default_namespace.clone());
    let client = Client::try_from(config).map_err(KubeError::from_request)?;
    let dialer = KubeDialer { client, namespace, relay: non_empty(&kube.relay_pod), targets: Mutex::default() };

    if let Some(pod) = &dialer.relay {
        dialer.probe_relay(pod, host, port).await?;
    }
    if let Some(name) = &tls {
        let probe = dialer.connect(host, port).await?;
        tls::connector().connect(name.clone(), probe).await.map_err(|source| KubeError::Tls { host: host.to_string(), source })?;
    } else if dialer.relay.is_none() {
        drop(dialer.connect(host, port).await?);
    }
    tracing::info!(namespace = %dialer.namespace, relay = ?dialer.relay, "Kubernetes tunnel opened");
    Tunnel::start(Arc::new(dialer), host, port, tls).await.map_err(KubeError::LocalPort)
}

/// The contexts of the user's kubeconfig, and the current one ("" when none is set).
pub fn contexts() -> Result<(Vec<String>, String), KubeError> {
    let config = kube::config::Kubeconfig::read().map_err(KubeError::Config)?;
    let names = config.contexts.into_iter().map(|context| context.name).collect();
    Ok((names, config.current_context.unwrap_or_default()))
}
