//! Support types for Tx3Pool implementations.

use bytes::Buf;
use std::fmt::Debug;
use std::future::Future;
use std::hash::Hash;
use std::io::Result;
use std::pin::Pin;
use std::sync::atomic;
use std::sync::Arc;
use std::task::Poll;
use tokio::io::AsyncRead;
use tokio::io::AsyncWrite;

pub(crate) const FI_LEN_MASK: u32 = 0b00000011111111111111111111111111;

/// Tx3 helper until `std::io::Error::other()` is stablized.
pub fn other_err<E: Into<Box<dyn std::error::Error + Send + Sync>>>(
    error: E,
) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::Other, error)
}

/// Marker trait for something that can act as a remote connection identifier.
pub trait Id:
    'static + Send + Sync + Debug + PartialEq + Eq + PartialOrd + Ord + Hash
{
}

/// Receive an incoming message from a remote node.
/// Gives mutable access to the internal BytesList, for parsing.
/// The permit limiting the max read bytes in memory is bound to this struct.
/// Drop this to allow that permit to be freed, allowing additional
/// data to be read by the input readers.
pub struct InboundMsg<K: Id> {
    _permit: tokio::sync::OwnedSemaphorePermit,
    peer_id: Arc<K>,
    content: BytesList,
}

impl<K: Id> InboundMsg<K> {
    /// Extract the contents of this message, dropping the permit,
    /// thereby allowing additional messages to be buffered.
    pub fn extract(self) -> (Arc<K>, BytesList) {
        (self.peer_id, self.content)
    }
}

/// When implementing a pool, the pool needs to be able to make certain
/// calls out to the implementor. This PoolImp trait provides those calls.
pub trait PoolImp: 'static + Send + Sync {
    /// A type which uniquely identifies an endpoint.
    type Id: Id;

    /// The backend system transport connection type.
    type Connection: 'static + Send + AsyncRead + AsyncWrite + Unpin;

    /// A future which resolves into an "accept" result.
    type AcceptFut: 'static
        + Send
        + Future<Output = Result<(Self::Id, Self::Connection)>>;

    /// A future returned by the "connect" function.
    type ConnectFut: 'static + Send + Future<Output = Result<Self::Connection>>;

    /// Establish a new outgoing connection to a remote peer.
    fn connect(&self, id: Arc<Self::Id>) -> Self::ConnectFut;

    /// Receive an incoming message from a remote node.
    fn incoming(&self, msg: InboundMsg<Self::Id>);
}

/// A set of distinct chunks of bytes that can be treated as a single unit
#[derive(Default)]
pub struct BytesList(pub std::collections::VecDeque<bytes::Bytes>);

impl BytesList {
    /// Construct a new BytesList.
    pub fn new() -> Self {
        Self::default()
    }

    /// Construct a new BytesList with given capacity.
    pub fn with_capacity(capacity: usize) -> Self {
        Self(std::collections::VecDeque::with_capacity(capacity))
    }

    /// Push a new bytes::Bytes into this BytesList.
    pub fn push(&mut self, data: bytes::Bytes) {
        if !data.has_remaining() {
            self.0.push_back(data);
        }
    }

    /// Extract the contents of this BytesList into a new one
    pub fn extract(&mut self) -> Self {
        Self(std::mem::take(&mut self.0))
    }

    /// Remove specified byte cnt from the front of this list as a new list.
    /// Panics if self doesn't contain enough bytes.
    #[allow(clippy::comparison_chain)] // clearer written explicitly
    pub fn take_front(&mut self, mut cnt: usize) -> Self {
        let mut out = BytesList::new();
        loop {
            let mut item = match self.0.pop_front() {
                Some(item) => item,
                None => panic!("UnexpectedEof"),
            };

            let rem = item.remaining();
            if rem == cnt {
                out.push(item);
                return out;
            } else if rem < cnt {
                out.push(item);
                cnt -= rem;
            } else if rem > cnt {
                out.push(item.split_to(cnt));
                self.0.push_front(item);
                return out;
            }
        }
    }
}

impl From<std::collections::VecDeque<bytes::Bytes>> for BytesList {
    #[inline(always)]
    fn from(v: std::collections::VecDeque<bytes::Bytes>) -> Self {
        Self(v)
    }
}

