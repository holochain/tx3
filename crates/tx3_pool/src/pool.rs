//! Generic connection pool types.

use crate::types::*;
use std::future::Future;
use std::io::Result;
use std::sync::Arc;

/// Generic connection pool.
pub struct Pool<I: PoolImp> {
    imp: Arc<I>,
}

impl<I: PoolImp> Pool<I> {
    /// Construct a new generic connection pool.
    pub fn new(_config: PoolConfig, imp: Arc<I>) -> Self {
        Self { imp }
    }

    /// Access the given implementation
    pub fn as_imp(&self) -> &Arc<I> {
        &self.imp
    }

    /// Enqueue an outgoing message for send to a remote peer.
    pub fn send<B: Into<BytesList>>(
        &self,
        _peer_id: Arc<I::Id>,
        _content: B,
    ) -> impl Future<Output = Result<()>> + 'static + Send {
        async move { todo!() }
    }

    /// Try to accept an incoming connection. Note, if we've reached
    /// our incoming connection limit, this future will be dropped
    /// without being awaited (TooManyConnections).
    pub fn accept(&self, _accept_fut: I::AcceptFut) {
        todo!()
    }

    /// Immediately terminate all connections, stopping all processing, both
    /// incoming and outgoing. This is NOT a graceful shutdown, and probably
    /// should only be used in testing scenarios.
    pub fn terminate(&self) {
        todo!()
    }
}

