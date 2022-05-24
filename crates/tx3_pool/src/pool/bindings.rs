use super::*;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::atomic;

pub(crate) struct Bindings<I: Tx3PoolImp> {
    config: Arc<Tx3PoolConfig>,
    imp: Arc<I>,
    tls: tx3::tls::TlsConfig,
    cmd_send: tokio::sync::mpsc::UnboundedSender<PoolStateCmd>,
    in_con_limit: Arc<tokio::sync::Semaphore>,
    next_binding_id: atomic::AtomicU64,
    inner: Arc<Mutex<BindingsInner>>,
}

impl<I: Tx3PoolImp> Bindings<I> {
    pub async fn new(
        config: Arc<Tx3PoolConfig>,
        imp: Arc<I>,
        tls: tx3::tls::TlsConfig,
        cmd_send: tokio::sync::mpsc::UnboundedSender<PoolStateCmd>,
    ) -> Result<Self> {
        let in_con_limit = Arc::new(tokio::sync::Semaphore::new(
            config.max_in_con_count as usize,
        ));

        let id = tls.cert_digest().clone();

        let mut addr = Tx3Addr::default();
        addr.id = Some(id.clone());
        let addr = Arc::new(addr);

        // non-listening node used for outgoing connections
        let (out_node, _) =
            tx3::Tx3Node::new(tx3::Tx3Config::default().with_tls(tls.clone()))
                .await?;

        Ok(Self {
            config,
            imp,
            tls,
            cmd_send,
            in_con_limit,
            next_binding_id: atomic::AtomicU64::new(1),
            inner: Arc::new(Mutex::new(BindingsInner::new(id, addr, out_node))),
        })
    }

    pub fn local_addr(&self) -> Arc<Tx3Addr> {
        BindingsInner::access(&self.inner, |inner| inner.local_addr())
    }

    pub async fn bind<A: tx3::IntoAddr>(&self, bind: A) -> Result<BindHnd> {
        let binding_id =
            self.next_binding_id.fetch_add(1, atomic::Ordering::Relaxed);

        let (node, recv) = tx3::Tx3Node::new(
            tx3::Tx3Config::default()
                .with_bind(bind)
                .with_tls(self.tls.clone()),
        )
        .await?;

        let new_addr = BindingsInner::access(&self.inner, move |inner| {
            inner.new_binding(binding_id, node)
        });

        if let Some(new_addr) = new_addr {
            self.imp.get_pool_hooks().addr_update(new_addr);
        }

        let inner = self.inner.clone();
        let imp = self.imp.clone();
        let bind_term = Term::new(Some(Arc::new(move || {
            let new_addr = BindingsInner::access(&inner, |inner| {
                inner.terminate(binding_id)
            });
            if let Some(new_addr) = new_addr {
                imp.get_pool_hooks().addr_update(new_addr);
            }
        })));

        tokio::task::spawn(process_receiver(
            self.config.clone(),
            bind_term.clone(),
            self.imp.clone(),
            self.cmd_send.clone(),
            self.in_con_limit.clone(),
            recv,
        ));

        Ok(BindHnd::priv_new(bind_term))
    }
}

pub(crate) type BindingId = u64;

pub(crate) struct BindingsInner {
    id: Arc<Tx3Id>,
    addr: Arc<Tx3Addr>,
    map: HashMap<BindingId, tx3::Tx3Node>,
}

impl BindingsInner {
    pub fn new(
        id: Arc<Tx3Id>,
        addr: Arc<Tx3Addr>,
        out_node: tx3::Tx3Node,
    ) -> Self {
        let mut map = HashMap::new();
        map.insert(0, out_node);

        Self { id, addr, map }
    }

    pub fn access<R, F>(this: &Mutex<Self>, f: F) -> R
    where
        F: FnOnce(&mut Self) -> R,
    {
        f(&mut *this.lock())
    }

    pub fn check_calc_addr(&mut self) -> Option<Arc<Tx3Addr>> {
        let mut new_addr = Tx3Addr::default();
        new_addr.id = Some(self.id.clone());
        let mut stack_set = std::collections::HashSet::new();
        for (_, binding) in self.map.iter() {
            for stack in binding.local_addr().stack_list.iter() {
                if stack_set.insert(stack) {
                    new_addr.stack_list.push(stack.clone());
                }
            }
        }
        let new_addr = Arc::new(new_addr);
        if self.addr != new_addr {
            self.addr = new_addr.clone();
            Some(new_addr)
        } else {
            None
        }
    }

