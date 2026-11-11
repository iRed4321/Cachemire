//! The TLS client for what redis-rs can't reach itself: the handshake an SSH tunnel does
//! with the Redis server, on the tunnel's channel, checking its certificate against the
//! endpoint's own host name.

use std::sync::{Arc, OnceLock};

use rustls::{ClientConfig, RootCertStore};
use tokio_rustls::TlsConnector;

/// A connector trusting the bundled web roots; verifies the certificate against
/// the server name given at each handshake.
pub fn connector() -> TlsConnector {
    static CONFIG: OnceLock<Arc<ClientConfig>> = OnceLock::new();
    let config = CONFIG.get_or_init(|| {
        let roots = RootCertStore { roots: webpki_roots::TLS_SERVER_ROOTS.to_vec() };
        Arc::new(ClientConfig::builder().with_root_certificates(roots).with_no_client_auth())
    });
    TlsConnector::from(config.clone())
}
