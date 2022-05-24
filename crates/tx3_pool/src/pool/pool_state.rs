use super::*;

#[allow(dead_code)]
pub(crate) struct OutboundMsg {
    pub _permit: tokio::sync::OwnedSemaphorePermit,
    pub peer_id: Arc<Tx3Id>,
    pub content: BytesList,
    pub timeout_at: tokio::time::Instant,
    pub resolve: tokio::sync::oneshot::Sender<Result<()>>,
}

pub(crate) enum PoolStateCmd {
    OutboundMsg(OutboundMsg),
    InboundAccept(tokio::sync::OwnedSemaphorePermit, tx3::Tx3Connection),
}

pub(crate) async fn pool_state_task<I: Tx3PoolImp>(
    imp: Arc<I>,
    inbound_send: tokio::sync::mpsc::UnboundedSender<InboundMsg>,
    cmd_recv: tokio::sync::mpsc::UnboundedReceiver<PoolStateCmd>,
) {
    if let Err(err) = pool_state_task_inner(imp, inbound_send, cmd_recv).await {
        tracing::error!(?err);
    }
}

async fn pool_state_task_inner<I: Tx3PoolImp>(
    _imp: Arc<I>,
    _inbound_send: tokio::sync::mpsc::UnboundedSender<InboundMsg>,
    _cmd_recv: tokio::sync::mpsc::UnboundedReceiver<PoolStateCmd>,
) -> Result<()> {
    Ok(())
}
