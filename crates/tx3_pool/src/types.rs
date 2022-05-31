//! Support types for Tx3Pool implementations.

use crate::*;
use bytes::Buf;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::future::Future;
use std::io::Result;
use std::net::SocketAddr;
use std::sync::Arc;

pub(crate) const FI_LEN_MASK: u32 = 0b00000011111111111111111111111111;

/// Receive an incoming message from a remote node.
/// Gives mutable access to the internal BytesList, for parsing.
/// The permit limiting the max read bytes in memory is bound to this struct.
/// Drop this to allow that permit to be freed, allowing additional
/// data to be read by the input readers.
pub(crate) struct InboundMsg {
    pub(crate) _permit: tokio::sync::OwnedSemaphorePermit,
    pub peer_id: Arc<Tx3Id>,
    pub content: BytesList,
}

impl InboundMsg {
    /// Extract the contents of this message, dropping the permit,
    /// thereby allowing additional messages to be buffered.
    pub fn extract(self) -> (Arc<Tx3Id>, BytesList) {
        (self.peer_id, self.content)
    }
}

/// Tx3Pool operates using peer_ids. When it wants to establish a new
/// outgoing connection, it must be able to look up an addr by a peer_id.
pub trait AddrStore: 'static + Send + Sync {
    /// Future result type from looking up an addr.
    type LookupAddrFut: 'static + Send + Future<Output = Result<Arc<Tx3Addr>>>;

    /// Look up a Tx3Addr from a given Tx3Id.
    fn lookup_addr(&self, id: &Arc<Tx3Id>) -> Self::LookupAddrFut;
}

/// A simple in-memory address store.
pub struct AddrStoreMem(Mutex<HashMap<Arc<Tx3Id>, Arc<Tx3Addr>>>);

impl Default for AddrStoreMem {
    fn default() -> Self {
        Self(Mutex::new(HashMap::new()))
    }
}

impl AddrStoreMem {
    /// Set an item in the memory address store.
    pub fn set<A: tx3::IntoAddr>(&self, addr: A) {
        let addr = addr.into_addr();
        self.access(move |inner| {
            if let Some(id) = &addr.id {
                inner.insert(id.clone(), addr);
            }
        });
    }

    // -- private -- //
    fn access<R, F>(&self, f: F) -> R
    where
        F: FnOnce(&mut HashMap<Arc<Tx3Id>, Arc<Tx3Addr>>) -> R,
    {
        f(&mut *self.0.lock())
    }
}

impl AddrStore for AddrStoreMem {
    type LookupAddrFut = futures::future::Ready<Result<Arc<Tx3Addr>>>;

    fn lookup_addr(&self, id: &Arc<Tx3Id>) -> Self::LookupAddrFut {
        futures::future::ready(self.access(|inner| match inner.get(id) {
            Some(addr) => Ok(addr.clone()),
            None => Err(other_err(format!("NoAddrForId({})", id))),
        }))
    }
}

/// Tx3Pool hooks trait for calls / events to the implementor.
pub trait PoolHooks: 'static + Send + Sync {
    /// Future result type for connect_pre method.
    type ConnectPreFut: 'static + Send + Future<Output = bool>;

    /// Future result type for accept_addr method.
    type AcceptAddrFut: 'static + Send + Future<Output = bool>;

    /// Future result type for accept_id method.
    type AcceptIdFut: 'static + Send + Future<Output = bool>;

    /// The pool is now externally reachable at a different address.
    fn addr_update(&self, addr: Arc<Tx3Addr>);

    /// We are about to establish a new connection to the given address.
    /// If this is okay, return `true`, if not return `false`.
    fn connect_pre(&self, addr: Arc<Tx3Addr>) -> Self::ConnectPreFut;

    /// We are about to accept an incoming connection from the given
    /// socket address. If this is okay, return `true`, if not return `false`.
    /// Note, this method will be called before we do the tls handshaking,
    /// so we don't know the Tx3Id of the remote node.
    fn accept_addr(&self, addr: SocketAddr) -> Self::AcceptAddrFut;

    /// We are about to accept an incoming connection from the given
    /// Tx3Id. If this is okay, return `true`, if not return `false`.
    /// Note, this method will be called *after* tls handshaking,
    /// so if possible, prefer rejecting connections via `accept_addr()`.
    fn accept_id(&self, id: Arc<Tx3Id>) -> Self::AcceptIdFut;
}

/// A default hooks implementation with minimal / no-op implementations.
#[derive(Default)]
pub struct PoolHooksDefault;

