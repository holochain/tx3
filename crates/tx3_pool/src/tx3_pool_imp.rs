use crate::types::*;
use std::io::Result;
use std::sync::Arc;
use std::future::Future;
use std::pin::Pin;
use std::collections::HashMap;
use parking_lot::Mutex;

/// Tx3 pool inbound message receiver.
pub struct Tx3Recv {
    // safe to be unbounded, because pool logic keeps send permits
    // in the inbound msg struct... we only drop these permits
    // when the user of this recv instance actually pulls a message.
    inbound_recv: tokio::sync::mpsc::UnboundedReceiver<InboundMsg<Tx3PoolId>>,
}

impl Tx3Recv {
    /// Get the next inbound message received by this pool instance.
    pub async fn recv(&mut self) -> Option<(Arc<tx3::tls::TlsCertDigest>, BytesList)> {
        self.inbound_recv.recv().await.map(|msg| {
            let (peer_id, content) = msg.extract();
            let peer_id = Arc::new(peer_id.0);
            (peer_id, content)
        })
    }
}

/// Tx3 connection pool.
pub struct Tx3Pool {
    pool: Arc<crate::pool::Pool<Tx3PoolImp>>,
}

impl Tx3Pool {
    /// Construct / bind a new tx3 endpoint managed as a connection pool.
    pub async fn new(
        pool_config: PoolConfig,
        tx3_config: tx3::Tx3Config,
    ) -> Result<(Self, Tx3Recv)> {
        let (node, _inbound) = tx3::Tx3Node::new(tx3_config).await?;
        let node = Arc::new(node);
        let (inbound_send, inbound_recv) = tokio::sync::mpsc::unbounded_channel();
        let pool_imp = Arc::new(Tx3PoolImp::new(node.clone(), inbound_send));
        let pool = Arc::new(crate::pool::Pool::new(pool_config, pool_imp));
        let this = Self {
            pool,
        };
        let recv = Tx3Recv {
            inbound_recv,
        };
        Ok((this, recv))
    }

    /// Enqueue an outgoing message for send to a remote peer.
    pub fn send<A: Into<tx3::Tx3Url>, B: Into<BytesList>>(
        &self,
        url: A,
        content: B,
    ) -> impl Future<Output = Result<()>> + 'static + Send {
        let url = url.into();
        let content = content.into();
        let pool = self.pool.clone();
        async move {
            let peer_id = pool.as_imp().register_id(url)?;
            pool.send(peer_id, content).await
        }
    }

    /// Immediately terminate all connections, stopping all processing, both
    /// incoming and outgoing. This is NOT a graceful shutdown, and probably
    /// should only be used in testing scenarios.
    pub fn terminate(&self) {
        todo!()
    }
}

// -- private -- //

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct Tx3PoolId(pub tx3::tls::TlsCertDigest);

impl Id for Tx3PoolId {}

#[derive(Clone)]
struct Tx3PoolImp {
    inner: Arc<Mutex<Tx3PoolImpInner>>,
    inbound_send: tokio::sync::mpsc::UnboundedSender<InboundMsg<Tx3PoolId>>,
    pool_term: Term,
}

impl Tx3PoolImp {
    pub fn new(
        node: Arc<tx3::Tx3Node>,
        inbound_send: tokio::sync::mpsc::UnboundedSender<InboundMsg<Tx3PoolId>>,
    ) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Tx3PoolImpInner::new(node))),
            inbound_send,
            pool_term: Term::new(None),
        }
    }

    pub fn register_id(&self, url: tx3::Tx3Url) -> Result<Arc<Tx3PoolId>> {
        let id = match url.tls_cert_digest() {
            None => return Err(other_err("InvalidUrl")),
            Some(digest) => Arc::new(Tx3PoolId(digest)),
        };
        {
            let id = id.clone();
            Tx3PoolImpInner::access(&self.inner, move |inner| {
                inner.register_id(id, url);
            });
        }
        Ok(id)
    }
}

impl PoolImp for Tx3PoolImp {
    type Id = Tx3PoolId;

    type Connection = tx3::Tx3Connection;

    type AcceptFut = Pin<Box<dyn Future<Output = Result<(
        Self::Id,
        Self::Connection,
    )>> + 'static + Send>>;

    type ConnectFut = Pin<Box<dyn Future<Output = Result<Self::Connection>> + 'static + Send>>;

    fn connect(&self, id: Arc<Self::Id>) -> Self::ConnectFut {
        let inner = self.inner.clone();
        Box::pin(async move {
            let (node, url) = Tx3PoolImpInner::access(&inner, |inner| {
                inner.prep_connect(&id)
            })?;
            node.connect(url).await
        })
    }

    fn incoming(&self, msg: InboundMsg<Self::Id>) {
        if self.inbound_send.send(msg).is_err() {
            self.pool_term.term();
        }
    }
}

struct Tx3PoolImpInner {
    node: Arc<tx3::Tx3Node>,
    id_registry: HashMap<Arc<Tx3PoolId>, tx3::Tx3Url>,
}

impl Tx3PoolImpInner {
    pub fn new(node: Arc<tx3::Tx3Node>) -> Self {
        Self {
            node,
            id_registry: HashMap::new(),
        }
    }

    pub fn access<R, F>(this: &Mutex<Self>, f: F) -> R
    where
        F: FnOnce(&mut Self) -> R,
    {
        f(&mut *this.lock())
    }

    pub fn register_id(&mut self, id: Arc<Tx3PoolId>, url: tx3::Tx3Url) {
        self.id_registry.insert(id, url);
    }

    pub fn prep_connect(&mut self, id: &Arc<Tx3PoolId>) -> Result<(
        Arc<tx3::Tx3Node>,
        tx3::Tx3Url,
    )> {
        if let Some(url) = self.id_registry.get(id) {
            Ok((self.node.clone(), url.clone()))
        } else {
            Err(other_err("InvalidId"))
        }
    }
}