impl From<bytes::Bytes> for BytesList {
    #[inline(always)]
    fn from(b: bytes::Bytes) -> Self {
        let mut out = std::collections::VecDeque::with_capacity(8);
        out.push_back(b);
        out.into()
    }
}

impl From<Vec<u8>> for BytesList {
    #[inline(always)]
    fn from(v: Vec<u8>) -> Self {
        bytes::Bytes::from(v).into()
    }
}

impl From<&[u8]> for BytesList {
    #[inline(always)]
    fn from(b: &[u8]) -> Self {
        bytes::Bytes::copy_from_slice(b).into()
    }
}

impl<const N: usize> From<&[u8; N]> for BytesList {
    #[inline(always)]
    fn from(b: &[u8; N]) -> Self {
        bytes::Bytes::copy_from_slice(&b[..]).into()
    }
}

impl bytes::Buf for BytesList {
    fn remaining(&self) -> usize {
        self.0.iter().map(|b| b.remaining()).sum()
    }

    fn chunk(&self) -> &[u8] {
        match self.0.get(0) {
            Some(b) => b.chunk(),
            None => &[],
        }
    }

    #[allow(clippy::comparison_chain)] // clearer written explicitly
    fn advance(&mut self, mut cnt: usize) {
        loop {
            let mut item = match self.0.pop_front() {
                Some(item) => item,
                None => return,
            };

            let rem = item.remaining();
            if rem == cnt {
                return;
            } else if rem < cnt {
                cnt -= rem;
            } else if rem > cnt {
                item.advance(cnt);
                self.0.push_front(item);
                return;
            }
        }
    }
}

/// Terminal notification helper utility.
#[derive(Clone)]
pub struct Term {
    term: Arc<atomic::AtomicBool>,
    sig: Arc<tokio::sync::Notify>,
    trgr: Arc<dyn Fn() + 'static + Send + Sync>,
}

impl Term {
    /// Construct a new term instance with optional term callback.
    pub fn new(trgr: Option<Arc<dyn Fn() + 'static + Send + Sync>>) -> Self {
        let trgr = trgr.unwrap_or_else(|| Arc::new(|| {}));
        Self {
            term: Arc::new(atomic::AtomicBool::new(false)),
            sig: Arc::new(tokio::sync::Notify::new()),
            trgr,
        }
    }

    /// Trigger termination.
    pub fn term(&self) {
        self.term.store(true, atomic::Ordering::Release);
        self.sig.notify_waiters();
        (self.trgr)();
    }

    /// Returns `true` if termination has been triggered.
    pub fn is_term(&self) -> bool {
        self.term.load(atomic::Ordering::Acquire)
    }

    /// Returns a future that will resolve when termination is triggered.
    pub fn on_term(
        &self,
    ) -> impl std::future::Future<Output = ()> + 'static + Send + Unpin {
        let sig = self.sig.clone();
        let mut fut = Box::pin(async move { sig.notified().await });
        let term = self.term.clone();
        futures::future::poll_fn(move |cx| {
            if term.load(atomic::Ordering::Acquire) {
                return Poll::Ready(());
            }
            match Pin::new(&mut fut).poll(cx) {
                Poll::Pending => (),
                Poll::Ready(_) => return Poll::Ready(()),
            }
            // even though we create the "notified" future first, there's still
            // a small chance we could have missed the notification, between
            // the previous flag load and when we called poll... so check again
            if term.load(atomic::Ordering::Acquire) {
                return Poll::Ready(());
            }
            // now, our cx is registered for wake, and we've double checked
            // the flag, safe to return pending
            Poll::Pending
        })
    }
}

/// Configuration for a [crate::pool::Pool] instance.
#[non_exhaustive]
pub struct PoolConfig {
    /// Maximum byte length of a single message.
    /// (Currently cannot be > u32::MAX >> 6 until multiplexing is implemented).
    /// Default: u32::MAX >> 6.
    pub max_msg_byte_count: u32,

    /// How many conncurrent outgoing connections are allowed.
    /// Default: 64.
    pub max_out_con_count: u32,

    /// How many concurrent incoming connections are allowed.
    /// Default: 64.
    pub max_in_con_count: u32,

    /// How many bytes can build up in our outgoing buffer before
    /// we start experiencing backpressure.
    /// (Cannot be > usize::MAX >> 3 do to tokio semaphore limitations)
    /// Default: u32::MAX >> 6.
    pub max_out_byte_count: u64,

