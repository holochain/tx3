//! Direct system stream connector / acceptor types

use crate::tls::*;
use crate::ws_framed::*;
use crate::*;
use rw_stream_sink::*;
use std::net::SocketAddr;
use std::pin::Pin;
use std::task::Context;
use std::task::Poll;
use tokio::net::*;
use tokio_rustls::*;

/// Direct system stream type.
///
/// This newtype is largely to reduce boilerplate / generic params
pub struct DirectStream(RwStreamSink<WsFramed<TlsStream<TcpStream>>>);

impl DirectStream {
    /// Establish a direct outgoing connection
    pub async fn connect(addr: SocketAddr, tls: TlsClient) -> Result<Self> {
        // -- tcp -- //
        let con = TcpStream::connect(addr).await?;

        // -- tls -- //
        let tls: TlsConnector = tls.0.into();
        let name = "stub".try_into().unwrap();
        let con = tls.connect(name, con).await?;
        let con: TlsStream<_> = con.into();

        // -- ws -- //
        let con = WsFramed::connect(con).await?;
        let con = RwStreamSink::new(con);

        // -- result -- //
        Ok(Self(con))
    }
}

impl futures::io::AsyncRead for DirectStream {
    #[inline(always)]
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<Result<usize>> {
        Pin::new(&mut self.0).poll_read(cx, buf)
    }
}

impl futures::io::AsyncWrite for DirectStream {
    #[inline(always)]
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize>> {
        Pin::new(&mut self.0).poll_write(cx, buf)
    }

    #[inline(always)]
    fn poll_flush(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<()>> {
        Pin::new(&mut self.0).poll_flush(cx)
    }

    #[inline(always)]
    fn poll_close(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<()>> {
        Pin::new(&mut self.0).poll_close(cx)
    }
}

/// Direct system stream listener acceptor future type.
///
/// This newtype is largely to reduce boilerplate / generic params
pub struct DirectIncoming(TcpStream, TlsAcceptor);

impl DirectIncoming {
    /// Accept this incoming stream seed
    pub async fn accept(self) -> Result<DirectStream> {
        // -- tcp -- //
        let Self(con, tls) = self;

        // -- tls -- //
        let con = tls.accept(con).await?;
        let con: TlsStream<_> = con.into();

        // -- ws -- //
        let con = WsFramed::accept(con).await?;
        let con = RwStreamSink::new(con);

        // -- result -- //
        Ok(DirectStream(con))
    }
}

/// Direct system stream listener type.
///
/// This newtype is largely to reduce boilerplate / generic params
pub struct DirectListener(TcpListener, TlsAcceptor);

impl DirectListener {
    /// Bind a new direct listener
    pub async fn bind(addr: SocketAddr, tls: TlsServer) -> Result<Self> {
        let tls = tls.0.into();
        let srv = TcpListener::bind(addr).await?;
        Ok(Self(srv, tls))
    }
}

impl futures::Stream for DirectListener {
    type Item = Result<DirectIncoming>;

    #[inline(always)]
    fn poll_next(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Self::Item>> {
        match self.0.poll_accept(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Err(err)) => Poll::Ready(Some(Err(err))),
            Poll::Ready(Ok((s, _))) => {
                Poll::Ready(Some(Ok(DirectIncoming(s, self.1.clone()))))
            }
        }
    }
}
