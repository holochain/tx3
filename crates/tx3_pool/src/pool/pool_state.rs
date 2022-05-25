use super::*;
use crate::pool::con_state::*;
use std::collections::HashMap;

#[allow(dead_code)]
pub(crate) struct OutboundMsg {
    pub _permit: tokio::sync::OwnedSemaphorePermit,
    pub peer_id: Arc<Tx3Id>,
    pub content: BytesList,
    pub timeout_at: tokio::time::Instant,
    pub resolve: tokio::sync::oneshot::Sender<Result<()>>,
}

pub(crate) enum PoolStateCmd {
    CleanupCheck,
    OutboundMsg(OutboundMsg),
    InboundAccept(tokio::sync::OwnedSemaphorePermit, tx3::Tx3Connection),
}

pub(crate) async fn pool_state_task<I: Tx3PoolImp>(
    pool_term: Term,
    bindings: Bindings<I>,
    imp: Arc<I>,
    inbound_send: tokio::sync::mpsc::UnboundedSender<InboundMsg>,
    cmd_send: tokio::sync::mpsc::UnboundedSender<PoolStateCmd>,
    cmd_recv: tokio::sync::mpsc::UnboundedReceiver<PoolStateCmd>,
) {
    let on_term = pool_term.on_term();
    tokio::task::spawn(async move {
        tokio::select! {
            _ = on_term => (),
            _ = async move {
                let dur = std::time::Duration::from_secs(5);
                let mut interval = tokio::time::interval_at(
                    tokio::time::Instant::now() + dur,
                    dur,
                );
                interval.set_missed_tick_behavior(
                    tokio::time::MissedTickBehavior::Delay,
                );
                loop {
                    interval.tick().await;
                    if cmd_send.send(PoolStateCmd::CleanupCheck).is_err() {
                        break;
                    }
                }
            } => (),
        }
    });

    tokio::select! {
        _ = pool_term.on_term() => (),
        _ = async move {
            if let Err(err) = pool_state_task_inner(bindings, imp, inbound_send, cmd_recv).await {
                tracing::error!(?err);
            }
        } => (),
    }
}

async fn pool_state_task_inner<I: Tx3PoolImp>(
    bindings: Bindings<I>,
    imp: Arc<I>,
    _inbound_send: tokio::sync::mpsc::UnboundedSender<InboundMsg>,
    mut cmd_recv: tokio::sync::mpsc::UnboundedReceiver<PoolStateCmd>,
) -> Result<()> {
    let mut con_state_map: HashMap<Arc<Tx3Id>, ConState<I>> = HashMap::new();

    while let Some(cmd) = cmd_recv.recv().await {
        match cmd {
            PoolStateCmd::CleanupCheck => {
                con_state_map.retain(|_, con_state| con_state.is_active());
            }
            PoolStateCmd::OutboundMsg(msg) => {
                let peer_id = msg.peer_id.clone();
                con_state_map
                    .entry(peer_id.clone())
                    .or_insert_with(|| ConState::new(bindings.clone(), imp.clone(), peer_id))
                    .push_outgoing_msg(msg);
            }
            PoolStateCmd::InboundAccept(con_permit, con) => {
                let peer_id = con.remote_id().clone();
                con_state_map
                    .entry(peer_id.clone())
                    .or_insert_with(|| ConState::new(bindings.clone(), imp.clone(), peer_id))
                    .maybe_use_con(con_permit, con);
            }
        }
    }

    Ok(())
}