    /// How many bytes can build up in our incoming buffer before
    /// we stop reading to trigger backpressure.
    /// (Cannot be > usize::MAX >> 3 do to tokio semaphore limitations)
    /// Default: u32::MAX >> 6.
    pub max_in_byte_count: u64,

    /// Connect timeout after which to abandon establishing new connections,
    /// or handshaking incoming connections.
    /// Default: 20 seconds.
    pub connect_timeout: std::time::Duration,

    /// Idle timeout after which to close the connection.
    /// Default: 20 seconds.
    pub idle_timeout: std::time::Duration,

    /// Time an outgoing message is allowed to remain queued but un-sent.
    /// This timer stops counting as soon as it is claimed by a send worker.
    /// Default: 20 seconds.
    pub msg_send_timeout: std::time::Duration,

    /// NOT PUB -- both sides should match -- someday negotiate? --
    /// The max message read/write per connection before closing.
    /// Default: 64.
    #[allow(dead_code)]
    max_read_write_msg_count_per_con: u64,

    /// NOT PUB -- both sides should match -- someday negotiate? --
    /// The max byte count that can be read/written per con before closing.
    /// Default: u32::MAX >> 6.
    #[allow(dead_code)]
    max_read_write_byte_count_per_con: u64,
}

impl Default for PoolConfig {
    fn default() -> Self {
        Self {
            max_msg_byte_count: FI_LEN_MASK,
            max_out_con_count: 64,
            max_in_con_count: 64,
            max_out_byte_count: FI_LEN_MASK as u64,
            max_in_byte_count: FI_LEN_MASK as u64,
            connect_timeout: std::time::Duration::from_secs(20),
            idle_timeout: std::time::Duration::from_secs(20),
            msg_send_timeout: std::time::Duration::from_secs(20),
            max_read_write_msg_count_per_con: 64,
            max_read_write_byte_count_per_con: FI_LEN_MASK as u64,
        }
    }
}

/*
/// Common types.
pub trait Tx3TransportCommon: 'static + Send + Sync {
    /// A type which uniquely identifies an endpoint.
    type EndpointId: 'static
        + Send
        + Sync
        + Debug
        + PartialEq
        + Eq
        + PartialOrd
        + Ord
        + Hash;

    /// The backend system transport connection type.
    type Connection: 'static + Send + AsyncRead + AsyncWrite + Unpin;
}

/// Connector type.
pub trait Tx3TransportConnector<C: Tx3TransportCommon>:
    'static + Send + Sync
{
    /// A future returned by the "connect" function.
    type ConnectFut: 'static + Send + Future<Output = Result<C::Connection>>;

    /// Establish a new outgoing connection to a remote peer.
    fn connect(&self, id: Arc<C::EndpointId>) -> Self::ConnectFut;
}

/// Acceptor type.
pub trait Tx3TransportAcceptor<C: Tx3TransportCommon>:
    'static + Send + Stream<Item = Self::AcceptFut> + Unpin
{
    /// A future that resolves into an incoming connection, or errors.
    type AcceptFut: 'static
        + Send
        + Future<Output = Result<(Arc<C::EndpointId>, C::Connection)>>;
}

/// Implement this trait on a newtype to supply the backend system transport
/// types and hooks to run a Tx3 pool.
pub trait Tx3Transport: 'static + Send {
    /// Common types.
    type Common: Tx3TransportCommon;

    /// Connector type.
    type Connector: Tx3TransportConnector<Self::Common>;

    /// Acceptor type.
    type Acceptor: Tx3TransportAcceptor<Self::Common>;

    /// A type which explains how to bind a local endpoint.
    /// Could be a url or a set of urls.
    type BindPath: 'static + Send + Sync;

    /// Additional app data returned by the "bind" function
    type BindAppData: 'static + Send;

    /// A future returned by the "bind" function.
    type BindFut: 'static
        + Send
        + Future<
            Output = Result<(
                Self::BindAppData,
                Self::Connector,
                Self::Acceptor,
            )>,
        >;

    /// Bind a new local endpoint of this type.
    fn bind(self, path: Arc<Self::BindPath>) -> Self::BindFut;
}

pub(crate) type Id<T> =
    <<T as Tx3Transport>::Common as Tx3TransportCommon>::EndpointId;


pub(crate) struct Message {
    pub content: BytesList,
    pub _permit: tokio::sync::OwnedSemaphorePermit,
}

impl Message {
    pub fn new(
        content: BytesList,
        permit: tokio::sync::OwnedSemaphorePermit,
    ) -> Self {
        Self {
            content,
            _permit: permit,
        }
    }
}

struct SharedPermitInner {
    _permit: tokio::sync::OwnedSemaphorePermit,
}

#[derive(Clone)]
pub(crate) struct SharedPermit(Arc<SharedPermitInner>);

impl SharedPermit {
    fn priv_new(p: tokio::sync::OwnedSemaphorePermit) -> Self {
        Self(Arc::new(SharedPermitInner { _permit: p }))
    }
}

type SharedMap<K> = HashMap<
    Arc<K>,
    futures::future::Shared<
        futures::future::BoxFuture<
            'static,
            std::result::Result<SharedPermit, ()>,
        >,
    >,
>;

#[derive(Clone)]
pub(crate) struct SharedSemaphore<K>
where
    K: 'static + Send + Sync + Eq + std::hash::Hash,
{
    limit: Arc<tokio::sync::Semaphore>,
    map: Arc<Mutex<SharedMap<K>>>,
}

#[allow(dead_code)]
pub(crate) struct SharedPermitAcceptResolver<K>
where
    K: 'static + Send + Sync + Eq + std::hash::Hash,
{
    permit: SharedPermit,
    map: Arc<Mutex<SharedMap<K>>>,
}

impl<K> SharedPermitAcceptResolver<K>
where
    K: 'static + Send + Sync + Eq + std::hash::Hash,
{
    #[allow(dead_code)]
    pub fn resolve(
        self,
        _key: Arc<K>,
    ) -> std::result::Result<SharedPermit, ()> {
        /*
        use std::collections::hash_map::Entry;
        match self.map.entry(key) {
            Entry::Occupied(e) => {
            }
            Entry::Vacant(e) => {
            }
        }
        */
        todo!()
    }
}

