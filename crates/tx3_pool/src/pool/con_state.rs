use super::*;
use futures::future::BoxFuture;
use parking_lot::Mutex;
use std::collections::VecDeque;
use std::pin::Pin;
use std::sync::atomic;
use std::task::Context;
use std::task::Poll;
use std::task::Waker;

#[derive(Clone)]
pub(crate) struct ConState<I: Tx3PoolImp> {
    inner: Arc<Mutex<ConStateInner<I>>>,
}

impl<I: Tx3PoolImp> ConState<I> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        config: Arc<Tx3PoolConfig>,
        pool_term: Term,
        inbound_send: tokio::sync::mpsc::UnboundedSender<InboundMsg>,
        out_con_limit: Arc<tokio::sync::Semaphore>,
        in_byte_limit: Arc<tokio::sync::Semaphore>,
        bindings: Bindings<I>,
        imp: Arc<I>,
        peer_id: Arc<Tx3Id>,
    ) -> Self {
        Self {
            inner: Arc::new(Mutex::new(ConStateInner::new(
                config,
                pool_term,
                inbound_send,
                out_con_limit,
                in_byte_limit,
                bindings,
                imp,
                peer_id,
            ))),
        }
    }

    /// If this returns `false` it can be safely dropped.
    pub fn is_active(&self) -> bool {
        ConStateInner::access(&self.inner, |inner| inner.check_active())
    }

    /// Push a new outgoing message into this state instance.
    pub fn push_outgoing_msg(&self, msg: OutboundMsg) {
        if let Some(waker) = ConStateInner::access(&self.inner, move |inner| {
            inner.push_outgoing_msg(self.inner.clone(), msg)
        }) {
            waker.wake();
        }
    }

    /// Either accept an incoming connection / start processing messages,
    /// or drop it, because we already have an existing connection to this
    /// peer.
    pub fn maybe_use_con(
        &self,
        con_permit: tokio::sync::OwnedSemaphorePermit,
        con: tx3::Tx3Connection,
    ) {
        ConStateInner::access(&self.inner, move |inner| {
            inner.maybe_use_con(self.inner.clone(), con_permit, con)
        });
    }
}

enum ConnectStep {
    NoConnection,
    PendingConnect,
    Connected,
}

impl ConnectStep {
    fn is_active(&self) -> bool {
        match self {
            ConnectStep::NoConnection => false,
            ConnectStep::PendingConnect | ConnectStep::Connected => true,
        }
    }
}

struct ConStateInner<I: Tx3PoolImp> {
    config: Arc<Tx3PoolConfig>,
    pool_term: Term,
    inbound_send: tokio::sync::mpsc::UnboundedSender<InboundMsg>,
    out_con_limit: Arc<tokio::sync::Semaphore>,
    in_byte_limit: Arc<tokio::sync::Semaphore>,
    bindings: Bindings<I>,
    imp: Arc<I>,
    peer_id: Arc<Tx3Id>,
    msg_list: VecDeque<OutboundMsg>,
    connect_step: ConnectStep,
    notify_writer: Arc<tokio::sync::Notify>,
    writer_waker: Option<Waker>,
}

