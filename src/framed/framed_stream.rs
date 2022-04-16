use super::*;

/// A stream backed by a websocket
pub struct FramedStream<F>
where
    F: Framed + Unpin,
{
    framed: F,
    pend_read: Option<BytesList>,
}

impl<F> FramedStream<F>
where
    F: Framed + Unpin,
{
    /// Construct a new FramedStream
    pub fn new(framed: F) -> Self {
        Self {
            framed,
            pend_read: None,
        }
    }
}

impl<F> tokio::io::AsyncWrite for FramedStream<F>
where
    F: Framed + Unpin,
{
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize>> {
        let mut framed = Pin::new(&mut self.framed);

        if let Poll::Pending = framed
            .as_mut()
            .poll_ready(cx)
            .map_err(other_err)?
        {
            return Poll::Pending;
        }

        // Would be nice to have a way of buffering up smaller messages
        // and sending them as a unit, to avoid some of the websocket overhead
        let take_len = std::cmp::min(buf.len(), 16 << 20);

        framed.start_send(bytes::Bytes::copy_from_slice(&buf[..take_len]).into())?;

        Poll::Ready(Ok(take_len))
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<()>> {
        Pin::new(&mut self.framed)
            .poll_flush(cx)
            .map_err(other_err)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<()>> {
        Pin::new(&mut self.framed)
            .poll_close(cx)
            .map_err(other_err)
    }
}

impl<F> tokio::io::AsyncRead for FramedStream<F>
where
    F: Framed + Unpin,
{
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<Result<()>> {
        let Self { framed, pend_read } = &mut *self;

        if pend_read.is_none() {
            match Pin::new(ws).poll_next(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(None) => return Poll::Ready(Err(other_err("SocketClosed"))),
                Poll::Ready(Some(Err(err))) => return Poll::Ready(Err(err)),
                Poll::Ready(Some(Ok(l))) => *pend_read = Some(l),
            }
        }

        let mut drop = false;

        {
            let list = pend_read.as_mut().unwrap();

            loop {
                let c = list.chun
                let put_len = std::cmp::min(buf.remaining(), list.remaining());
                if put_len < 1 {
                    break;
                }
                buf.put_slice(

            buf.put_slice(&pr[*cur..*cur + put_len]);
            *cur += put_len;

            if *cur >= put_len {
                drop = true;
            }
        }

        if drop {
            *pend_read = None;
        }

        Poll::Ready(Ok(()))
    }
}
