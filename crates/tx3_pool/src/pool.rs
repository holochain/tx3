#![allow(dead_code)]
use crate::pool_con::*;
use crate::types::*;
use crate::*;
use parking_lot::Mutex;
use std::collections::HashMap;

type Id<T> = <<T as Tx3Transport>::Common as Tx3TransportCommon>::EndpointId;

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
    in_recv: tokio::sync::mpsc::UnboundedReceiver<(Id<T>, Message)>,
}

impl<T: Tx3Transport> Tx3PoolIncoming<T> {
    /// Pull the next incoming message from the receive stream.
    pub async fn recv(&mut self) -> Option<(Id<T>, BytesList)> {
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

impl<T: Tx3Transport> ConMap<T> {
    pub fn new() -> Self {
        Self(Arc::new(Mutex::new(HashMap::new())))
    }

    pub async fn get_con(
        &self,
        _id: Arc<Id<T>>,
        _con_permit: SharedPermit,
    ) -> Result<ConInfo> {
        todo!()
    }
}

/// Tx3 pool.
pub struct Tx3Pool<T: Tx3Transport> {
    pool_term: Term,
    con_limit: SharedSemaphore<Id<T>>,
    con_map: ConMap<T>,
}

impl<T: Tx3Transport> Tx3Pool<T> {
    /// Bind a new transport backend, wrapping it in Tx3Pool logic.
    pub async fn bind(
        transport: T,
        path: Arc<T::BindPath>,
    ) -> Result<(T::BindAppData, Self, Tx3PoolIncoming<T>)> {
        let (app, _connector, _acceptor) = transport.bind(path).await?;
        let (_in_send, in_recv) = tokio::sync::mpsc::unbounded_channel();
        let incoming = Tx3PoolIncoming { in_recv };

        let pool_term = Term::new();
        // TODO _ FIXME - limit from config
        let con_limit = SharedSemaphore::new(64);
        let con_map = ConMap::new();

        let this = Tx3Pool {
            pool_term,
            con_limit,
            con_map,
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
                .get_permit(dst.clone(), 1)
                .await
                .map_err(|_| other_err("PoolClosed"))?;

            let con = self.con_map.get_con(dst, con_permit).await?;

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
        .map_err(|err| other_err(err))?
    }

    /// Immediately terminate all connections, stopping all processing, both
    /// incoming and outgoing. This is NOT a graceful shutdown, and probably
    /// should only be used in testing scenarios.
    pub fn terminate(&self) {
        self.con_limit.close();
        self.pool_term.term();
    }
}
