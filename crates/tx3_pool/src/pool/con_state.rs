use super::*;
use parking_lot::Mutex;
use std::collections::VecDeque;

#[derive(Clone)]
pub(crate) struct ConState<I: Tx3PoolImp> {
    inner: Arc<Mutex<ConStateInner<I>>>,
}

impl<I: Tx3PoolImp> ConState<I> {
    pub fn new(
        bindings: Bindings<I>,
        imp: Arc<I>,
        peer_id: Arc<Tx3Id>,
    ) -> Self {
        Self {
            inner: Arc::new(Mutex::new(ConStateInner::new(bindings, imp, peer_id))),
        }
    }

    /// If this returns `false` it can be safely dropped.
    pub fn is_active(&self) -> bool {
        ConStateInner::access(&self.inner, |inner| inner.is_active())
    }

    /// Push a new outgoing message into this state instance.
    pub fn push_outgoing_msg(&self, msg: OutboundMsg) {
        ConStateInner::access(&self.inner, move |inner| inner.push_outgoing_msg(
            self.inner.clone(),
            msg,
        ));
    }

    /// Either accept an incoming connection / start processing messages,
    /// or drop it, because we already have an existing connection to this
    /// peer.
    pub fn maybe_use_con(
        &self,
        _con_permit: tokio::sync::OwnedSemaphorePermit,
        _con: tx3::Tx3Connection,
    ) {
        todo!()
    }
}

struct ConStateInner<I: Tx3PoolImp> {
    bindings: Bindings<I>,
    imp: Arc<I>,
    peer_id: Arc<Tx3Id>,
    msg_list: VecDeque<OutboundMsg>,
    pending_out_con: bool,
    con: Option<()>,
}

impl<I: Tx3PoolImp> ConStateInner<I> {
    pub fn new(bindings: Bindings<I>, imp: Arc<I>, peer_id: Arc<Tx3Id>) -> Self {
        Self {
            bindings,
            imp,
            peer_id,
            msg_list: VecDeque::new(),
            pending_out_con: false,
            con: None,
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
        if self.con.is_none() && !self.pending_out_con {
            self.pending_out_con = true;
            tokio::task::spawn(open_out_con(self.bindings.clone(), self.imp.clone(), this, self.peer_id.clone()));
        }
    }

    pub fn is_active(&mut self) -> bool {
        self.clear_timeouts();
        !self.msg_list.is_empty() || self.pending_out_con || self.con.is_some()
    }

    pub fn push_outgoing_msg(
        &mut self,
        this: Arc<Mutex<ConStateInner<I>>>,
        msg: OutboundMsg,
    ) {
        self.msg_list.push_back(msg);
        self.check_out_con(this);
    }
}

async fn open_out_con<I: Tx3PoolImp>(
    bindings: Bindings<I>,
    imp: Arc<I>,
    this: Arc<Mutex<ConStateInner<I>>>,
    peer_id: Arc<Tx3Id>,
) {
    if let Err(err) = open_out_con_inner(bindings, imp, this, peer_id).await {
        tracing::debug!(?err);
        // TODO - set pending_out_con false
    }
}

async fn open_out_con_inner<I: Tx3PoolImp>(
    bindings: Bindings<I>,
    imp: Arc<I>,
    _this: Arc<Mutex<ConStateInner<I>>>,
    peer_id: Arc<Tx3Id>,
) -> Result<()> {
    // TODO - first acquire con_permit

    let addr = imp.get_addr_store().lookup_addr(&peer_id).await?;
    tracing::trace!(?addr, "addr for new out con");

    if !imp.get_pool_hooks().connect_pre(addr.clone()).await {
        return Err(other_err("connect_pre returned false"));
    }

    let node = bindings.get_out_node()?;

    let _con = node.connect(addr).await?;

    Ok(())
}