impl PoolHooks for PoolHooksDefault {
    type ConnectPreFut = futures::future::Ready<bool>;
    type AcceptAddrFut = futures::future::Ready<bool>;
    type AcceptIdFut = futures::future::Ready<bool>;

    fn addr_update(&self, addr: Arc<Tx3Addr>) {
        let _ = addr;
    }

    fn connect_pre(&self, addr: Arc<Tx3Addr>) -> Self::ConnectPreFut {
        let _ = addr;
        futures::future::ready(true)
    }

    fn accept_addr(&self, addr: SocketAddr) -> Self::AcceptAddrFut {
        let _ = addr;
        futures::future::ready(true)
    }

    fn accept_id(&self, id: Arc<Tx3Id>) -> Self::AcceptAddrFut {
        let _ = id;
        futures::future::ready(true)
    }
}

/// Implementation trait for configuring Tx3Pool sub types.
pub trait Tx3PoolImp: 'static + Send + Sync {
    /// The address store type to use in this tx3 pool instance.
    type AddrStore: AddrStore;

    /// The pool hooks type associated with this tx3 pool instance.
    type PoolHooks: PoolHooks;

    /// Get the address store associated with this implementation instance.
    fn get_addr_store(&self) -> &Arc<Self::AddrStore>;

    /// Get the pool hooks accosiated with this implementation instance.
    fn get_pool_hooks(&self) -> &Arc<Self::PoolHooks>;
}

/// A provided default Tx3PoolImp struct with sane defaults.
#[derive(Default)]
pub struct Tx3PoolImpDefault {
    addr_store: Arc<AddrStoreMem>,
    pool_hooks: Arc<PoolHooksDefault>,
}

impl Tx3PoolImp for Tx3PoolImpDefault {
    type PoolHooks = PoolHooksDefault;
    type AddrStore = AddrStoreMem;

    fn get_addr_store(&self) -> &Arc<Self::AddrStore> {
        &self.addr_store
    }

    fn get_pool_hooks(&self) -> &Arc<Self::PoolHooks> {
        &self.pool_hooks
    }
}

/// Configuration for a tx3 pool instance.
#[non_exhaustive]
pub struct Tx3PoolConfig {
    /// Tls configuration to use for this tx3 pool, or None
    /// indicating we should generate a new ephemeral certificate.
    pub tls: Option<tx3::tls::TlsConfig>,

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
    pub max_out_byte_count: usize,

    /// How many bytes can build up in our incoming buffer before
    /// we stop reading to trigger backpressure.
    /// (Cannot be > usize::MAX >> 3 do to tokio semaphore limitations)
    /// Default: u32::MAX >> 6.
    pub max_in_byte_count: usize,

    /// Connect timeout after which to abandon establishing new connections,
    /// or handshaking incoming connections.
    /// Default: 8 seconds.
    pub connect_timeout: std::time::Duration,

    /// NOT PUB -- both sides should match -- someday negotiate? --
    /// See [pool](crate::pool) module docs.
    /// Default: 4 seconds.
    pub(crate) con_tgt_time: std::time::Duration,

    /// NOT PUB -- both sides should match -- someday negotiate? --
    /// See [pool](crate::pool) module docs.
    /// Default: 65_536 (524,288 bps up + down).
    #[allow(dead_code)]
    pub(crate) rate_min_bytes_per_s: u32,
}

impl Default for Tx3PoolConfig {
    fn default() -> Self {
        Self {
            tls: None,
            max_msg_byte_count: FI_LEN_MASK,
            max_out_con_count: 64,
            max_in_con_count: 64,
            max_out_byte_count: FI_LEN_MASK as usize,
            max_in_byte_count: FI_LEN_MASK as usize,
            connect_timeout: std::time::Duration::from_secs(8),
            con_tgt_time: std::time::Duration::from_secs(4),
            rate_min_bytes_per_s: 65_536,
        }
    }
}

/// A set of distinct chunks of bytes that can be treated as a single unit
#[derive(Clone, Default)]
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
        if data.has_remaining() {
            self.0.push_back(data);
        }
    }

    /// Copy data into a Vec<u8>. You should avoid this if possible.
    pub fn to_vec(&self) -> Vec<u8> {
        use std::io::Read;
        let mut out = Vec::with_capacity(self.remaining());
        // data is local, it can't error, safe to unwrap
        self.clone().reader().read_to_end(&mut out).unwrap();
        out
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
