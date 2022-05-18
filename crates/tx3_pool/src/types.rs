//! Types you'll need if you're implementing transport newtypes for a pool.

use crate::*;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::pin::Pin;
use std::sync::atomic;
use std::task::Poll;

/// Tx3 helper until `std::io::Error::other()` is stablized.
pub fn other_err<E: Into<Box<dyn std::error::Error + Send + Sync>>>(
    error: E,
) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::Other, error)
}

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

#[derive(Clone)]
pub(crate) struct Term {
    term: Arc<atomic::AtomicBool>,
    sig: Arc<tokio::sync::Notify>,
    trgr: Arc<dyn Fn() + 'static + Send + Sync>,
}

impl Term {
    pub fn new(trgr: Arc<dyn Fn() + 'static + Send + Sync>) -> Self {
        Self {
            term: Arc::new(atomic::AtomicBool::new(false)),
            sig: Arc::new(tokio::sync::Notify::new()),
            trgr,
        }
    }

    pub fn term(&self) {
        self.term.store(true, atomic::Ordering::Release);
        self.sig.notify_waiters();
        (self.trgr)();
    }

    pub fn is_term(&self) -> bool {
        self.term.load(atomic::Ordering::Acquire)
    }

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
