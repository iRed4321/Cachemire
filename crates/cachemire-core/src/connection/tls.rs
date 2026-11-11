//! The certificate authorities Redis TLS trusts: the bundled web roots plus the ones
//! installed on the machine, and the TLS client an SSH tunnel uses with them.

use std::sync::{Arc, OnceLock};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use rustc_hash::FxHashSet;
use rustls::pki_types::CertificateDer;
use rustls::{ClientConfig, RootCertStore};
use tokio_rustls::TlsConnector;

/// Every usable bundled and system root, once each. A system store that can't be
/// read, or a certificate in it that isn't usable, is logged and left out.
fn trusted_roots() -> &'static [CertificateDer<'static>] {
    static ROOTS: OnceLock<Vec<CertificateDer<'static>>> = OnceLock::new();
    ROOTS.get_or_init(|| {
        let native = rustls_native_certs::load_native_certs();
        for error in &native.errors {
            tracing::warn!(%error, "couldn't read some of the system's trusted certificates");
        }
        let mut seen = FxHashSet::default();
        let mut checker = RootCertStore::empty();
        let candidates = webpki_root_certs::TLS_SERVER_ROOT_CERTS.iter().cloned().chain(native.certs);
        candidates.filter(|cert| seen.insert(cert.to_vec()) && checker.add(cert.clone()).is_ok()).collect()
    })
}

/// The trusted roots as a PEM bundle, in the form redis-rs takes.
pub fn roots_pem() -> Vec<u8> {
    let mut pem = String::new();
    for cert in trusted_roots() {
        pem.push_str("-----BEGIN CERTIFICATE-----\n");
        for line in STANDARD.encode(cert).as_bytes().chunks(64) {
            pem.push_str(&String::from_utf8_lossy(line));
            pem.push('\n');
        }
        pem.push_str("-----END CERTIFICATE-----\n");
    }
    pem.into_bytes()
}

/// A connector trusting the same roots; verifies the certificate against the
/// server name given at each handshake.
pub fn connector() -> TlsConnector {
    static CONFIG: OnceLock<Arc<ClientConfig>> = OnceLock::new();
    let config = CONFIG.get_or_init(|| {
        let mut roots = RootCertStore::empty();
        roots.add_parsable_certificates(trusted_roots().iter().cloned());
        Arc::new(ClientConfig::builder().with_root_certificates(roots).with_no_client_auth())
    });
    TlsConnector::from(config.clone())
}
