//! TLS encryption types

use crate::*;
use once_cell::sync::Lazy;
use sha2::Digest;
use std::sync::Arc;

/// The well-known CA keypair in plaintext pem format.
/// Some TLS clients require CA roots to validate client-side certificates.
/// By publishing the private keys here, we are essentially allowing
/// self-signed client certificates.
const WK_CA_KEYPAIR_PEM: &str = r#"-----BEGIN PRIVATE KEY-----
MIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgkxOEyiRyocjLRpQk
RE7/bOwmHtkdLLGQrlz23m4aKQOhRANCAATUDekPM40vfqOMxf00KZwRk6gSciHx
xkzPZovign1qmbu0vZstKoVLXoGvlA/Kral9txqhSEGqIL7TdbKyMMQz
-----END PRIVATE KEY-----"#;

/// The well-known pseudo name/id for the well-known lair CA root.
const WK_CA_ID: &str = "aKdjnmYOn1HVc_RwSdxR6qa.aQLW3d5D1nYiSSO2cOrcT7a";

/// This doesn't need to be pub... We need the rcgen::Certificate
/// with the private keys still integrated in order to sign certs.
static WK_CA_RCGEN_CERT: Lazy<Arc<rcgen::Certificate>> = Lazy::new(|| {
    let mut params = rcgen::CertificateParams::new(vec![WK_CA_ID.into()]);
    params.alg = &rcgen::PKCS_ECDSA_P256_SHA256;
    params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    params
        .extended_key_usages
        .push(rcgen::ExtendedKeyUsagePurpose::Any);
    params.distinguished_name = rcgen::DistinguishedName::new();
    params.distinguished_name.push(
        rcgen::DnType::CommonName,
        "Lair Well-Known Pseudo-Self-Signing CA",
    );
    params
        .distinguished_name
        .push(rcgen::DnType::OrganizationName, "Holochain Foundation");
    params.key_pair =
        Some(rcgen::KeyPair::from_pem(WK_CA_KEYPAIR_PEM).unwrap());
    let cert = rcgen::Certificate::from_params(params).unwrap();
    Arc::new(cert)
});

/// The well-known lair CA pseudo-self-signing certificate.
static WK_CA_CERT_DER: Lazy<Arc<Vec<u8>>> = Lazy::new(|| {
    let cert = WK_CA_RCGEN_CERT.as_ref();
    let cert = cert.serialize_der().unwrap();
    Arc::new(cert)
});

/// Generate a new der-encoded tls certificate and private key
pub fn gen_cert() -> Result<(Vec<u8>, Vec<u8>)> {
    let sni = format!("a{}a.a{}a", nanoid::nanoid!(), nanoid::nanoid!());

    let mut params = rcgen::CertificateParams::new(vec![sni.clone()]);
    params.alg = &rcgen::PKCS_ECDSA_P256_SHA256;
    params
        .extended_key_usages
        .push(rcgen::ExtendedKeyUsagePurpose::Any);
    params
        .extended_key_usages
        .push(rcgen::ExtendedKeyUsagePurpose::ServerAuth);
    params
        .extended_key_usages
        .push(rcgen::ExtendedKeyUsagePurpose::ClientAuth);
    params.distinguished_name = rcgen::DistinguishedName::new();
    params.distinguished_name.push(
        rcgen::DnType::CommonName,
        format!("Lair Pseudo-Self-Signed Cert {}", &sni),
    );

    let cert = rcgen::Certificate::from_params(params).map_err(other_err)?;

    let priv_key = cert.serialize_private_key_der();

    let root_cert = &**WK_CA_RCGEN_CERT;
    let cert_der = cert
        .serialize_der_with_signer(root_cert)
        .map_err(other_err)?;

    Ok((cert_der, priv_key))
}

/// TLS Configuration
pub struct Config {
    alpn: Vec<Vec<u8>>,
    cert: Option<(Vec<u8>, Vec<u8>)>,
    cipher_suites: Vec<rustls::SupportedCipherSuite>,
    protocol_versions: Vec<&'static rustls::SupportedProtocolVersion>,
    key_log: bool,
    session_storage: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            alpn: Vec::new(),
            cert: None,
            cipher_suites: vec![
                rustls::cipher_suite::TLS13_CHACHA20_POLY1305_SHA256,
                rustls::cipher_suite::TLS13_AES_256_GCM_SHA384,
            ],
            protocol_versions: vec![&rustls::version::TLS13],
            key_log: false,
            session_storage: 512,
        }
    }
}

