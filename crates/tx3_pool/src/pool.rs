//! Tx3 connection pool types.
//!
//! Notes on tx3 connection handling. The optimization tradeoff here
//! is the ability to speak to a large number of remote peers while only
//! holding open connections to a handful of them at the same time.
//! Rather than trying to prioritize keeping connections open to avoid
//! the setup cost, we are choosing to accept this cost, intentionally
//! rotating our connections closed so we can open new ones to new peers.
//!
//! ### Configuration Parameters
//!
//! These parameters are currently private so all nodes use the same values
//! until we implement some kind of negotiation.
//!
//! - `con_tgt_time` - a time target correlated to how long we'd
//!   ideally like connections to remain active once established.
//!   Default: 4 seconds.
//!   - unless "want_close" is set, the write side will remain open for at
//!     least this long, awaiting any late outgoing messages.
//!   - "want_close" will automatically be set after this time.
//!   - this time is used as the time window for "rate minimum" checking.
//! - `rate_min_bytes_per_s` - as opposed to a simple idle
//!   timeout, we also want to mitigate malicious actors opening connections
//!   and stalling them with slow byte transmisions. This "rate minimum"
//!   requires connections to send / receive data at a minimum rate, or
//!   be considered idle and closed.
//!   Default: 65_536 (524,288 bps up + down).
//!
//! ### The "want_close" Flag
//!
//! "want_close" only ever affects the behavior of the write side.
//! The read side does its own timing checks described below under
//! "Read Timeout".
//!
//! The "want_close" flag is set in two scenarios:
//! - When `con_tgt_time` has elapsed.
//! - When the read side of a connection ends, errors, or times out.
//!
//! When the write side of a connection has completed sending a full message,
//! it checks the "want_close" flag. If it is set, it processes a "shutdown",
//! and is dropped. It does not look for more data to write.
//!
//! ### "Rate Minimum" Checking
//!
//! After `con_tgt_time` has elapsed, "rate minimum" checking begins. For
//! the previous sliding `con_tgt_time` window, if the average combined
//! bytes received and bytes sent per second is ever below
//! `rate_min_bytes_per_s`, both read and write sides of the connection
//! are closed immediately without awaiting any completion of current
//! reads or writes.
//!
//! ### Read Timeout
//!
//! If TWICE `con_tgt_time` has elapsed on the read side and the current
//! message completes, the read side will be dropped without attempting
//! to read any bytes of the next message.
//!
//! Waiting twice the time is to mitigate subjective connection start time
//! differences between the peers.

use crate::types::*;
use crate::*;
use bytes::Buf;
use std::future::Future;
use std::io::Result;
use std::net::SocketAddr;
use std::sync::Arc;

mod bindings;
use bindings::*;

mod pool_state;
use pool_state::*;

mod con_state;

/// Tx3 pool inbound message receiver.
pub struct Tx3Recv {
    // safe to be unbounded, because pool logic keeps send permits
    // in the inbound msg struct... we only drop these permits
    // when the user of this recv instance actually pulls a message.
    inbound_recv: tokio::sync::mpsc::UnboundedReceiver<InboundMsg>,
}

impl Tx3Recv {
    /// Get the next inbound message received by this pool instance.
    pub async fn recv(&mut self) -> Option<(Arc<Tx3Id>, BytesList)> {
        self.inbound_recv.recv().await.map(|msg| msg.extract())
    }
}

/// Handle allowing a particular binding (listener) to be dropped.
/// If this handle is dropped, the binding will remain open, only terminating
/// on error, or if the process is shut down.
pub struct BindHnd {
    bind_term: Term,
}

impl BindHnd {
    /// Terminate this specific listener, the rest of the pool will remain
    /// active, and any open connections will remain active, we
    /// just stop accepting incoming connections from this listener.
    pub fn terminate(&self) {
        self.bind_term.term();
    }

    // -- private -- //

    fn priv_new(bind_term: Term) -> Self {
        Self { bind_term }
    }
}

/// Tx3 connection pool.
pub struct Tx3Pool<I: Tx3PoolImp> {
    config: Arc<Tx3PoolConfig>,
    #[allow(dead_code)]
    tls: tx3::tls::TlsConfig,
    pool_term: Term,
    imp: Arc<I>,
    bindings: Bindings<I>,
    out_byte_limit: Arc<tokio::sync::Semaphore>,
    cmd_send: tokio::sync::mpsc::UnboundedSender<PoolStateCmd>,
}

