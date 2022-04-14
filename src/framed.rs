//! Wrapper types for framed messages

use crate::*;
use std::task::Context;
use std::task::Poll;
use std::pin::Pin;
use tokio_tungstenite::tungstenite::Message;

/// Represents an IO device that can send / recv framed binary data
pub trait Framed: futures_util::Sink<Vec<u8>> + futures_util::Stream<Item = Result<Vec<u8>>> {}

/// Wrapper struct to turn a tokio_tungstenite websocket stream into a Framed
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
    /// Construct a new WsFramed newtype wrapper
    pub fn new(ws: tokio_tungstenite::WebSocketStream<S>) -> Self {
        Self {
            ws,
        }
    }
}

impl<S> futures_util::Sink<Vec<u8>> for WsFramed<S>
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

impl<S> futures_util::Stream for WsFramed<S>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    type Item = Result<Vec<u8>>;

    fn poll_next(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Self::Item>> {
        match Pin::new(&mut self.ws).poll_next(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Ready(Some(Err(err))) => Poll::Ready(Some(Err(other_err(err)))),
            Poll::Ready(Some(Ok(item))) => {
                Poll::Ready(Some(Ok(match item {
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
                    }
                    Message::Frame(f) => f.into_data(),
                })))
            }
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.ws.size_hint()
    }
}

impl<S> Framed for WsFramed<S>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{}
