use redis::aio::{ConnectionManager, ConnectionManagerConfig};

use super::error::AppError;
use super::models::ConnectionItem;

/// Where the Redis server is, as the connection says (through an SSH tunnel,
/// this is where the SSH server has to reach).
pub fn target_of(connection: &ConnectionItem) -> Result<(String, u16), AppError> {
    let endpoint = connection.endpoint()?;
    Ok((endpoint.host, endpoint.port))
}

fn build_client(connection: &ConnectionItem, via_local_port: Option<u16>) -> Result<redis::Client, AppError> {
    use redis::IntoConnectionInfo;

    let mut endpoint = connection.endpoint()?;
    // through an SSH tunnel, the server is the tunnel's end on this machine
    if let Some(port) = via_local_port {
        endpoint.host = "127.0.0.1".to_string();
        endpoint.port = port;
    }

    // RESP2 on purpose: every server speaks it (RESP3 needs Redis 6+), the replies are
    // decoded in their RESP2 shapes (flat HSCAN and WITHSCORES arrays), and nothing
    // here needs RESP3's push messages
    let mut redis_settings = redis::RedisConnectionInfo::default()
        .set_db(endpoint.db)
        .set_protocol(redis::ProtocolVersion::RESP2);
    if let Some(username) = endpoint.username {
        redis_settings = redis_settings.set_username(username);
    }
    if let Some(password) = endpoint.password {
        redis_settings = redis_settings.set_password(password);
    }

    let addr = if endpoint.tls {
        redis::ConnectionAddr::TcpTls { host: endpoint.host, port: endpoint.port, insecure: false, tls_params: None }
    } else {
        redis::ConnectionAddr::Tcp(endpoint.host, endpoint.port)
    };
    let info = addr
        .into_connection_info()?
        .set_redis_settings(redis_settings);

    Ok(redis::Client::open(info)?)
}

/// Opens a connection that reopens its socket by itself once it drops, with
/// explicit timeouts. No retries: a failed (re)connect is reported at once,
/// and the next command tries again. `via_local_port` targets the SSH tunnel.
pub async fn create_connection(connection: &ConnectionItem, via_local_port: Option<u16>) -> Result<ConnectionManager, AppError> {
    let client = build_client(connection, via_local_port)?;
    let config = ConnectionManagerConfig::new()
        .set_connection_timeout(Some(std::time::Duration::from_secs(20)))
        .set_response_timeout(Some(std::time::Duration::from_secs(60)))
        .set_number_of_retries(0);
    Ok(ConnectionManager::new_with_config(client, config).await?)
}