impl<I: Tx3PoolImp> ConStateInner<I> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        config: Arc<Tx3PoolConfig>,
        pool_term: Term,
        inbound_send: tokio::sync::mpsc::UnboundedSender<InboundMsg>,
        out_con_limit: Arc<tokio::sync::Semaphore>,
        in_byte_limit: Arc<tokio::sync::Semaphore>,
        bindings: Bindings<I>,
        imp: Arc<I>,
        peer_id: Arc<Tx3Id>,
    ) -> Self {
        Self {
            config,
            pool_term,
            inbound_send,
            out_con_limit,
            in_byte_limit,
            bindings,
            imp,
            peer_id,
            msg_list: VecDeque::new(),
            connect_step: ConnectStep::NoConnection,
            notify_writer: Arc::new(tokio::sync::Notify::new()),
            writer_waker: None,
        }
    }

    pub fn access<R, F>(this: &Mutex<Self>, f: F) -> R
    where
        F: FnOnce(&mut Self) -> R,
    {
        f(&mut *this.lock())
    }

    fn clear_timeouts(&mut self) {
        let now = tokio::time::Instant::now();
        loop {
            if self.msg_list.is_empty() {
                break;
            }

            if self.msg_list.front().unwrap().timeout_at >= now {
                break;
            }

            let msg = self.msg_list.pop_front().unwrap();
            let _ = msg.resolve.send(Err(other_err("Timeout")));
        }
    }

    fn check_out_con(&mut self, this: Arc<Mutex<ConStateInner<I>>>) {
        if !self.connect_step.is_active() {
            self.connect_step = ConnectStep::PendingConnect;
            let this2 = this.clone();
            self.pool_term.spawn_err(
                open_out_con(
                    self.config.clone(),
                    self.out_con_limit.clone(),
                    self.bindings.clone(),
                    self.imp.clone(),
                    this,
                    self.peer_id.clone(),
                ),
                move |err| {
                    tracing::debug!(?err);
                    ConStateInner::access(&this2, |inner| {
                        inner.connect_step = ConnectStep::NoConnection;
                    });
                },
            );
        }
    }

    pub fn check_active(&mut self) -> bool {
        self.clear_timeouts();
        !self.msg_list.is_empty() || self.connect_step.is_active()
    }

    pub fn push_outgoing_msg(
        &mut self,
        this: Arc<Mutex<ConStateInner<I>>>,
        msg: OutboundMsg,
    ) -> Option<Waker> {
        self.msg_list.push_back(msg);
        self.notify_writer.notify_waiters();
        self.check_out_con(this);
        self.writer_waker.take()
    }

    pub fn get_outgoing_msg(
        &mut self,
        cx: &mut Context<'_>,
    ) -> Option<OutboundMsg> {
        match self.msg_list.pop_front() {
            Some(msg) => Some(msg),
            None => {
                let _waker = cx.waker().clone();
                None
            }
        }
    }

    pub fn maybe_use_con(
        &mut self,
        this: Arc<Mutex<ConStateInner<I>>>,
        con_permit: tokio::sync::OwnedSemaphorePermit,
        con: tx3::Tx3Connection,
    ) {
        match self.connect_step {
            ConnectStep::NoConnection | ConnectStep::PendingConnect => {
                self.connect_step = ConnectStep::Connected;
                spawn_manage_con(
                    self.config.clone(),
                    self.pool_term.clone(),
                    self.inbound_send.clone(),
                    self.in_byte_limit.clone(),
                    self.imp.clone(),
                    self.notify_writer.clone(),
                    this,
                    con_permit,
                    con,
                );
            }
            ConnectStep::Connected => {
                tracing::debug!("dropping con due to already connected");
            }
        }
    }
}

async fn open_out_con<I: Tx3PoolImp>(
    config: Arc<Tx3PoolConfig>,
    out_con_limit: Arc<tokio::sync::Semaphore>,
    bindings: Bindings<I>,
    imp: Arc<I>,
    this: Arc<Mutex<ConStateInner<I>>>,
    peer_id: Arc<Tx3Id>,
) -> Result<()> {
    tokio::time::timeout(config.connect_timeout, async move {
        let con_permit =
            out_con_limit.acquire_owned().await.map_err(other_err)?;

        let addr = imp.get_addr_store().lookup_addr(&peer_id).await?;
        tracing::trace!(?addr, "addr for new out con");

        if !imp.get_pool_hooks().connect_pre(addr.clone()).await {
            return Err(other_err("connect_pre returned false"));
        }

        let node = bindings.get_out_node()?;

        let con = node.connect(addr).await?;

        let this2 = this.clone();
        ConStateInner::access(&this, move |inner| {
            inner.maybe_use_con(this2, con_permit, con)
        });

        Ok(())
    })
    .await
    .map_err(other_err)?
}

struct ManageConDrop<I: Tx3PoolImp> {
    _permit: tokio::sync::OwnedSemaphorePermit,
    this: Arc<Mutex<ConStateInner<I>>>,
}

impl<I: Tx3PoolImp> Drop for ManageConDrop<I> {
    fn drop(&mut self) {
        ConStateInner::access(&self.this, |inner| {
            inner.connect_step = ConnectStep::NoConnection;
        });
    }
}

