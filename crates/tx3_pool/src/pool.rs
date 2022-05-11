#![allow(dead_code)]
use crate::types::*;
use crate::*;

const FI_LEN_MASK: u32 = 0b00000011111111111111111111111111;
//const FI_OTH_MASK: u32 = 0b11111100000000000000000000000000;

/// Tx3 pool configuration.
#[non_exhaustive]
pub struct Tx3PoolConfig {}

impl Default for Tx3PoolConfig {
    fn default() -> Self {
        Self {}
    }
}

impl Tx3PoolConfig {
    /// Construct a new default Tx3PoolConfig.
    pub fn new() -> Self {
        Tx3PoolConfig::default()
    }
}

/// Tx3 incoming message stream.
pub struct Tx3PoolIncoming<T: Tx3Transport> {
    in_recv: tokio::sync::mpsc::UnboundedReceiver<Message<T>>,
}

impl<T: Tx3Transport> Tx3PoolIncoming<T> {
    /// Pull the next incoming message from the receive stream.
    pub async fn recv(
        &mut self,
    ) -> Option<(
        Arc<<<T as Tx3Transport>::Common as Tx3TransportCommon>::EndpointId>,
        BytesList,
    )> {
        let msg = self.in_recv.recv().await?;
        Some(msg.complete())
    }
}

/// Tx3 pool.
pub struct Tx3Pool<T: Tx3Transport> {
    task_cmd_send: tokio::sync::mpsc::UnboundedSender<PoolTaskCmd<T>>,
    pool_term: Term,
    new_msg_limit: Arc<tokio::sync::Semaphore>,
}

impl<T: Tx3Transport> Tx3Pool<T> {
    fn priv_new(
        task_cmd_send: tokio::sync::mpsc::UnboundedSender<PoolTaskCmd<T>>,
        pool_term: Term,
    ) -> Self {
        let new_msg_limit = Arc::new(tokio::sync::Semaphore::new(1));
        Self {
            task_cmd_send,
            pool_term,
            new_msg_limit,
        }
    }

    /// Bind a new transport backend, wrapping it in Tx3Pool logic.
    pub async fn bind(
        transport: T,
        path: Arc<T::BindPath>,
    ) -> Result<(T::BindAppData, Self, Tx3PoolIncoming<T>)> {
        let (app, _connector, _acceptor) = transport.bind(path).await?;
        let (task_cmd_send, task_cmd_recv) =
            tokio::sync::mpsc::unbounded_channel();
        let pool_term = Term::new();
        let this = Tx3Pool::priv_new(task_cmd_send, pool_term.clone());
        let (_in_send, in_recv) = tokio::sync::mpsc::unbounded_channel();
        let incoming = Tx3PoolIncoming { in_recv };
        tokio::task::spawn(pool_task(task_cmd_recv, pool_term));
        Ok((app, this, incoming))
    }

    /// Attempt to send a framed message to a target node.
    ///
    /// This can experience backpressure in two ways:
    /// - There is no active connection and we are at our max connection count,
    ///   and are unable to close any per ongoing activity.
    /// - The backpressure of sending to the outgoing transport connection.
    ///
    /// This call can timeout per the timeout specified in Tx3PoolConfig.
    ///
    /// This future will resolve Ok() when all data has been offloaded to
    /// the underlying transport
    pub async fn send<B: Into<BytesList>>(
        &self,
        dst: Arc<
            <<T as Tx3Transport>::Common as Tx3TransportCommon>::EndpointId,
        >,
        data: B,
    ) -> Result<()> {
        let mut data = data.into();
        let rem = data.remaining();
        if rem > FI_LEN_MASK as usize {
            return Err(other_err("MsgTooLarge"));
        }
        data.0.push_front(bytes::Bytes::copy_from_slice(
            &(rem as u32).to_le_bytes()[..],
        ));
        // TODO _ FIXME - this actually needs to be the outgoing bytes limit
        let permit = self.new_msg_limit.clone().acquire_owned().await.unwrap();
        let (res_s, res_r) = tokio::sync::oneshot::channel();
        let msg_res = MsgRes::new(Some(res_s));
        let msg = <Message<T>>::new(dst, data, permit, Some(msg_res));
        // we never close this semaphore, so safe to unwrap
        let permit = self.new_msg_limit.clone().acquire_owned().await.unwrap();
        self.task_cmd_send
            .send(PoolTaskCmd::NewMsg(msg, permit))
            .map_err(|_| other_err("PollTaskClosed"))?;
        // because of the Drop impl on msg, this resolve should never err
        res_r.await.unwrap()
    }

    /// Immediately terminate all connections, stopping all processing, both
    /// incoming and outgoing. This is NOT a graceful shutdown, and probably
    /// should only be used in testing scenarios.
    pub fn terminate(&self) {
        self.pool_term.term();
    }
}

enum PoolTaskCmd<T: Tx3Transport> {
    NewMsg(Message<T>, tokio::sync::OwnedSemaphorePermit),
}

async fn pool_task<T: Tx3Transport>(
    cmd_recv: tokio::sync::mpsc::UnboundedReceiver<PoolTaskCmd<T>>,
    pool_term: Term,
) {
    if let Err(err) = pool_task_inner(cmd_recv, pool_term).await {
        tracing::error!(?err);
    } else {
        tracing::debug!("pool task ended");
    }
}

async fn pool_task_inner<T: Tx3Transport>(
    mut cmd_recv: tokio::sync::mpsc::UnboundedReceiver<PoolTaskCmd<T>>,
    pool_term: Term,
) -> Result<()> {
    loop {
        if pool_term.is_term() {
            break;
        }
        let _cmd = tokio::select! {
            _ = pool_term.on_term() => break,
            cmd = cmd_recv.recv() => match cmd {
                None => break,
                Some(cmd) => cmd,
            },
        };
    }
    Ok(())
}
