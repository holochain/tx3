//! Controller for connection pooling / message send management

// --
//
// - outgoing messages (limited by byte count)
// - incoming messages (limited by byte count)
//
// - connection opener (limited by max outgoing con count)
//   - finds next priority outgoing message that doesn't have
//     an already outgoing connection
// - connection acceptor (limited by max incoming con count)
//
// - connection
//   - close on: idle timeout, max message count, max byte count
//     - idle time is only marked on complete messages
//       so bad actors can't send 1 byte a second to muck with us
//     - single messages can go beyond max byte count, but the first
//       of max messages or max bytes to be reached will stop the
//       connection after that current message is sent completely
//     - reader stops reading after any limit reached
//     - writer stops writing after any limit reached
//     - once both have stopped, connection is closed
//     - if either reader or writer error, connection is closed
//
// --

use crate::types::*;
use crate::*;
use std::fmt::Debug;
use std::future::Future;
use std::hash::Hash;
use tokio::io::AsyncRead;
use tokio::io::AsyncWrite;

const FI_LEN_MASK: u32 = 0b00000011111111111111111111111111;

/// Marker trait for something that can act as a remote connection identifier.
pub trait RemId:
    'static + Send + Sync + Debug + PartialEq + Eq + PartialOrd + Ord + Hash
{
}

/// Receive an incoming message from a remote node.
/// Gives mutable access to the internal BytesList, for parsing.
/// The permit limiting the max read bytes in memory is bound to this struct.
/// Drop this to allow that permit to be freed, allowing additional
/// data to be read by the input readers.
pub struct InboundMessage<K: RemId> {
    _permit: tokio::sync::OwnedSemaphorePermit,
    peer_id: Arc<K>,
    content: BytesList,
}

impl<K: RemId> InboundMessage<K> {
    /// Get the remote id of the peer who sent this message.
    pub fn peer_id(&self) -> &Arc<K> {
        &self.peer_id
    }

    /// Get the content of this message.
    pub fn content(&mut self) -> &mut BytesList {
        &mut self.content
    }
}

/// When implementing a pool, the pool needs to be able to make certain
/// calls out to the implementor. This PoolImp trait provides those calls.
pub trait PoolImp: 'static + Send + Sync {
    /// A type which uniquely identifies an endpoint.
    type EndpointId: RemId;

    /// The backend system transport connection type.
    type Connection: 'static + Send + AsyncRead + AsyncWrite + Unpin;

    /// A future which resolves into an "accept" result.
    type AcceptFut: 'static + Send + Future<Output = Result<(
        Self::EndpointId,
        Self::Connection,
    )>>;

    /// A future returned by the "connect" function.
    type ConnectFut: 'static + Send + Future<Output = Result<Self::Connection>>;

    /// Establish a new outgoing connection to a remote peer.
    fn connect(&self, id: Arc<Self::EndpointId>) -> Self::ConnectFut;

    /// Receive an incoming message from a remote node.
    fn incoming(&self, msg: InboundMessage<Self::EndpointId>);
}

/// Configuration for the Tx3Pool instance.
#[non_exhaustive]
pub struct Tx3PoolConfig {
    /// Maximum byte length of a single message.
    pub max_msg_byte_count: u32,

    /// How many conncurrent outgoing connections are allowed.
    pub max_out_con_count: u8,

    /// How many concurrent incoming connections are allowed.
    pub max_in_con_count: u8,

    /// How many bytes can build up in our outgoing buffer before
    /// we start experiencing backpressure.
    pub max_out_byte_count: u32,

    /// How many bytes can build up in our incoming buffer before
    /// we stop reading to trigger backpressure.
    pub max_in_byte_count: u32,

    /// Idle timeout after which to close the connection.
    pub idle_timeout: std::time::Duration,

    /// NOT PUB -- both sides should match -- someday negotiate? --
    /// The max message read/write per connection before closing
    #[allow(dead_code)]
    max_read_write_msg_count_per_con: u32,

    /// NOT PUB -- both sides should match -- someday negotiate? --
    /// The max byte count that can be read/written per con before closing
    #[allow(dead_code)]
    max_read_write_byte_count_per_con: u32,
}

impl Default for Tx3PoolConfig {
    fn default() -> Self {
        Self {
            max_msg_byte_count: FI_LEN_MASK,
            max_out_con_count: 64,
            max_in_con_count: 64,
            max_out_byte_count: FI_LEN_MASK,
            max_in_byte_count: FI_LEN_MASK,
            idle_timeout: std::time::Duration::from_secs(20),
            max_read_write_msg_count_per_con: 64,
            max_read_write_byte_count_per_con: FI_LEN_MASK,
        }
    }
}

/// A Tx3 Connection Pool.
#[derive(Clone)]
pub struct Tx3Pool<I: PoolImp> {
    #[allow(dead_code)]
    imp: Arc<I>,
}

