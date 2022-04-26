//! TLS encryption types

use crate::*;
use once_cell::sync::Lazy;
use sha2::Digest;
use std::sync::Arc;
use tokio_rustls::*;

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

/// Sha256 digest of DER encoded tls certificate
pub type TlsCertDigest = Arc<[u8; 32]>;

/// DER encoded tls certificate
pub struct TlsCertDer(pub Box<[u8]>);

/// DER encoded tls private key
pub struct TlsPkDer(pub Box<[u8]>);

/// Generate a new der-encoded tls certificate and private key
pub fn gen_cert_pair() -> Result<(TlsCertDer, TlsPkDer)> {
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

    Ok((
        TlsCertDer(cert_der.into_boxed_slice()),
        TlsPkDer(priv_key.into_boxed_slice()),
    ))
}

/// Single shared keylog file all sessions can report to
static KEY_LOG: Lazy<Arc<dyn rustls::KeyLog>> =
    Lazy::new(|| Arc::new(rustls::KeyLogFile::new()));

/// TLS configuration builder
pub struct TlsConfigBuilder {
    alpn: Vec<Vec<u8>>,
    cert: Option<(TlsCertDer, TlsPkDer)>,
    cipher_suites: Vec<rustls::SupportedCipherSuite>,
    protocol_versions: Vec<&'static rustls::SupportedProtocolVersion>,
    key_log: bool,
    session_storage: usize,
}

impl Default for TlsConfigBuilder {
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

impl TlsConfigBuilder {
    /// Push an alpn protocol to use with this connection. If none are
    /// set, alpn negotiotion will not happen. Otherwise, specify the
    /// most preferred protocol first
    pub fn with_alpn(mut self, alpn: &[u8]) -> Self {
        self.alpn.push(alpn.into());
        self
    }

    /// Set the certificate / private key to use with this TLS config.
    /// If not set, a new certificate and private key will be generated
    pub fn with_cert(mut self, cert: TlsCertDer, pk: TlsPkDer) -> Self {
        self.cert = Some((cert, pk));
        self
    }

    /// Enable or disable TLS keylogging. Note, even if enabled,
    /// the standard `SSLKEYLOGFILE` environment variable must
    /// be present for keys to be written.
    pub fn with_keylog(mut self, key_log: bool) -> Self {
        self.key_log = key_log;
        self
    }

    /// Finalize/build a TlsConfig from this builder
    pub fn build(self) -> Result<TlsConfig> {
        let (cert, pk) = match self.cert {
            Some(r) => r,
            None => gen_cert_pair()?,
        };

        let mut digest = sha2::Sha256::new();
        digest.update(&cert.0);
        let digest: Arc<[u8; 32]> = Arc::new(digest.finalize().into());

        let cert = rustls::Certificate(cert.0.into_vec());
        let pk = rustls::PrivateKey(pk.0.into_vec());

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

        Ok(TlsConfig {
            srv: Arc::new(srv),
            cli: Arc::new(cli),
            digest,
        })
    }
}

/// A fully configured tls p2p session state instance
#[derive(Clone)]
pub struct TlsConfig {
    pub(crate) srv: Arc<rustls::ServerConfig>,
    #[allow(dead_code)]
    pub(crate) cli: Arc<rustls::ClientConfig>,
    digest: TlsCertDigest,
}

impl TlsConfig {
    /// Builder for generating TlsConfig instances
    pub fn builder() -> TlsConfigBuilder {
        TlsConfigBuilder::default()
    }

    /// Get the sha256 hash of the TLS certificate representing this server
    pub fn cert_digest(&self) -> &TlsCertDigest {
        &self.digest
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