    pub fn local_addr(&mut self) -> Arc<Tx3Addr> {
        self.addr.clone()
    }

    pub fn new_binding(
        &mut self,
        binding_id: BindingId,
        node: tx3::Tx3Node,
    ) -> Option<Arc<Tx3Addr>> {
        self.map.insert(binding_id, node);
        self.check_calc_addr()
    }

    pub fn terminate(&mut self, binding_id: BindingId) -> Option<Arc<Tx3Addr>> {
        if let Some(node) = self.map.remove(&binding_id) {
            drop(node)
        }
        self.check_calc_addr()
    }
}

async fn process_receiver<I: Tx3PoolImp>(
    config: Arc<Tx3PoolConfig>,
    bind_term: Term,
    imp: Arc<I>,
    cmd_send: tokio::sync::mpsc::UnboundedSender<PoolStateCmd>,
    in_con_limit: Arc<tokio::sync::Semaphore>,
    recv: tx3::Tx3Inbound,
) {
    if let Err(err) = process_receiver_inner(
        config,
        bind_term.clone(),
        imp,
        cmd_send,
        in_con_limit,
        recv,
    )
    .await
    {
        tracing::debug!(?err);
    }
    bind_term.term();
}

async fn process_receiver_inner<I: Tx3PoolImp>(
    config: Arc<Tx3PoolConfig>,
    bind_term: Term,
    imp: Arc<I>,
    cmd_send: tokio::sync::mpsc::UnboundedSender<PoolStateCmd>,
    in_con_limit: Arc<tokio::sync::Semaphore>,
    mut recv: tx3::Tx3Inbound,
) -> Result<()> {
    tokio::select! {
        _ = bind_term.on_term() => (),
        _ = async move {
            loop {
                let (accept, addr) = match recv.recv().await {
                    None => break,
                    Some(r) => r,
                };

                let permit = match in_con_limit.clone().try_acquire_owned() {
                    Ok(permit) => permit,
                    Err(_) => {
                        // no available permits, drop the connection
                        // this is preferable to backpressure because
                        // in DDoS mitigation mode, we don't want a bunch
                        // of waiting connections hanging around
                        continue;
                    }
                };

                tokio::task::spawn(process_receiver_accept(
                    config.clone(),
                    bind_term.clone(),
                    imp.clone(),
                    permit,
                    accept,
                    addr,
                    cmd_send.clone(),
                ));
            }
        } => (),
    }
    Ok(())
}

async fn process_receiver_accept<I: Tx3PoolImp>(
    config: Arc<Tx3PoolConfig>,
    bind_term: Term,
    imp: Arc<I>,
    permit: tokio::sync::OwnedSemaphorePermit,
    accept: tx3::Tx3InboundAccept,
    addr: SocketAddr,
    cmd_send: tokio::sync::mpsc::UnboundedSender<PoolStateCmd>,
) {
    if let Err(err) = process_receiver_accept_inner(
        config, bind_term, imp, permit, accept, addr, cmd_send,
    )
    .await
    {
        tracing::debug!(?err);
    }
}

async fn process_receiver_accept_inner<I: Tx3PoolImp>(
    config: Arc<Tx3PoolConfig>,
    bind_term: Term,
    imp: Arc<I>,
    permit: tokio::sync::OwnedSemaphorePermit,
    accept: tx3::Tx3InboundAccept,
    addr: SocketAddr,
    cmd_send: tokio::sync::mpsc::UnboundedSender<PoolStateCmd>,
) -> Result<()> {
    let fut = async move {
        if !imp.get_pool_hooks().accept_addr(addr).await {
            return Ok(());
        }

        let con = accept.accept().await?;

        if !imp.get_pool_hooks().accept_id(con.remote_id().clone()).await {
            return Ok(());
        }

        let _ = cmd_send
            .send(PoolStateCmd::InboundAccept(
                permit,
                con,
            ));

        Ok(())
    };

    tokio::time::timeout(config.connect_timeout, async move {
        tokio::select! {
            _ = bind_term.on_term() => Ok(()),
            r = fut => r,
        }
    })
    .await
    .map_err(other_err)?
}