impl Config {
    /// Push an alpn protocol to use with this connection. If none are
    /// set, alpn negotiotion will not happen. Otherwise, specify the
    /// most preferred protocol first
    pub fn with_alpn(mut self, alpn: &[u8]) -> Self {
        self.alpn.push(alpn.into());
        self
    }

    /// Set the certificate / private key to use with this TLS config.
    /// If not set, a new certificate and private key will be generated
    pub fn with_cert(mut self, cert_der: Vec<u8>, pk_der: Vec<u8>) -> Self {
        self.cert = Some((cert_der, pk_der));
        self
    }
}

/// Single shared keylog file all sessions can report to
static KEY_LOG: Lazy<Arc<dyn rustls::KeyLog>> =
    Lazy::new(|| Arc::new(rustls::KeyLogFile::new()));

/// Tls server configuration
#[derive(Clone)]
pub struct TlsServer(pub(crate) Arc<rustls::ServerConfig>, Arc<[u8; 32]>);

impl TlsServer {
    /// Get the sha256 hash of the TLS certificate representing this server
    pub fn cert_digest(&self) -> &Arc<[u8; 32]> {
        &self.1
    }
}

/// Tls client configuration
#[derive(Clone)]
pub struct TlsClient(pub(crate) Arc<rustls::ClientConfig>, Arc<[u8; 32]>);

impl TlsClient {
    /// Get the sha256 hash of the TLS certificate representing this client
    pub fn cert_digest(&self) -> &Arc<[u8; 32]> {
        &self.1
    }
}

impl Config {
    /// Build rustls server / client from config
    pub fn build(self) -> Result<(TlsServer, TlsClient)> {
        let (cert, pk) = match self.cert {
            Some(r) => r,
            None => gen_cert()?,
        };

        let mut digest = sha2::Sha256::new();
        digest.update(&cert);
        let digest: Arc<[u8; 32]> = Arc::new(digest.finalize().into());

        let cert = rustls::Certificate(cert);
        let pk = rustls::PrivateKey(pk);

        let root_cert = rustls::Certificate(WK_CA_CERT_DER.to_vec());
        let mut root_store = rustls::RootCertStore::empty();
        root_store.add(&root_cert).map_err(other_err)?;

        let mut srv = rustls::ServerConfig::builder()
            .with_cipher_suites(self.cipher_suites.as_slice())
            .with_safe_default_kx_groups()
            .with_protocol_versions(self.protocol_versions.as_slice())
            .map_err(other_err)?
            .with_client_cert_verifier(
                rustls::server::AllowAnyAuthenticatedClient::new(root_store),
            )
            .with_single_cert(vec![cert.clone()], pk.clone())
            .map_err(other_err)?;

        if self.key_log {
            srv.key_log = KEY_LOG.clone();
        }
        srv.ticketer = rustls::Ticketer::new().map_err(other_err)?;
        srv.session_storage =
            rustls::server::ServerSessionMemoryCache::new(self.session_storage);
        for alpn in self.alpn.iter() {
            srv.alpn_protocols.push(alpn.clone());
        }
        let srv = TlsServer(Arc::new(srv), digest.clone());

        let mut cli = rustls::ClientConfig::builder()
            .with_cipher_suites(self.cipher_suites.as_slice())
            .with_safe_default_kx_groups()
            .with_protocol_versions(self.protocol_versions.as_slice())
            .map_err(other_err)?
            .with_custom_certificate_verifier(Arc::new(V))
            .with_single_cert(vec![cert], pk)
            .map_err(other_err)?;

        if self.key_log {
            cli.key_log = KEY_LOG.clone();
        }
        cli.session_storage =
            rustls::client::ClientSessionMemoryCache::new(self.session_storage);
        for alpn in self.alpn.iter() {
            cli.alpn_protocols.push(alpn.clone());
        }
        let cli = TlsClient(Arc::new(cli), digest);

        Ok((srv, cli))
    }
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
    ) -> std::result::Result<rustls::client::ServerCertVerified, rustls::Error>
    {
        Ok(rustls::client::ServerCertVerified::assertion())
    }
}
