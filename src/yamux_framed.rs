//! Message framing based on `yamux` to mitigate head-of-line blocking

use crate::*;
use futures::future::BoxFuture;
use futures::stream::BoxStream;
use futures::{AsyncRead, AsyncWrite};
use std::future::Future;
use std::sync::Arc;

const MAX_CONCURRENT_TX: usize = 5;
const MAX_MSG_SIZE: u64 = 64 << 20;

/// Message framing/muxing based on `yamux` to mitigate head-of-line blocking
pub fn yamux_framed_cli<S>(socket: S) -> (YamuxFramedSend, YamuxFramedRecv)
where
    S: 'static + AsyncRead + AsyncWrite + Unpin + Send,
{
    new_yamux(socket, yamux::Mode::Client)
}

/// Message framing/muxing based on `yamux` to mitigate head-of-line blocking
pub fn yamux_framed_srv<S>(socket: S) -> (YamuxFramedSend, YamuxFramedRecv)
where
    S: 'static + AsyncRead + AsyncWrite + Unpin + Send,
{
    new_yamux(socket, yamux::Mode::Server)
}

fn new_yamux<S>(
    socket: S,
    mode: yamux::Mode,
) -> (YamuxFramedSend, YamuxFramedRecv)
where
    S: 'static + AsyncRead + AsyncWrite + Unpin + Send,
{
    let mut config = yamux::Config::default();
    // incoming and outgoing
    config.set_max_num_streams(MAX_CONCURRENT_TX * 2);

    let mut socket = yamux::Connection::new(socket, config, mode);

    let limit = Arc::new(tokio::sync::Semaphore::new(MAX_CONCURRENT_TX));
    let control = socket.control();
    let (s, r) =
        tokio::sync::mpsc::channel::<Result<yamux::Stream>>(MAX_CONCURRENT_TX);

    tokio::task::spawn(async move {
        loop {
            match socket.next_stream().await {
                Err(e) => {
                    let _ = s.send(Err(other_err(e))).await;
                    break;
                }
                Ok(None) => break,
                Ok(Some(stream)) => {
                    // we need to keep polling next_stream even if we
                    // don't have send permits...
                    match s.try_send(Ok(stream)) {
                        Ok(()) => (),
                        Err(tokio::sync::mpsc::error::TrySendError::Full(
                            _,
                        )) => (),
                        Err(_) => break,
                    }
                }
            }
        }
    });

    (YamuxFramedSend { limit, control }, YamuxFramedRecv::new(r))
}

/// Message framing/muxing sender side
pub struct YamuxFramedSend {
    limit: Arc<tokio::sync::Semaphore>,
    control: yamux::Control,
}

impl YamuxFramedSend {
    /// Send a message. Hint: you can use this concurrently with
    /// `Arc<YamuxFramedSend>`
    pub fn send(
        &self,
        data: Vec<u8>,
    ) -> impl Future<Output = Result<()>> + 'static + Send {
        let limit = self.limit.clone();
        let mut control = self.control.clone();
        async move {
            use futures::AsyncWriteExt;
            let _g = limit.acquire().await;
            let mut stream = control.open_stream().await.map_err(other_err)?;
            stream.write_all(&data).await?;
            stream.close().await?;
            Ok(())
        }
    }

    /// Close this connection
    pub async fn close(mut self) -> Result<()> {
        self.control.close().await.map_err(other_err)
    }
}

type SFut = BoxFuture<'static, Result<Vec<u8>>>;
type Unfold = BoxStream<'static, SFut>;
type Buffer = futures::stream::BufferUnordered<Unfold>;

/// Message framing/muxing receiver side
pub struct YamuxFramedRecv {
    recv: Buffer,
}

impl YamuxFramedRecv {
    fn new(r: tokio::sync::mpsc::Receiver<Result<yamux::Stream>>) -> Self {
        use futures::StreamExt;
        let s: Unfold =
            Box::pin(futures::stream::unfold(r, |mut r| async move {
                match r.recv().await {
                    None => None,
                    Some(res) => {
                        let f: SFut = Box::pin(async move {
                            use futures::AsyncReadExt;
                            let res = res?;
                            let mut res = AsyncReadExt::take(res, MAX_MSG_SIZE);
                            let mut out = Vec::new();
                            res.read_to_end(&mut out).await?;
                            Ok(out)
                        });
                        Some((f, r))
                    }
                }
            }));
        let s = s.buffer_unordered(MAX_CONCURRENT_TX);
        Self { recv: s }
    }

    /// Get the next message sent by the remote end of this channel
    pub async fn recv(&mut self) -> Option<Result<Vec<u8>>> {
        use futures::StreamExt;
        self.recv.next().await
    }
}
