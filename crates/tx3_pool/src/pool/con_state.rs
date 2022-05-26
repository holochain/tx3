use super::*;
use futures::future::BoxFuture;
use parking_lot::Mutex;
use std::collections::VecDeque;
use std::pin::Pin;
use std::task::Poll;

#[derive(Clone)]
pub(crate) struct ConState<I: Tx3PoolImp> {
    inner: Arc<Mutex<ConStateInner<I>>>,
}

impl<I: Tx3PoolImp> ConState<I> {
    pub fn new(
        inbound_send: tokio::sync::mpsc::UnboundedSender<InboundMsg>,
        out_con_limit: Arc<tokio::sync::Semaphore>,
        in_byte_limit: Arc<tokio::sync::Semaphore>,
        bindings: Bindings<I>,
        imp: Arc<I>,
        peer_id: Arc<Tx3Id>,
    ) -> Self {
        Self {
            inner: Arc::new(Mutex::new(ConStateInner::new(
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
        ConStateInner::access(&self.inner, |inner| inner.is_active())
    }

    /// Push a new outgoing message into this state instance.
    pub fn push_outgoing_msg(&self, msg: OutboundMsg) {
        ConStateInner::access(&self.inner, move |inner| {
            inner.push_outgoing_msg(self.inner.clone(), msg)
        });
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
    inbound_send: tokio::sync::mpsc::UnboundedSender<InboundMsg>,
    out_con_limit: Arc<tokio::sync::Semaphore>,
    in_byte_limit: Arc<tokio::sync::Semaphore>,
    bindings: Bindings<I>,
    imp: Arc<I>,
    peer_id: Arc<Tx3Id>,
    msg_list: VecDeque<OutboundMsg>,
    connect_step: ConnectStep,
    out_notify: Arc<tokio::sync::Notify>,
}

impl<I: Tx3PoolImp> ConStateInner<I> {
    pub fn new(
        inbound_send: tokio::sync::mpsc::UnboundedSender<InboundMsg>,
        out_con_limit: Arc<tokio::sync::Semaphore>,
        in_byte_limit: Arc<tokio::sync::Semaphore>,
        bindings: Bindings<I>,
        imp: Arc<I>,
        peer_id: Arc<Tx3Id>,
    ) -> Self {
        Self {
            inbound_send,
            out_con_limit,
            in_byte_limit,
            bindings,
            imp,
            peer_id,
            msg_list: VecDeque::new(),
            connect_step: ConnectStep::NoConnection,
            out_notify: Arc::new(tokio::sync::Notify::new()),
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
            tokio::task::spawn(open_out_con(
                self.out_con_limit.clone(),
                self.bindings.clone(),
                self.imp.clone(),
                this,
                self.peer_id.clone(),
            ));
        }
    }

    #[allow(clippy::wrong_self_convention)]
    pub fn is_active(&mut self) -> bool {
        self.clear_timeouts();
        !self.msg_list.is_empty() || self.connect_step.is_active()
    }

    pub fn push_outgoing_msg(
        &mut self,
        this: Arc<Mutex<ConStateInner<I>>>,
        msg: OutboundMsg,
    ) {
        self.msg_list.push_back(msg);
        self.out_notify.notify_waiters();
        self.check_out_con(this);
    }

    pub fn get_outgoing_msg(
        &mut self,
    ) -> std::result::Result<OutboundMsg, BoxFuture<'static, ()>> {
        if let Some(msg) = self.msg_list.pop_front() {
            Ok(msg)
        } else {
            let out_notify = self.out_notify.clone();
            Err(Box::pin(async move {
                out_notify.notified().await;
            }))
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
                tokio::task::spawn(manage_con(
                    self.inbound_send.clone(),
                    self.in_byte_limit.clone(),
                    self.imp.clone(),
                    this,
                    con_permit,
                    con,
                ));
            }
            ConnectStep::Connected => {
                tracing::debug!("dropping con due to already connected");
            }
        }
    }
}

async fn open_out_con<I: Tx3PoolImp>(
    out_con_limit: Arc<tokio::sync::Semaphore>,
    bindings: Bindings<I>,
    imp: Arc<I>,
    this: Arc<Mutex<ConStateInner<I>>>,
    peer_id: Arc<Tx3Id>,
) {
    if let Err(err) =
        open_out_con_inner(out_con_limit, bindings, imp, this.clone(), peer_id)
            .await
    {
        tracing::debug!(?err);
        ConStateInner::access(&this, |inner| {
            inner.connect_step = ConnectStep::NoConnection;
        });
    }
}

async fn open_out_con_inner<I: Tx3PoolImp>(
    out_con_limit: Arc<tokio::sync::Semaphore>,
    bindings: Bindings<I>,
    imp: Arc<I>,
    this: Arc<Mutex<ConStateInner<I>>>,
    peer_id: Arc<Tx3Id>,
) -> Result<()> {
    let con_permit = out_con_limit.acquire_owned().await.map_err(other_err)?;

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

async fn manage_con<I: Tx3PoolImp>(
    inbound_send: tokio::sync::mpsc::UnboundedSender<InboundMsg>,
    in_byte_limit: Arc<tokio::sync::Semaphore>,
    _imp: Arc<I>,
    this: Arc<Mutex<ConStateInner<I>>>,
    con_permit: tokio::sync::OwnedSemaphorePermit,
    con: tx3::Tx3Connection,
) {
    let peer_id = con.remote_id().clone();

    let con_term = Term::new(None);

    let con_drop = Arc::new(ManageConDrop {
        _permit: con_permit,
        this: this.clone(),
    });

    let (mut con, write_half) = tokio::io::split(con);

    tokio::task::spawn(manage_write_con(
        con_drop.clone(),
        con_term.clone(),
        this.clone(),
        write_half,
    ));

    // -- this task is now the "read side" -- //

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
                    Poll::Ready(Err(err)) => {
                        tracing::debug!(?err);
                        con_term.term();
                        return Poll::Ready(());
                    }
                    Poll::Ready(Ok(_)) => {
                        let filled = buf.filled();
                        if filled.is_empty() {
                            tracing::debug!("eof");
                            con_term.term();
                            return Poll::Ready(());
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
                        Poll::Ready(Err(err)) => {
                            tracing::debug!(?err);
                            con_term.term();
                            return Poll::Ready(());
                        }
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
                    return Poll::Ready(());
                }
            }
        }
    })
    .await;
}

async fn manage_write_con<I: Tx3PoolImp>(
    con_drop: Arc<ManageConDrop<I>>,
    con_term: Term,
    this: Arc<Mutex<ConStateInner<I>>>,
    mut con: tokio::io::WriteHalf<tx3::Tx3Connection>,
) {
    // keep this until this task drops
    let _con_drop = con_drop;

    let mut out_notify_fut: Option<BoxFuture<'static, ()>> = None;
    let mut cur_msg = None;
    futures::future::poll_fn(move |cx| {
        use tokio::io::AsyncWrite;

        'write_loop: loop {
            if out_notify_fut.is_some() {
                match out_notify_fut.as_mut().unwrap().as_mut().poll(cx) {
                    Poll::Pending => return Poll::Pending,
                    Poll::Ready(_) => {
                        out_notify_fut.take();
                    }
                }
            }

            if cur_msg.is_none() {
                let res = ConStateInner::access(&this, |inner| {
                    match inner.get_outgoing_msg() {
                        Ok(msg) => Ok(msg),
                        Err(mut fut) => {
                            // we need to poll this fut once before releasing
                            // the lock
                            Err(match fut.as_mut().poll(cx) {
                                Poll::Pending => (fut, Poll::Pending),
                                Poll::Ready(_) => (fut, Poll::Ready(())),
                            })
                        }
                    }
                });

                match res {
                    Ok(mut msg) => {
                        let len = msg.content.remaining() as u32;
                        let len = len.to_le_bytes();
                        let len = bytes::Bytes::copy_from_slice(&len[..]);
                        msg.content.0.push_front(len);
                        cur_msg = Some(msg);
                    }
                    Err((fut, Poll::Pending)) => {
                        out_notify_fut = Some(fut);
                        return Poll::Pending;
                    }
                    Err((_fut, Poll::Ready(_))) => {
                        // erm... there was no message, but we got
                        // an immediate message available notification
                        // i guess, re-run the loop?
                        continue 'write_loop;
                    }
                }
            }

            match Pin::new(&mut con)
                .poll_write(cx, cur_msg.as_ref().unwrap().content.chunk())
            {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Err(err)) => {
                    tracing::debug!(?err);
                    if let Some(msg) = cur_msg.take() {
                        let _ = msg.resolve.send(Err(err));
                    }
                    con_term.term();
                    return Poll::Ready(());
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
                    }
                }
            }
        }
    })
    .await;
}