#[allow(clippy::too_many_arguments)]
fn spawn_manage_con<I: Tx3PoolImp>(
    config: Arc<Tx3PoolConfig>,
    pool_term: Term,
    inbound_send: tokio::sync::mpsc::UnboundedSender<InboundMsg>,
    in_byte_limit: Arc<tokio::sync::Semaphore>,
    _imp: Arc<I>,
    notify_writer: Arc<tokio::sync::Notify>,
    this: Arc<Mutex<ConStateInner<I>>>,
    con_permit: tokio::sync::OwnedSemaphorePermit,
    con: tx3::Tx3Connection,
) {
    let peer_id = con.remote_id().clone();

    let con_term = Term::new("ConClosed", None);

    let con_drop = Arc::new(ManageConDrop {
        _permit: con_permit,
        this: this.clone(),
    });

    let (read_half, write_half) = tokio::io::split(con);

    let want_close = Arc::new(atomic::AtomicBool::new(false));

    let con_start_time = tokio::time::Instant::now();
    let con_tgt_time = con_start_time + config.con_tgt_time;

    {
        let want_close = want_close.clone();
        let notify_writer = notify_writer.clone();
        Term::spawn2(&pool_term, &con_term, async move {
            tokio::time::sleep_until(con_tgt_time).await;
            want_close.store(true, atomic::Ordering::Release);
            notify_writer.notify_waiters();
            Ok(())
        });
    }

    {
        let want_close = want_close.clone();
        let con_term2 = con_term.clone();
        Term::spawn_err2(
            &pool_term,
            &con_term,
            manage_read_con(
                inbound_send,
                in_byte_limit,
                peer_id,
                con_drop.clone(),
                want_close.clone(),
                read_half,
            ),
            move |err| {
                want_close.store(true, atomic::Ordering::Release);
                con_term2.term();
                tracing::debug!(?err);
            },
        );
    }

    let con_term2 = con_term.clone();
    Term::spawn_err2(
        &pool_term,
        &con_term,
        manage_write_con(con_drop, notify_writer, this, want_close, write_half),
        move |err| {
            con_term2.term();
            tracing::debug!(?err);
        },
    );
}

async fn manage_read_con<I: Tx3PoolImp>(
    inbound_send: tokio::sync::mpsc::UnboundedSender<InboundMsg>,
    in_byte_limit: Arc<tokio::sync::Semaphore>,
    peer_id: Arc<Tx3Id>,
    con_drop: Arc<ManageConDrop<I>>,
    want_close: Arc<atomic::AtomicBool>,
    mut con: tokio::io::ReadHalf<tx3::Tx3Connection>,
) -> Result<()> {
    // keep this until this task drops
    let _con_drop = con_drop;

    let mut buf = [0; 4096];
    let mut buf_data = BytesList::new();
    let mut next_size = None;
    let mut byte_permit: Option<tokio::sync::OwnedSemaphorePermit> = None;
    let mut byte_permit_fut: Option<
        BoxFuture<'static, Result<tokio::sync::OwnedSemaphorePermit>>,
    > = None;
    futures::future::poll_fn(move |cx| {
        use tokio::io::AsyncRead;

        'read_loop: loop {
            {
                let mut buf = tokio::io::ReadBuf::new(&mut buf);
                match Pin::new(&mut con).poll_read(cx, &mut buf) {
                    Poll::Pending => return Poll::Pending,
                    Poll::Ready(Err(err)) => return Poll::Ready(Err(err)),
                    Poll::Ready(Ok(_)) => {
                        let filled = buf.filled();
                        if filled.is_empty() {
                            // the stream is closed
                            // exit gracefully
                            // but we *still* need to set want_close
                            // so that the write side knows to
                            // start shutting down
                            want_close.store(true, atomic::Ordering::Release);
                            return Poll::Ready(Ok(()));
                        }
                        tracing::trace!(byte_count = ?filled.len(), "read bytes");
                        buf_data
                            .push(bytes::Bytes::copy_from_slice(filled));
                    }
                }
            }

            loop {
                if next_size.is_none() {
                    if buf_data.remaining() < 4 {
                        continue 'read_loop;
                    }
                    next_size = Some(buf_data.get_u32_le());
                }

                let next_size_unwrapped = *next_size.as_ref().unwrap();
                if buf_data.remaining() < next_size_unwrapped as usize {
                    continue 'read_loop;
                }

                tracing::trace!(?next_size_unwrapped, "read len");

                if byte_permit.is_none() {
                    if byte_permit_fut.is_none() {
                        let limit = in_byte_limit.clone();
                        byte_permit_fut = Some(Box::pin(async move {
                            limit
                                .acquire_owned()
                                .await
                                .map_err(other_err)
                        }));
                    }

                    match byte_permit_fut.as_mut().unwrap().as_mut().poll(cx) {
                        Poll::Pending => return Poll::Pending,
                        Poll::Ready(Err(err)) => return Poll::Ready(Err(err)),
                        Poll::Ready(Ok(permit)) => {
                            byte_permit = Some(permit);
                            byte_permit_fut.take();
                        }
                    }
                }

                next_size = None;
                let message = InboundMsg {
                    _permit: byte_permit.take().unwrap(),
                    peer_id: peer_id.clone(),
                    content: buf_data.take_front(next_size_unwrapped as usize),
                };

                if inbound_send.send(message).is_err() {
                    return Poll::Ready(Err(other_err("InboundClosed")));
                }
            }
        }
    })
    .await
}

