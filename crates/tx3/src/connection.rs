use crate::tls::*;
use crate::*;
use std::pin::Pin;
use std::task::Context;
use std::task::Poll;
use std::sync::Arc;

/// A Tx3 p2p connection to a remote peer
pub struct Tx3Connection {
    remote_tls_cert_digest: Arc<TlsCertDigest>,
    socket: tokio_rustls::TlsStream<tokio::net::TcpStream>,
}

impl Tx3Connection {
    /// Get the TLS certificate digest of the remote end of this connection
    pub fn remote_tls_cert_digest(&self) -> &Arc<TlsCertDigest> {
        &self.remote_tls_cert_digest
    }

    // -- private -- //

    pub(crate) async fn priv_accept(
        tls: TlsConfig,
        socket: tokio::net::TcpStream,
    ) -> Result<Self> {
        let socket = tokio_rustls::TlsAcceptor::from(tls.srv.clone())
            .accept(socket)
            .await?
            .into();
        let remote_tls_cert_digest = hash_cert(&socket)?;
        Ok(Self {
            remote_tls_cert_digest,
            socket,
        })
    }

    pub(crate) async fn priv_connect(
        tls: TlsConfig,
        socket: tokio::net::TcpStream,
    ) -> Result<Self> {
        let name = "tx3".try_into().unwrap();
        let socket = tokio_rustls::TlsConnector::from(tls.cli.clone())
            .connect(name, socket)
            .await?
            .into();
        let remote_tls_cert_digest = hash_cert(&socket)?;
        Ok(Self {
            remote_tls_cert_digest,
            socket,
        })
    }
}

impl tokio::io::AsyncRead for Tx3Connection {
    #[inline(always)]
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<Result<()>> {
        Pin::new(&mut self.socket).poll_read(cx, buf)
    }
}

impl tokio::io::AsyncWrite for Tx3Connection {
    #[inline(always)]
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize>> {
        Pin::new(&mut self.socket).poll_write(cx, buf)
    }

    #[inline(always)]
    fn poll_flush(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<()>> {
        Pin::new(&mut self.socket).poll_flush(cx)
    }

    #[inline(always)]
    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<()>> {
        Pin::new(&mut self.socket).poll_shutdown(cx)
    }

    #[inline(always)]
    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[std::io::IoSlice<'_>],
    ) -> Poll<Result<usize>> {
        Pin::new(&mut self.socket).poll_write_vectored(cx, bufs)
    }

    #[inline(always)]
    fn is_write_vectored(&self) -> bool {
        self.socket.is_write_vectored()
    }
}

fn hash_cert(
    socket: &tokio_rustls::TlsStream<tokio::net::TcpStream>,
) -> Result<Arc<TlsCertDigest>> {
    let (_, c) = socket.get_ref();
    if let Some(chain) = c.peer_certificates() {
        if !chain.is_empty() {
            use sha2::Digest;
            let mut digest = sha2::Sha256::new();
            digest.update(&chain[0].0);
            let digest = Arc::new(TlsCertDigest(digest.finalize().into()));
            return Ok(digest);
        }
    }
    Err(other_err("InvalidPeerCert"))
}
