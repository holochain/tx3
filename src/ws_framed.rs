//! Wrapper struct to turn a stream into a websocket backed framed stream/sink

use crate::*;
use futures::{Sink, Stream};
use std::pin::Pin;
use std::task::Context;
use std::task::Poll;
use tokio_tungstenite::tungstenite::Message;

/// Wrapper struct to turn a stream into a websocket backed framed stream/sink
pub struct WsFramed<S>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    ws: tokio_tungstenite::WebSocketStream<S>,
}

impl<S> WsFramed<S>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    /// Construct a new WsFramed as a server-side stream
    pub async fn accept(s: S) -> Result<Self> {
        let ws = tokio_tungstenite::accept_async(s)
            .await
            .map_err(other_err)?;
        Ok(Self { ws })
    }

    /// Construct a new WsFramed as a client-side stream
    pub async fn connect(s: S) -> Result<Self> {
        let (ws, _) = tokio_tungstenite::client_async("wss://localhost", s)
            .await
            .map_err(other_err)?;
        Ok(Self { ws })
    }
}

impl<S> Sink<Vec<u8>> for WsFramed<S>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    type Error = std::io::Error;

    fn poll_ready(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<()>> {
        match Pin::new(&mut self.ws).poll_ready(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Err(err)) => Poll::Ready(Err(other_err(err))),
            Poll::Ready(Ok(_)) => Poll::Ready(Ok(())),
        }
    }

    fn start_send(mut self: Pin<&mut Self>, item: Vec<u8>) -> Result<()> {
        Pin::new(&mut self.ws)
            .start_send(Message::Binary(item))
            .map_err(other_err)
    }

    fn poll_flush(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<()>> {
        match Pin::new(&mut self.ws).poll_flush(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Err(err)) => Poll::Ready(Err(other_err(err))),
            Poll::Ready(Ok(_)) => Poll::Ready(Ok(())),
        }
    }

    fn poll_close(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<()>> {
        match Pin::new(&mut self.ws).poll_close(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Err(err)) => Poll::Ready(Err(other_err(err))),
            Poll::Ready(Ok(_)) => Poll::Ready(Ok(())),
        }
    }
}

impl<S> Stream for WsFramed<S>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    type Item = Result<Vec<u8>>;

    fn poll_next(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Self::Item>> {
        let out = match Pin::new(&mut self.ws).poll_next(cx) {
            Poll::Pending => return Poll::Pending,
            Poll::Ready(None) => return Poll::Ready(None),
            Poll::Ready(Some(Err(err))) => {
                return Poll::Ready(Some(Err(other_err(err))))
            }
            Poll::Ready(Some(Ok(item))) => match item {
                Message::Text(s) => s.into_bytes(),
                Message::Binary(b) => b,
                Message::Ping(p) => p,
                Message::Pong(p) => p,
                Message::Close(c) => match c {
                    None => return Poll::Ready(Some(Err(other_err("Closed")))),
                    Some(c) => {
                        let err = other_err(format!("{:?}", c));
                        return Poll::Ready(Some(Err(err)));
                    }
                },
                Message::Frame(f) => f.into_data(),
            },
        };
        Poll::Ready(Some(Ok(out)))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.ws.size_hint()
    }
}
