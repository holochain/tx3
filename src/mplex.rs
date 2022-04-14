//! Stream multiplexing

use crate::*;
use crate::framed::*;
use std::task::Poll;
use std::task::Context;
use std::pin::Pin;
use tokio::sync::Semaphore;
use tokio::sync::OwnedSemaphorePermit;
use std::sync::Arc;
use futures_util::future::BoxFuture;
use std::future::Future;
use std::sync::atomic;

mod mark;
pub use mark::*;

/// Construct a new mplex handle / incoming receiver stream pair
///
/// Panics if `max_bytes > (u32::MAX >> 3)`
pub fn new_mplex<F>(
    _framed: F,
    max_bytes: u32,
) -> (Mplex, MplexInbound)
where
    F: Framed + Unpin,
{
    if max_bytes > (u32::MAX >> 3) {
        panic!("max_bytes cannot be > (u32::MAX >> 3)");
    }

    let next_id = Arc::new(atomic::AtomicU64::new(0));

    let out_limit = Arc::new(Semaphore::new(max_bytes as usize));
    let (out_send, _out_recv) = tokio::sync::mpsc::unbounded_channel();

    let (_inbound_send, inbound_recv) = tokio::sync::mpsc::channel(32);

    let hnd = Mplex {
        next_id,
        out_limit,
        out_send,
    };

    let recv = MplexInbound {
        inbound_recv,
    };

    (hnd, recv)
}

enum OutCmd {
    Open(u64),
    Write(u64, bytes::Bytes, OwnedSemaphorePermit),
    Flush(u64, tokio::sync::oneshot::Sender<Result<()>>),
    Shutdown(u64, tokio::sync::oneshot::Sender<Result<()>>),
}

type OutboundSend = tokio::sync::mpsc::UnboundedSender<OutCmd>;

/// Multiplexer async IO stream
pub struct MplexStream {
    stream_id: u64,
    out_limit: Arc<Semaphore>,
    out_send: OutboundSend,
    want_out_permit: Option<(usize, BoxFuture<'static, OwnedSemaphorePermit>)>,
    want_flush: Option<tokio::sync::oneshot::Receiver<Result<()>>>,
    want_shutdown: Option<tokio::sync::oneshot::Receiver<Result<()>>>,
}

impl tokio::io::AsyncWrite for MplexStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize>> {
        let buf_len = buf.len();
        if buf_len == 0 {
            panic!("invalid empty buf");
        }
        let take_len = std::cmp::min(512, buf_len);
        if let Some((l, _)) = &self.want_out_permit {
            if *l != take_len {
                drop(self.want_out_permit.take());
            }
        }
        if self.want_out_permit.is_none() {
            let out_limit = self.out_limit.clone();
            self.want_out_permit = Some((take_len, Box::pin(async move {
                out_limit.acquire_many_owned(take_len as u32).await.unwrap()
            })));
        }
        let permit = match self.want_out_permit.as_mut().unwrap().1.as_mut().poll(cx) {
            Poll::Pending => return Poll::Pending,
            Poll::Ready(permit) => permit,
        };
        let b = bytes::Bytes::copy_from_slice(&buf[..take_len]);
        let _ = self.out_send.send(OutCmd::Write(self.stream_id, b, permit));
        Poll::Ready(Ok(take_len))
    }

    fn poll_flush(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<()>> {
        if self.want_flush.is_none() {
            let (s, r) = tokio::sync::oneshot::channel();
            let _ = self.out_send.send(OutCmd::Flush(self.stream_id, s));
            self.want_flush = Some(r);
        }
        match Pin::new(self.want_flush.as_mut().unwrap()).poll(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Err(_)) => Poll::Ready(Err(other_err("SocketClosed"))),
            Poll::Ready(Ok(r)) => Poll::Ready(r),
        }
    }

    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<()>> {
        if self.want_shutdown.is_none() {
            let (s, r) = tokio::sync::oneshot::channel();
            let _ = self.out_send.send(OutCmd::Shutdown(self.stream_id, s));
            self.want_shutdown = Some(r);
        }
        match Pin::new(self.want_shutdown.as_mut().unwrap()).poll(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Err(_)) => Poll::Ready(Err(other_err("SocketClosed"))),
            Poll::Ready(Ok(r)) => Poll::Ready(r),
        }
    }
}

/// Multiplexer incoming receiver stream
pub struct MplexInbound {
    inbound_recv: tokio::sync::mpsc::Receiver<Result<MplexStream>>,
}

impl futures_util::Stream for MplexInbound {
    type Item = Result<MplexStream>;

    fn poll_next(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Self::Item>> {
        self.inbound_recv.poll_recv(cx)
    }
}

/// Multiplexer built atop a Framed IO device
pub struct Mplex{
    next_id: Arc<atomic::AtomicU64>,
    out_limit: Arc<Semaphore>,
    out_send: OutboundSend,
}

impl Mplex {
    /// Open a new outgoing stream to the remote
    pub fn open_stream(&self) -> MplexStream {
        let stream_id = self.next_id.fetch_add(1, atomic::Ordering::Relaxed);
        let _ = self.out_send.send(OutCmd::Open(stream_id));
        MplexStream {
            stream_id,
            out_limit: self.out_limit.clone(),
            out_send: self.out_send.clone(),
            want_out_permit: None,
            want_flush: None,
            want_shutdown: None,
        }
    }
}