impl<K> SharedSemaphore<K>
where
    K: 'static + Send + Sync + Eq + std::hash::Hash,
{
    pub fn new(count: u32) -> Self {
        let limit = Arc::new(tokio::sync::Semaphore::new(count as usize));
        let map = Arc::new(Mutex::new(HashMap::new()));
        Self { limit, map }
    }

    fn access_map<R, F>(&self, f: F) -> R
    where
        F: FnOnce(&mut SharedMap<K>) -> R,
    {
        let mut inner = self.map.lock();
        f(&mut *inner)
    }

    pub fn close(&self) {
        self.limit.close();
        self.access_map(|map| {
            map.clear();
        });
    }

    /*
    pub fn is_closed(&self) -> bool {
        self.limit.is_closed()
    }
    */

    pub fn remove(&self, key: &Arc<K>) {
        self.access_map(|map| {
            map.remove(key);
        })
    }

    pub fn get_accept_permit(&self) -> Option<SharedPermitAcceptResolver<K>> {
        match self.limit.clone().try_acquire_owned() {
            Err(_) => None,
            Ok(permit) => Some(SharedPermitAcceptResolver {
                permit: SharedPermit::priv_new(permit),
                map: self.map.clone(),
            }),
        }
    }

    pub async fn get_permit(
        &self,
        key: Arc<K>,
    ) -> std::result::Result<SharedPermit, ()> {
        let limit = self.limit.clone();
        self.access_map(move |map| {
            use futures::future::BoxFuture;
            use futures::future::FutureExt;
            use futures::future::TryFutureExt;
            use std::collections::hash_map::Entry;
            match map.entry(key) {
                Entry::Occupied(e) => Ok(e.get().clone()),
                Entry::Vacant(e) => {
                    if limit.is_closed() {
                        return Err(());
                    }
                    let fut: BoxFuture<
                        'static,
                        std::result::Result<SharedPermit, ()>,
                    > = async move {
                        limit
                            .acquire_owned()
                            .map_err(|_| ())
                            .map_ok(SharedPermit::priv_new)
                            .await
                    }
                    .boxed();
                    let fut = fut.shared();
                    e.insert(fut.clone());
                    Ok(fut)
                }
            }
        })?
        .await
    }
}
*/
