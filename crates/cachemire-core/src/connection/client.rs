use std::time::Duration;

use redis::aio::{ConnectionManager, ConnectionManagerConfig};
use redis::io::tcp::TcpSettings;
use redis::io::tcp::socket2::TcpKeepalive;
use rustls::pki_types::ServerName;

use crate::connection::{ConnectionItem, tls};
use crate::error::{Error, Invalid};

/// Where the Redis server is, as the connection says (through an SSH tunnel,
/// this is where the SSH server has to reach).
pub fn target_of(connection: &ConnectionItem) -> Result<(String, u16), Error> {
    let endpoint = connection.endpoint()?;
    Ok((endpoint.host, endpoint.port))
}

/// The name a `rediss://` endpoint's certificate has to match, when the connection is
/// secured; through an SSH tunnel the tunnel does the TLS handshake with it.
pub fn tls_name_of(connection: &ConnectionItem) -> Result<Option<ServerName<'static>>, Error> {
    let endpoint = connection.endpoint()?;
    if !endpoint.tls {
        return Ok(None);
    }
    let name = ServerName::try_from(endpoint.host).map_err(|_| Invalid::UnreadableUrl)?;
    Ok(Some(name))
}

pub(crate) fn build_client(connection: &ConnectionItem, via_local_port: Option<u16>) -> Result<redis::Client, Error> {
    use redis::IntoConnectionInfo;

    let mut endpoint = connection.endpoint()?;
    // through an SSH tunnel, the server is the tunnel's end on this machine, in plain
    // TCP: the tunnel does the TLS handshake with the endpoint's real host name
    if let Some(port) = via_local_port {
        endpoint.host = "127.0.0.1".to_string();
        endpoint.port = port;
        endpoint.tls = false;
    }

    // RESP2 on purpose: every server speaks it (RESP3 needs Redis 6+), the replies are
    // decoded in their RESP2 shapes (flat HSCAN and WITHSCORES arrays), and nothing
    // here needs RESP3's push messages
    let mut redis_settings = redis::RedisConnectionInfo::default().set_db(endpoint.db).set_protocol(redis::ProtocolVersion::RESP2);
    if let Some(username) = endpoint.username {
        redis_settings = redis_settings.set_username(username);
    }
    if let Some(password) = endpoint.password {
        redis_settings = redis_settings.set_password(password);
    }

    let tls = endpoint.tls;
    let addr = if tls {
        redis::ConnectionAddr::TcpTls { host: endpoint.host, port: endpoint.port, insecure: false, tls_params: None }
    } else {
        redis::ConnectionAddr::Tcp(endpoint.host, endpoint.port)
    };
    // keepalive probes find a socket that died silently (sleep, a dropped NAT mapping)
    let keepalive = TcpKeepalive::new().with_time(Duration::from_secs(30)).with_interval(Duration::from_secs(10));
    let tcp = TcpSettings::default().set_nodelay(true).set_keepalive(keepalive);
    let info = addr.into_connection_info()?.set_redis_settings(redis_settings).set_tcp_settings(tcp);

    if tls {
        // trusts the bundled roots and the system's (the redis feature for the former
        // stays on so redis-rs doesn't load the system store itself, and fail with it)
        let certificates = redis::TlsCertificates { client_tls: None, root_cert: Some(tls::roots_pem()) };
        return Ok(redis::Client::build_with_tls(info, certificates)?);
    }
    Ok(redis::Client::open(info)?)
}

/// Opens a connection that reopens its socket by itself once it drops, with
/// explicit timeouts. No retries: a failed (re)connect is reported at once,
/// and the next command tries again. `via_local_port` targets the SSH tunnel.
pub async fn create_connection(connection: &ConnectionItem, via_local_port: Option<u16>) -> Result<ConnectionManager, Error> {
    let client = build_client(connection, via_local_port)?;
    let config = ConnectionManagerConfig::new()
        .set_connection_timeout(Some(Duration::from_secs(20)))
        .set_response_timeout(Some(Duration::from_secs(60)))
        .set_number_of_retries(0);
    Ok(ConnectionManager::new_with_config(client, config).await?)
}