impl<I: PoolImp> Tx3Pool<I> {
    /// Construct a new Tx3 connection pool instance.
    pub fn new(imp: I) -> Self {
        Self {
            imp: Arc::new(imp),
        }
    }

    /// Enqueue an outgoing message for send to a remote peer.
    pub fn send(
        &self,
        _peer_id: Arc<I::EndpointId>,
        _content: BytesList,
    ) -> impl Future<Output = Result<()>> + 'static + Send {
        async move {
            todo!()
        }
    }

    /// Try to accept an incoming connection. Note, if we've reached
    /// our incoming connection limit, this future will be dropped
    /// without being awaited (TooManyConnections).
    pub fn accept(&self, _accept_fut: I::AcceptFut) {
        todo!()
    }
}

/*
}

impl Default for Tx3PoolConfig {
    fn default() -> Self {
        Self {
            max_msg_byte_count: FI_LEN_MASK,
            max_out_con_count: 64,
            max_in_con_count: 64,
            max_out_byte_count: FI_LEN_MASK,
            max_in_byte_count: FI_LEN_MASK,
            max_read_write_msg_count_per_con: 64,
            max_read_write_byte_count_per_con: FI_LEN_MASK,
        }
    }
}

/// A Tx3 Connection Pool.
#[derive(Clone)]
pub struct Tx3Pool<I: PoolImp> {
    #[allow(dead_code)]
    imp: Arc<I>,
}

impl<I: PoolImp> Tx3Pool<I> {
    /// Construct a new Tx3 connection pool instance.
    pub fn new(imp: I) -> Self {
        Self {
            imp: Arc::new(imp),
        }
    }

    /// Enqueue an outgoing message for send to a remote peer.
    pub fn send(
        &self,
        _peer_id: Arc<I::EndpointId>,
        _content: BytesList,
    ) -> impl Future<Output = Result<()>> + 'static + Send {
        async move {
            todo!()
        }
    }

    /// Try to accept an incoming connection. Note, if we've reached
    /// our incoming connection limit, this future will be dropped
    /// without being awaited (TooManyConnections).
    pub fn accept(&self, _accept_fut: I::AcceptFut) {
        todo!()
    }
}

use crate::*;
use crate::types::*;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::collections::VecDeque;

const FI_LEN_MASK: u32 = 0b00000011111111111111111111111111;

/// Marker trait for something that can act as a remote connection identifier.
pub trait RemId:
    'static + Send + Sync + Debug + PartialEq + Eq + PartialOrd + Ord + Hash
{
}

// -- private -- //

struct MsgData {
    _permit: tokio::sync::OwnedSemaphorePermit,
    time: tokio::time::Instant,
    data: BytesList,
}

#[derive(Clone)]
struct OutMsgQueue<K: RemId> {
    byte_limit: Arc<tokio::sync::Semaphore>,
    inner: Arc<Mutex<OutMsgQueueInner<K>>>,
}

impl<K: RemId> OutMsgQueue<K> {
    pub fn new(max_out_bytes: u32) -> Self {
        let byte_limit = Arc::new(tokio::sync::Semaphore::new(max_out_bytes as usize));
        let inner = Arc::new(Mutex::new(OutMsgQueueInner::new()));

        Self {
            byte_limit,
            inner,
        }
    }

    pub fn enqueue(&self, tgt: Arc<K>, data: BytesList) -> impl Future<Output = Result<()>> + 'static + Send {
        let time = tokio::time::Instant::now();
        let byte_limit = self.byte_limit.clone();
        let inner = self.inner.clone();
        async move {
            let want_size = data.remaining();
            if want_size > FI_LEN_MASK as usize {
                return Err(other_err("MsgTooLarge"));
            }
            let permit = byte_limit
                .acquire_many_owned(want_size as u32)
                .await
                .map_err(|_| other_err("PoolClosed"))?;
            let data = MsgData {
                _permit: permit,
                time,
                data,
            };
            OutMsgQueueInner::access(&inner, move |inner| {
                inner.put(tgt, data);
            });
            Ok(())
        }
    }
}

struct OutMsgQueueInner<K: RemId> {
    messages: HashMap<Arc<K>, VecDeque<MsgData>>,
    priority: VecDeque<Arc<K>>,
}

impl<K: RemId> OutMsgQueueInner<K> {
    pub fn new() -> Self {
        Self {
            messages: HashMap::new(),
            priority: VecDeque::new(),
        }
    }

    pub fn access<R, F>(this: &Mutex<Self>, f: F) -> R
    where
        F: FnOnce(&mut Self) -> R,
    {
        f(&mut *this.lock())
    }

    pub fn put(&mut self, tgt: Arc<K>, data: MsgData) {
        if !self.priority.contains(&tgt) {
            self.priority.push_back(tgt.clone());
        }
        self.messages.entry(tgt).or_default().push_back(data);
    }
}

*/