/*
#![allow(dead_code)]
use crate::pool_con::*;
use crate::types::*;
use crate::*;
use parking_lot::Mutex;
use std::collections::HashMap;

const FI_LEN_MASK: u32 = 0b00000011111111111111111111111111;
//const FI_OTH_MASK: u32 = 0b11111100000000000000000000000000;

/// Tx3 pool configuration.
#[non_exhaustive]
pub struct Tx3PoolConfig {}

#[allow(clippy::derivable_impls)]
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
    in_recv: tokio::sync::mpsc::UnboundedReceiver<(Arc<Id<T>>, Message)>,
}

impl<T: Tx3Transport> Tx3PoolIncoming<T> {
    /// Pull the next incoming message from the receive stream.
    pub async fn recv(&mut self) -> Option<(Arc<Id<T>>, BytesList)> {
        let (id, Message { content, .. }) = self.in_recv.recv().await?;
        Some((id, content))
    }
}

#[derive(Clone)]
struct ConInfo {
    _con_permit: SharedPermit,
    pub pool_con: PoolCon,
}

type ConMapInner<T> = HashMap<
    Arc<Id<T>>,
    futures::future::Shared<
        futures::future::BoxFuture<
            'static,
            std::result::Result<ConInfo, Arc<std::io::Error>>,
        >,
    >,
>;

struct ConMap<T: Tx3Transport>(Arc<Mutex<ConMapInner<T>>>);

impl<T: Tx3Transport> Clone for ConMap<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<T: Tx3Transport> ConMap<T> {
    pub fn new() -> Self {
        Self(Arc::new(Mutex::new(HashMap::new())))
    }

    fn access<R, F>(&self, f: F) -> R
    where
        F: FnOnce(&mut ConMapInner<T>) -> R,
    {
        let mut map = self.0.lock();
        f(&mut *map)
    }

    pub fn close(&self) {
        self.access(|map| {
            map.clear();
        })
    }

    pub async fn get_con(
        &self,
        id: Arc<Id<T>>,
        con_permit: SharedPermit,
        connector: Arc<<T as Tx3Transport>::Connector>,
        pool_term: Term,
        con_limit: Arc<SharedSemaphore<Id<T>>>,
        in_send: tokio::sync::mpsc::UnboundedSender<(Arc<Id<T>>, Message)>,
    ) -> Result<ConInfo> {
        let this = self.clone();
        self.access(move |map| {
            use futures::future::BoxFuture;
            use futures::future::FutureExt;
            use std::collections::hash_map::Entry;
            match map.entry(id.clone()) {
                Entry::Occupied(e) => Result::Ok(e.get().clone()),
                Entry::Vacant(e) => {
                    let fut: BoxFuture<
                        'static,
                        std::result::Result<ConInfo, Arc<std::io::Error>>,
                    > = async move {
                        let con = connector
                            .connect(id.clone())
                            .await
                            .map_err(Arc::new)?;

                        let con_term = {
                            let id = id.clone();
                            Term::new(Arc::new(move || {
                                this.access(|map| {
                                    map.remove(&id);
                                });
                                con_limit.remove(&id);
                            }))
                        };

                        let pool_con = PoolCon::new::<T>(
                            id,
                            pool_term,
                            con_term,
                            con,
                            in_send,
                            FI_LEN_MASK as usize,
                            FI_LEN_MASK as usize,
                        );

                        let con_info = ConInfo {
                            _con_permit: con_permit,
                            pool_con,
                        };

                        Ok(con_info)
                    }
                    .boxed();
                    let fut = fut.shared();
                    e.insert(fut.clone());
                    Result::Ok(fut)
                }
            }
        })?
        .await
        .map_err(other_err)
    }
}

/// Tx3 pool.
pub struct Tx3Pool<T: Tx3Transport> {
    connector: Arc<<T as Tx3Transport>::Connector>,
    pool_term: Term,
    con_limit: Arc<SharedSemaphore<Id<T>>>,
    con_map: ConMap<T>,
    in_send: tokio::sync::mpsc::UnboundedSender<(Arc<Id<T>>, Message)>,
}

impl<T: Tx3Transport> Tx3Pool<T> {
    /// Bind a new transport backend, wrapping it in Tx3Pool logic.
    pub async fn bind(
        transport: T,
        path: Arc<T::BindPath>,
    ) -> Result<(T::BindAppData, Self, Tx3PoolIncoming<T>)> {
        let (app, connector, acceptor) = transport.bind(path).await?;
        let (in_send, in_recv) = tokio::sync::mpsc::unbounded_channel();
        let incoming = Tx3PoolIncoming { in_recv };

        let pool_term = Term::new(Arc::new(|| {}));
        // TODO _ FIXME - limit from config
        let con_limit = Arc::new(SharedSemaphore::new(64));
        let con_map = ConMap::new();

        accept::<T>(
            acceptor,
            pool_term.clone(),
            con_limit.clone(),
            con_map.clone(),
        );

        let this = Tx3Pool {
            connector: Arc::new(connector),
            pool_term,
            con_limit,
            con_map,
            in_send,
        };

        Ok((app, this, incoming))
    }

    /// Attempt to send a framed message to a target node.
    ///
    /// This can experience backpressure in two ways:
    /// - There is no active connection and we are at our max connection count,
    ///   and are unable to free any active connections.
    /// - The backpressure of sending to the outgoing transport connection.
    ///
    /// This call can timeout per the timeout specified in Tx3PoolConfig.
    ///
    /// This future will resolve Ok() when all data has been offloaded to
    /// the underlying transport
    pub async fn send<B: Into<BytesList>>(
        &self,
        dst: Arc<Id<T>>,
        data: B,
    ) -> Result<()> {
        let data = data.into();

        // TODO _ FIXME - use timeout from config
        tokio::time::timeout(std::time::Duration::from_secs(20), async move {
            let con_permit = self
                .con_limit
                .get_permit(dst.clone())
                .await
                .map_err(|_| other_err("PoolClosed"))?;

            let con = self
                .con_map
                .get_con(
                    dst,
                    con_permit,
                    self.connector.clone(),
                    self.pool_term.clone(),
                    self.con_limit.clone(),
                    self.in_send.clone(),
                )
                .await?;

            let bytes_permit = con
                .pool_con
                .acquire_send_permit(data.remaining() as u32)
                .await?;

            let msg = Message {
                content: data,
                _permit: bytes_permit,
            };

            con.pool_con.send(msg).map_err(|_| other_err("ConClosed"))?;

            Ok(())
        })
        .await
        .map_err(other_err)?
    }

    /// Immediately terminate all connections, stopping all processing, both
    /// incoming and outgoing. This is NOT a graceful shutdown, and probably
    /// should only be used in testing scenarios.
    pub fn terminate(&self) {
        self.pool_term.term();
        self.con_limit.close();
        self.con_map.close();
    }
}

fn accept<T: Tx3Transport>(
    acceptor: <T as Tx3Transport>::Acceptor,
    pool_term: Term,
    con_limit: Arc<SharedSemaphore<Id<T>>>,
    con_map: ConMap<T>,
) {
    tokio::task::spawn(async move {
        if let Err(err) =
            accept_inner(acceptor, pool_term, con_limit, con_map).await
        {
            tracing::warn!(?err);
        }
    });
}

async fn accept_inner<T: Tx3Transport>(
    mut acceptor: <T as Tx3Transport>::Acceptor,
    pool_term: Term,
    con_limit: Arc<SharedSemaphore<Id<T>>>,
    _con_map: ConMap<T>,
) -> Result<()> {
    use futures::stream::StreamExt;

    loop {
        tokio::select! {
            _ = pool_term.on_term() => break,
            a = acceptor.next() => match a {
                None => break,
                Some(a) => accept2::<T>(a, pool_term.clone(), con_limit.clone()),
            },
        }
    }

    Ok(())
}

fn accept2<T: Tx3Transport>(
    acceptor: <<T as Tx3Transport>::Acceptor as Tx3TransportAcceptor<
        <T as Tx3Transport>::Common,
    >>::AcceptFut,
    pool_term: Term,
    con_limit: Arc<SharedSemaphore<Id<T>>>,
) {
    tokio::task::spawn(async move {
        if let Err(err) =
            accept2_inner::<T>(acceptor, pool_term, con_limit).await
        {
            tracing::debug!(?err);
        }
    });
}

async fn accept2_inner<T: Tx3Transport>(
    acceptor: <<T as Tx3Transport>::Acceptor as Tx3TransportAcceptor<
        <T as Tx3Transport>::Common,
    >>::AcceptFut,
    pool_term: Term,
    con_limit: Arc<SharedSemaphore<Id<T>>>,
) -> Result<()> {
    let _accept_permit = match con_limit.get_accept_permit() {
        None => return Err(other_err("DropIncomingNoPermits")),
        Some(permit) => permit,
    };

    let (_id, _con) = tokio::select! {
        _ = pool_term.on_term() => return Ok(()),
        a = acceptor => a,
    }?;

    Ok(())
}
*/
