use crate::addr::*;
use crate::config::*;
use crate::direct::*;
use crate::tls::*;
use crate::yamux_framed::*;
use crate::*;
use std::net::SocketAddr;
use std::sync::Arc;
use url::Url;

const ALPN_FWST: &[u8] = b"Tx3/fwst";
const ALPN_FSPWST: &[u8] = b"Tx3/fspwst";

/// Bind a tx3 endpoint able to receive incoming connections
/// and establish outgoing connections.
pub async fn tx3_endpoint(
    tx3_config: Tx3Config,
) -> Result<(Tx3EpHnd, Tx3EpRecv)> {
    match tx3_config.tx3_stack {
        Tx3Stack::Fwst => tx3_fwst_endpoint(tx3_config).await,
        oth => {
            return Err(other_err(format!("unhandled tx3_stack: {:?}", oth)))
        }
    }
}

fn make_addrs(
    addrs: Vec<SocketAddr>,
    stack: &'static str,
    digest: &[u8; 32],
) -> Vec<Tx3Url> {
    let digest = base64::encode_config(&digest, base64::URL_SAFE_NO_PAD);
    addrs
        .into_iter()
        .map(|a| {
            Tx3Url::new(
                Url::parse(&format!("tx3://{}/{}/{}", a, stack, digest))
                    .unwrap(),
            )
        })
        .collect()
}

async fn tx3_fwst_endpoint(
    mut tx3_config: Tx3Config,
) -> Result<(Tx3EpHnd, Tx3EpRecv)> {
    let mut tls = tls::Config::default();
    if let Some((cert, pk)) = tx3_config.tls_cert_and_pk.take() {
        tls = tls.with_cert(cert, pk);
    }
    let (tls_srv, tls_cli) =
        tls.with_alpn(ALPN_FWST).with_alpn(ALPN_FSPWST).build()?;
    let digest = tls_srv.cert_digest().clone();
    let listener =
        DirectListener::bind(tx3_config.tcp_bind_socket_addr().await?, tls_srv)
            .await?;
    let bound_urls = make_addrs(listener.local_addr()?, "fwst", &*digest);
    let hnd = Tx3EpHnd {
        bound_urls,
        tls_cli,
    };
    let recv = Tx3EpRecv { listener };
    Ok((hnd, recv))
}

/// Result type for an incoming or outgoing Tx3 connection
pub struct Tx3Remote {
    /// A sha256 hash of the remote TLS certificate
    pub cert_digest: Arc<[u8; 32]>,

    /// The message sender side of this connection
    pub send: YamuxFramedSend,

    /// The message receiver side of this connection
    pub recv: YamuxFramedRecv,
}

/// Handle for making outgoing connections
pub struct Tx3EpHnd {
    bound_urls: Vec<Tx3Url>,
    tls_cli: TlsClient,
}

impl Tx3EpHnd {
    /// Get a list of urls this endpoint is bound to
    pub async fn bound_urls(&self) -> Result<Vec<Tx3Url>> {
        Ok(self.bound_urls.clone())
    }

    /// Establish an outgoing connection
    pub async fn connect(&self, url: Tx3Url) -> Result<Tx3Remote> {
        let (_stack, want_cert) = {
            let (stack, mut path) = url.stack();
            let want_cert = base64::decode_config(
                path.next().unwrap(),
                base64::URL_SAFE_NO_PAD,
            )
            .unwrap();
            (stack, want_cert)
        };
        let addr = url.socket_addr().await?;
        let (cert, _direct) =
            DirectStream::connect(addr, self.tls_cli.clone()).await?;
        if &*cert != want_cert.as_slice() {
            return Err(other_err("InvalidPeerCert"));
        }
        todo!()
    }
}

/// Stream of incoming connections
pub struct Tx3EpRecv {
    #[allow(dead_code)]
    listener: DirectListener,
}

impl Tx3EpRecv {
    /// Get the next incoming connection
    pub async fn recv(&mut self) -> Option<Result<Tx3Remote>> {
        //let (cert, direct) = self.listener.accept()
        todo!()
    }
}
