use super::*;

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
    /// Construct a new WsFramed as a server-side stream
    pub async fn accept(s: S) -> Result<Self> {
        let ws = tokio_tungstenite::accept_async(s)
            .await
            .map_err(other_err)?;
        Ok(Self {
            ws,
        })
    }

    /// Construct a new WsFramed as a client-side stream
    pub async fn connect(s: S) -> Result<Self> {
        let (ws, _) = tokio_tungstenite::client_async("wss://localhost", s)
            .await
            .map_err(other_err)?;
        Ok(Self {
            ws,
        })
    }
}

impl<S> futures::Sink<BytesList> for WsFramed<S>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    type Error = std::io::Error;

    fn poll_ready(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<()>> {
        match Pin::new(&mut self.ws).poll_ready(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Err(err)) => Poll::Ready(Err(other_err(err))),
            Poll::Ready(Ok(_)) => Poll::Ready(Ok(())),
        }
    }

    fn start_send(mut self: Pin<&mut Self>, mut item: BytesList) -> Result<()> {
        let mut out = Vec::with_capacity(item.remaining());
        item.copy_to_slice(&mut out[..]);
        Pin::new(&mut self.ws)
            .start_send(Message::Binary(out))
            .map_err(other_err)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<()>> {
        match Pin::new(&mut self.ws).poll_flush(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Err(err)) => Poll::Ready(Err(other_err(err))),
            Poll::Ready(Ok(_)) => Poll::Ready(Ok(())),
        }
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<()>> {
        match Pin::new(&mut self.ws).poll_close(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Err(err)) => Poll::Ready(Err(other_err(err))),
            Poll::Ready(Ok(_)) => Poll::Ready(Ok(())),
        }
    }
}

impl<S> futures::Stream for WsFramed<S>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    type Item = Result<BytesList>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let out = match Pin::new(&mut self.ws).poll_next(cx) {
            Poll::Pending => return Poll::Pending,
            Poll::Ready(None) => return Poll::Ready(None),
            Poll::Ready(Some(Err(err))) => return Poll::Ready(Some(Err(other_err(err)))),
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
        Poll::Ready(Some(Ok(out.into())))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.ws.size_hint()
    }
}

impl<S> Framed for WsFramed<S> where S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin {}
