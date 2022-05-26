use super::*;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::atomic;

pub(crate) struct Bindings<I: Tx3PoolImp> {
    config: Arc<Tx3PoolConfig>,
    pool_term: Term,
    imp: Arc<I>,
    tls: tx3::tls::TlsConfig,
    cmd_send: tokio::sync::mpsc::UnboundedSender<PoolStateCmd>,
    in_con_limit: Arc<tokio::sync::Semaphore>,
    next_binding_id: Arc<atomic::AtomicU64>,
    inner: Arc<Mutex<BindingsInner>>,
}

impl<I: Tx3PoolImp> Clone for Bindings<I> {
    fn clone(&self) -> Self {
        Self {
            config: self.config.clone(),
            pool_term: self.pool_term.clone(),
            imp: self.imp.clone(),
            tls: self.tls.clone(),
            cmd_send: self.cmd_send.clone(),
            in_con_limit: self.in_con_limit.clone(),
            next_binding_id: self.next_binding_id.clone(),
            inner: self.inner.clone(),
        }
    }
}

impl<I: Tx3PoolImp> Bindings<I> {
    pub async fn new(
        config: Arc<Tx3PoolConfig>,
        pool_term: Term,
        imp: Arc<I>,
        tls: tx3::tls::TlsConfig,
        cmd_send: tokio::sync::mpsc::UnboundedSender<PoolStateCmd>,
    ) -> Result<Self> {
        let in_con_limit = Arc::new(tokio::sync::Semaphore::new(
            config.max_in_con_count as usize,
        ));

        let id = tls.cert_digest().clone();

        let addr = Tx3Addr {
            id: Some(id.clone()),
            ..Default::default()
        };
        let addr = Arc::new(addr);

        // non-listening node used for outgoing connections
        let (out_node, _) =
            tx3::Tx3Node::new(tx3::Tx3Config::default().with_tls(tls.clone()))
                .await?;

        Ok(Self {
            config,
            pool_term,
            imp,
            tls,
            cmd_send,
            in_con_limit,
            next_binding_id: Arc::new(atomic::AtomicU64::new(1)),
            inner: Arc::new(Mutex::new(BindingsInner::new(id, addr, out_node))),
        })
    }

    pub fn get_out_node(&self) -> Result<tx3::Tx3Node> {
        BindingsInner::access(&self.inner, |inner| inner.get_out_node())
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
        let bind_term = Term::new(
            "BindingClosed",
            Some(Arc::new(move || {
                let new_addr = BindingsInner::access(&inner, |inner| {
                    inner.terminate(binding_id)
                });
                if let Some(new_addr) = new_addr {
                    imp.get_pool_hooks().addr_update(new_addr);
                }
            })),
        );

        {
            let bind_term_err = bind_term.clone();
            Term::spawn_err2(
                &self.pool_term,
                &bind_term,
                process_receiver(
                    self.config.clone(),
                    self.pool_term.clone(),
                    bind_term.clone(),
                    self.imp.clone(),
                    self.cmd_send.clone(),
                    self.in_con_limit.clone(),
                    recv,
                ),
                move |err| {
                    tracing::debug!(?err);
                    bind_term_err.term();
                },
            );
        }

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
        let mut new_addr = Tx3Addr {
            id: Some(self.id.clone()),
            ..Default::default()
        };
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

    pub fn get_out_node(&self) -> Result<tx3::Tx3Node> {
        match self.map.get(&0) {
            Some(node) => Ok(node.clone()),
            None => Err(other_err("NoOutNodeBound")),
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
    pool_term: Term,
    bind_term: Term,
    imp: Arc<I>,
    cmd_send: tokio::sync::mpsc::UnboundedSender<PoolStateCmd>,
    in_con_limit: Arc<tokio::sync::Semaphore>,
    mut recv: tx3::Tx3Inbound,
) -> Result<()> {
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

        Term::spawn2(
            &pool_term,
            &bind_term,
            process_receiver_accept(
                config.clone(),
                imp.clone(),
                permit,
                accept,
                addr,
                cmd_send.clone(),
            ),
        );
    }

    Ok(())
}

async fn process_receiver_accept<I: Tx3PoolImp>(
    config: Arc<Tx3PoolConfig>,
    imp: Arc<I>,
    permit: tokio::sync::OwnedSemaphorePermit,
    accept: tx3::Tx3InboundAccept,
    addr: SocketAddr,
    cmd_send: tokio::sync::mpsc::UnboundedSender<PoolStateCmd>,
) -> Result<()> {
    tokio::time::timeout(config.connect_timeout, async move {
        if !imp.get_pool_hooks().accept_addr(addr).await {
            return Ok(());
        }

        let con = accept.accept().await?;

        if !imp
            .get_pool_hooks()
            .accept_id(con.remote_id().clone())
            .await
        {
            return Ok(());
        }

        let _ =
            cmd_send.send(PoolStateCmd::InboundAccept(Box::new((permit, con))));

        Ok(())
    })
    .await
    .map_err(other_err)?
}
