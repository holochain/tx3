//! `feature = "dev_utils"` Helper utilities to aid in development

use std::sync::Arc;

/// Get the "localhost" ephemeral self signed tls certificate
pub fn localhost_self_signed_tls_cert(
) -> (rustls::Certificate, rustls::PrivateKey) {
    let cert =
        rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
    let pk = rustls::PrivateKey(cert.serialize_private_key_der());
    let cert = rustls::Certificate(cert.serialize_der().unwrap());
    (cert, pk)
}

/// Get a simple server config based on a single cert / private key
pub fn simple_server_config(
    cert: rustls::Certificate,
    pk: rustls::PrivateKey,
) -> tokio_rustls::TlsAcceptor {
    let server_config = rustls::ServerConfig::builder()
        .with_safe_defaults()
        .with_no_client_auth()
        .with_single_cert(vec![cert], pk)
        .unwrap();
    Arc::new(server_config).into()
}

/// Get a trusting client config that accepts all certificates
pub fn trusting_client_config() -> tokio_rustls::TlsConnector {
    let client_config = rustls::ClientConfig::builder()
        .with_safe_defaults()
        .with_custom_certificate_verifier(Arc::new(V))
        .with_no_client_auth();
    Arc::new(client_config).into()
}

struct V;

impl rustls::client::ServerCertVerifier for V {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::Certificate,
        _intermediates: &[rustls::Certificate],
        _server_name: &rustls::ServerName,
        _scts: &mut dyn Iterator<Item = &[u8]>,
        _ocsp_response: &[u8],
        _now: std::time::SystemTime,
    ) -> Result<rustls::client::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::ServerCertVerified::assertion())
    }
}