impl<I: Tx3PoolImp> Drop for Tx3Pool<I> {
    fn drop(&mut self) {
        self.pool_term.term();
    }
}

impl<I: Tx3PoolImp> Tx3Pool<I> {
    /// Construct a new generic connection pool.
    pub async fn new(
        mut config: Tx3PoolConfig,
        imp: Arc<I>,
    ) -> Result<(Arc<Self>, Tx3Recv)> {
        let tls = match config.tls.take() {
            Some(tls) => tls,
            None => tx3::tls::TlsConfigBuilder::default().build()?,
        };
        let config = Arc::new(config);

        let (cmd_send, cmd_recv) = tokio::sync::mpsc::unbounded_channel();

        let pool_term = Term::new("PoolClosed", None);

        let bindings = Bindings::new(
            config.clone(),
            pool_term.clone(),
            imp.clone(),
            tls.clone(),
            cmd_send.clone(),
        )
        .await?;

        let out_byte_limit =
            Arc::new(tokio::sync::Semaphore::new(config.max_out_byte_count));
        let (inbound_send, inbound_recv) =
            tokio::sync::mpsc::unbounded_channel();

        pool_term.spawn(pool_state_task(
            config.clone(),
            pool_term.clone(),
            bindings.clone(),
            imp.clone(),
            inbound_send,
            cmd_send.clone(),
            cmd_recv,
        ));

        let this = Self {
            config,
            tls,
            pool_term,
            imp,
            bindings,
            out_byte_limit,
            cmd_send,
        };

        let recv = Tx3Recv { inbound_recv };

        Ok((Arc::new(this), recv))
    }

    /// Access the internal implementation.
    pub fn as_imp(&self) -> &Arc<I> {
        &self.imp
    }

    /// Get the address at which this pool is externally reachable.
    pub fn local_addr(&self) -> Arc<Tx3Addr> {
        self.bindings.local_addr()
    }

    /// Bind a local listener into this pool, and begin listening
    /// to incoming messages.
    pub async fn bind<A: tx3::IntoAddr>(&self, bind: A) -> Result<BindHnd> {
        self.bindings.bind(bind).await
    }

    /// Enqueue an outgoing message for send to a remote peer.
    pub fn send<B: Into<BytesList>>(
        &self,
        peer_id: Arc<Tx3Id>,
        content: B,
        timeout: std::time::Duration,
    ) -> impl Future<Output = Result<()>> + 'static + Send {
        let pool_term = self.pool_term.clone();
        let max_msg_byte_count = self.config.max_msg_byte_count;
        let content = content.into();
        let timeout_at = tokio::time::Instant::now() + timeout;
        let out_byte_limit = self.out_byte_limit.clone();
        let cmd_send = self.cmd_send.clone();
        async move {
            let rem = content.remaining();
            if rem > max_msg_byte_count as usize {
                return Err(other_err("MsgTooLarge"));
            }

            tokio::select! {
                _ = pool_term.on_term() => Err(other_err("PoolClosed")),
                r = tokio::time::timeout_at(timeout_at, async move {
                    let permit = out_byte_limit
                        .acquire_many_owned(rem as u32)
                        .await
                        .map_err(|_| other_err("PoolClosed"))?;

                    let (resolve, result) = tokio::sync::oneshot::channel();

                    let msg = OutboundMsg {
                        _permit: permit,
                        peer_id,
                        content,
                        timeout_at,
                        resolve
                    };

                    cmd_send
                        .send(PoolStateCmd::OutboundMsg(msg))
                        .map_err(|_| other_err("PoolClosed"))?;

                    result.await.map_err(|_| other_err("PoolClosed"))?
                }) => r.map_err(|_| other_err("Timeout"))?,
            }
        }
    }

    /// Immediately terminate all connections, stopping all processing, both
    /// incoming and outgoing. This is NOT a graceful shutdown, and probably
    /// should only be used in testing scenarios.
    pub fn terminate(&self) {
        self.pool_term.term();
    }
}