async fn manage_write_con<I: Tx3PoolImp>(
    con_drop: Arc<ManageConDrop<I>>,
    notify_writer: Arc<tokio::sync::Notify>,
    this: Arc<Mutex<ConStateInner<I>>>,
    want_close: Arc<atomic::AtomicBool>,
    mut con: tokio::io::WriteHalf<tx3::Tx3Connection>,
) -> Result<()> {
    // keep this until this task drops
    let _con_drop = con_drop;

    let mut notify_writer_fut: Option<BoxFuture<'static, ()>> = None;
    let mut cur_msg = None;
    futures::future::poll_fn(move |cx| {
        use tokio::io::AsyncWrite;

        loop {
            // make sure we get a pending on a still outstanding
            // notify_writer_fut
            loop {
                if notify_writer_fut.is_none() {
                    let notify_writer = notify_writer.clone();
                    notify_writer_fut = Some(Box::pin(async move {
                        notify_writer.notified().await;
                    }));
                }

                match notify_writer_fut.as_mut().unwrap().as_mut().poll(cx) {
                    Poll::Pending => break,
                    Poll::Ready(_) => {
                        notify_writer_fut.take();
                    }
                }
            }

            if cur_msg.is_none() {
                if want_close.load(atomic::Ordering::Acquire) {
                    return Poll::Ready(Ok(()));
                }

                match ConStateInner::access(&this, |inner| {
                    inner.get_outgoing_msg(cx)
                }) {
                    None => return Poll::Pending,
                    Some(mut msg) => {
                        let len = msg.content.remaining() as u32;
                        let len = len.to_le_bytes();
                        let len = bytes::Bytes::copy_from_slice(&len[..]);
                        msg.content.0.push_front(len);
                        cur_msg = Some(msg);
                    }
                }
            }

            match Pin::new(&mut con)
                .poll_write(cx, cur_msg.as_ref().unwrap().content.chunk())
            {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Err(err)) => {
                    let err2 = other_err(format!("{:?}", err));
                    if let Some(msg) = cur_msg.take() {
                        let _ = msg.resolve.send(Err(err));
                    }
                    return Poll::Ready(Err(err2));
                }
                Poll::Ready(Ok(byte_count)) => {
                    tracing::trace!(?byte_count, "wrote bytes");
                    let msg = cur_msg.as_mut().unwrap();
                    msg.content.advance(byte_count);
                    if !msg.content.has_remaining() {
                        tracing::trace!("completed msg send");
                        if let Some(msg) = cur_msg.take() {
                            let _ = msg.resolve.send(Ok(()));
                        }

                        if want_close.load(atomic::Ordering::Acquire) {
                            return Poll::Ready(Ok(()));
                        }
                    }
                }
            }
        }
    })
    .await
}
