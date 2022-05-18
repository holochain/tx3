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
//   - close on: 0 len msg, idle timeout, max message count, max byte count
//     - well behaved actors send a zero length message after the message
//       count or byte count limits, or if they have no more data to send
//       allowing the read side to know to immediately close
//     - single messages can go beyond max byte count, but the first
//       of max messages or max bytes to be reached will stop the
//       connection after that current message is sent completely
//     - reader stops reading after any limit reached
//     - writer stops writing after any limit reached
//     - once both have stopped, connection is closed
//     - if either reader or writer error, connection is closed
//   - idle timeout considerations
//     - receiving a complete message sets the last tick time,
//       and resets the bytes received since last tick time was set.
//     - sending a complete message sets the last tick time,
//       and resets the bytes sent since last tick time was set.
//     - to prevent somebody sending (or limiting receipt to) one byte
//       per second from clogging all connections, if the idle time is
//       about to be checked, readers will check that read bytes per
//       second are over the threshold, and writers will check write
//       bytes per second.
//
// --

use crate::types::*;
use crate::*;
use std::fmt::Debug;
use std::future::Future;
use std::hash::Hash;
use std::collections::HashMap;
use std::collections::VecDeque;
use tokio::io::AsyncRead;
use tokio::io::AsyncWrite;
use parking_lot::Mutex;

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
pub struct InboundMsg<K: RemId> {
    _permit: tokio::sync::OwnedSemaphorePermit,
    peer_id: Arc<K>,
    content: BytesList,
}

impl<K: RemId> InboundMsg<K> {
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
    fn incoming(&self, msg: InboundMsg<Self::EndpointId>);
}

/// Configuration for the Tx3Pool instance.
#[non_exhaustive]
pub struct Tx3PoolConfig {
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

impl Default for Tx3PoolConfig {
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

/// A Tx3 Connection Pool.
#[derive(Clone)]
pub struct Tx3Pool<I: PoolImp> {
    config: Arc<Tx3PoolConfig>,
    out_byte_limit: Arc<tokio::sync::Semaphore>,
    pool_term: Term,
    pool_state: PoolState<I::EndpointId>,
}

impl<I: PoolImp> Tx3Pool<I> {
    /// Construct a new Tx3 connection pool instance.
    pub fn new(config: Arc<Tx3PoolConfig>, imp: Arc<I>) -> Self {
        let out_byte_limit = Arc::new(tokio::sync::Semaphore::new(
            config.max_out_byte_count as usize,
        ));

        let pool_term = Term::new(Arc::new(|| {}));
        let pool_state = PoolState::default();

        tokio::task::spawn(connector_task::<I>(
            config.clone(),
            imp,
            pool_term.clone(),
            pool_state.clone(),
        ));

        Self {
            config,
            out_byte_limit,
            pool_term,
            pool_state,
        }
    }

    /// Enqueue an outgoing message for send to a remote peer.
    pub fn send(
        &self,
        peer_id: Arc<I::EndpointId>,
        content: BytesList,
    ) -> impl Future<Output = Result<()>> + 'static + Send {
        let max_msg_byte_count = self.config.max_msg_byte_count;
        let msg_send_timeout = self.config.msg_send_timeout;
        let out_byte_limit = self.out_byte_limit.clone();
        let pool_state = self.pool_state.clone();
        async move {
            let size = content.remaining();
            if size > max_msg_byte_count as usize {
                return Err(other_err("MsgTooLarge"));
            }
            let timeout_at = tokio::time::Instant::now() + msg_send_timeout;
            tokio::time::timeout_at(timeout_at, async move {
                let permit = out_byte_limit
                    .acquire_many_owned(size as u32)
                    .await
                    .map_err(other_err)?;
                let (resolve, result) = tokio::sync::oneshot::channel();
                let msg = OutboundMsg::new(
                    permit,
                    timeout_at,
                    content,
                    resolve,
                );
                pool_state.out_enqueue(peer_id, msg);
                result.await.map_err(other_err)?
            }).await.map_err(|_| other_err("Timeout"))?
        }
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
        self.pool_term.term();
    }
}

#[derive(Clone)]
struct OutboundMsg(Arc<Mutex<Option<OutboundMsgInner>>>);

impl OutboundMsg {
    pub fn new(
        permit: tokio::sync::OwnedSemaphorePermit,
        timeout_at: tokio::time::Instant,
        content: BytesList,
        resolve: tokio::sync::oneshot::Sender<Result<()>>,
    ) -> Self {
        let this = Self(Arc::new(Mutex::new(Some(OutboundMsgInner {
            permit,
            timeout_at,
            content,
            resolve,
        }))));
        {
            let this = this.clone();
            tokio::task::spawn(async move {
                tokio::time::sleep_until(timeout_at).await;
                this.resolve(Err(other_err("Timeout")));
            });
        }
        this
    }

    pub fn timeout_at(&self) -> tokio::time::Instant {
        OutboundMsgInner::access(&self.0, |inner| {
            if let Some(inner) = inner {
                inner.timeout_at
            } else {
                tokio::time::Instant::now()
            }
        })
    }

    pub fn resolve(&self, result: Result<()>) {
        if let Some(resolve) = OutboundMsgInner::access(&self.0, |inner| {
            inner.take().map(|inner| inner.resolve)
        }) {
            let _ = resolve.send(result);
        }
    }

    pub fn extract_inner(&self) -> Option<(
        tokio::sync::OwnedSemaphorePermit,
        BytesList,
        tokio::sync::oneshot::Sender<Result<()>>,
    )> {
        if let Some(inner) = OutboundMsgInner::access(&self.0, |inner| {
            inner.take()
        }) {
            Some((inner.permit, inner.content, inner.resolve))
        } else {
            None
        }
    }
}

struct OutboundMsgInner {
    permit: tokio::sync::OwnedSemaphorePermit,
    timeout_at: tokio::time::Instant,
    content: BytesList,
    resolve: tokio::sync::oneshot::Sender<Result<()>>,
}

impl OutboundMsgInner {
    fn access<R, F>(this: &Mutex<Option<Self>>, f: F) -> R
    where
        F: FnOnce(&mut Option<Self>) -> R,
    {
        f(&mut *this.lock())
    }
}

struct PoolState<K: RemId> {
    /// inner state
    inner: Arc<Mutex<PoolStateInner<K>>>,

    /// notify connector task that we have a new outgoing bucket
    /// if it has capacity to open a new outgoing connection.
    out_new_peer_notify: Arc<tokio::sync::Notify>,
}

impl<K: RemId> Clone for PoolState<K> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            out_new_peer_notify: self.out_new_peer_notify.clone(),
        }
    }
}

impl<K: RemId> Default for PoolState<K> {
    fn default() -> Self {
        Self {
            inner: Arc::new(Mutex::new(PoolStateInner::default())),
            out_new_peer_notify: Arc::new(tokio::sync::Notify::new()),
        }
    }
}

impl<K: RemId> PoolState<K> {
    /// Get the out bucket notifier
    pub fn get_out_new_peer_notify(&self) -> Arc<tokio::sync::Notify> {
        self.out_new_peer_notify.clone()
    }

    /// Enqueue a new outgoing message. Messages are grouped into buckets
    /// per destination peer_id. If a new bucket is created, our connector
    /// task is notified incase it wants to open a new connection.
    pub fn out_enqueue(&self, peer_id: Arc<K>, msg: OutboundMsg) {
        let timeout_at = msg.timeout_at();
        if timeout_at >= tokio::time::Instant::now() {
            msg.resolve(Err(other_err("Timeout")));
            return;
        }
        PoolStateInner::access(&self.inner, move |inner| {
            inner.out_enqueue(peer_id, timeout_at, msg);
        });
        // is there an efficient way to do this only if it's a new peer?
        self.out_new_peer_notify.notify_waiters();
    }

    /// Grab the next outbound message for a given peer,
    /// if this returns None, there are no more messages, and the bucket
    /// will be cleared.
    pub fn out_dequeue(&self, peer_id: &Arc<K>) -> Option<OutboundMsg> {
        PoolStateInner::access(&self.inner, |inner| {
            inner.out_dequeue(peer_id)
        })
    }

    /// If we have a valid peer_id in our out_priority queue, this will
    /// return that peer_id, but first we check for timeouts...
    /// If there are no peers, this will return None.
    pub fn get_next_con(&self) -> Option<ConGuard<K>> {
        if let Some(peer_id) = PoolStateInner::access(
            &self.inner,
            |inner| inner.get_next_con(),
        ) {
            Some(ConGuard {
                pool_state: self.clone(),
                peer_id,
            })
        } else {
            None
        }
    }
}

struct ConGuard<K: RemId> {
    pool_state: PoolState<K>,
    peer_id: Arc<K>,
}

impl<K: RemId> Drop for ConGuard<K> {
    fn drop(&mut self) {
        let peer_id = self.peer_id.clone();
        PoolStateInner::access(&self.pool_state.inner, move |inner| {
            inner.con_guard_drop(peer_id);
        });
    }
}

impl<K: RemId> ConGuard<K> {
    pub fn peer_id(&self) -> Arc<K> {
        self.peer_id.clone()
    }
}

struct ConInfo {
    /// do we have an active connection task for the related peer_id?
    is_active: bool,

    /// if a cooldown is specified and not elapsed, we will not open
    /// a connection to this peer.
    cooldown: tokio::time::Instant,
}

struct PoolStateInner<K: RemId> {
    /// Queue of outgoing messages, awaiting a connection to send.
    out_queue: VecDeque<(
        Arc<K>,
        tokio::time::Instant,
        OutboundMsg,
    )>,

    /// Information about connections for peer_ids,
    con_info: HashMap<Arc<K>, ConInfo>,
}

impl<K: RemId> Default for PoolStateInner<K> {
    fn default() -> Self {
        Self {
            out_queue: VecDeque::new(),
            con_info: HashMap::new(),
        }
    }
}

impl<K: RemId> PoolStateInner<K> {
    fn access<R, F>(this: &Mutex<Self>, f: F) -> R
    where
        F: FnOnce(&mut Self) -> R,
    {
        f(&mut *this.lock())
    }

    fn out_enqueue(
        &mut self,
        peer_id: Arc<K>,
        timeout_at: tokio::time::Instant,
        msg: OutboundMsg,
    ) {
        self.out_queue.push_back((peer_id, timeout_at, msg));
    }

    fn out_dequeue(&mut self, want_peer_id: &Arc<K>) -> Option<OutboundMsg> {
        self.clear_timeouts();
        let mut found_idx = 0;
        let mut did_find = false;
        for (idx, (peer_id, _, _)) in self.out_queue.iter().enumerate() {
            if peer_id == want_peer_id {
                found_idx = idx;
                did_find = true;
                break;
            }
        }
        if !did_find {
            return None;
        }
        if let Some((_, _, msg)) = self.out_queue.remove(found_idx) {
            Some(msg)
        } else {
            None
        }
    }

    fn clear_timeouts(&mut self) {
        let now = tokio::time::Instant::now();
        self.out_queue.retain(|(_peer_id, timeout_at, msg)| {
            if *timeout_at >= now {
                msg.resolve(Err(other_err("Timeout")));
                false
            } else {
                true
            }
        });
        self.con_info.retain(|_peer_id, con_info| {
            if con_info.is_active {
                return true;
            }
            if con_info.cooldown > now {
                return true;
            }
            false
        });
    }

    fn get_next_con(&mut self) -> Option<Arc<K>> {
        self.clear_timeouts();
        for (peer_id, _, _) in self.out_queue.iter() {
            if !self.con_info.contains_key(peer_id) {
                self.con_info.insert(peer_id.clone(), ConInfo {
                    is_active: true,
                    cooldown: tokio::time::Instant::now(),
                });
                return Some(peer_id.clone());
            }
        }
        None
    }

    fn con_guard_drop(&mut self, peer_id: Arc<K>) {
        let mut remove = false;
        if let Some(con_info) = self.con_info.get_mut(&peer_id) {
            con_info.is_active = false;
            if tokio::time::Instant::now() >= con_info.cooldown {
                remove = true;
            }
        }
        if remove {
            self.con_info.remove(&peer_id);
        }
    }
}

async fn connector_task<I: PoolImp>(
    config: Arc<Tx3PoolConfig>,
    imp: Arc<I>,
    pool_term: Term,
    pool_state: PoolState<I::EndpointId>,
) {
    let out_con_limit = Arc::new(tokio::sync::Semaphore::new(
        config.max_out_con_count as usize,
    ));

    let mut con_permit = None;

    let out_new_peer_notify = pool_state.get_out_new_peer_notify();
    'connector_loop: loop {
        if pool_term.is_term() {
            break 'connector_loop;
        }

        if con_permit.is_none() {
            match tokio::select! {
                _ = pool_term.on_term() => break 'connector_loop,
                p = out_con_limit.clone().acquire_owned() => p,
            } {
                Ok(p) => con_permit = Some(p),
                Err(_) => break 'connector_loop,
            }
        }

        let wait_notified = out_new_peer_notify.notified();

        match pool_state.get_next_con() {
            None => {
                tokio::select! {
                    _ = pool_term.on_term() => break 'connector_loop,
                    _ = wait_notified => (),
                }
            }
            Some(con_guard) => {
                let con_permit = con_permit.take().unwrap();
                tokio::task::spawn(connect_task::<I>(
                    config.clone(),
                    imp.clone(),
                    pool_term.clone(),
                    pool_state.clone(),
                    con_guard,
                    con_permit,
                ));
            }
        }
    }

    tracing::debug!("ConnectorLoopEnded");
}

async fn connect_task<I: PoolImp>(
    config: Arc<Tx3PoolConfig>,
    imp: Arc<I>,
    pool_term: Term,
    pool_state: PoolState<I::EndpointId>,
    con_guard: ConGuard<I::EndpointId>,
    con_permit: tokio::sync::OwnedSemaphorePermit,
) {
    let con = tokio::select! {
        _ = pool_term.on_term() => return,
        c = imp.connect(con_guard.peer_id()) => match c {
            Err(err) => {
                tracing::debug!(?err);
                // TODO backoff timing - maybe when the con_guard drops?
                return;
            }
            Ok(con) => con,
        }
    };

    // split our connection in two, so we can treat it independently
    let (read_con, write_con) = tokio::io::split(con);

    // split our permit in two, so both halves must close for the
    // permit to be dropped.
    let con_read_permit = Arc::new(con_permit);
    let con_write_permit = con_read_permit.clone();

    // same with the con guard
    let con_read_guard = Arc::new(con_guard);
    let con_write_guard = con_read_guard.clone();

    tokio::task::spawn(read_task::<I>(
        read_con,
        config.clone(),
        pool_term.clone(),
        pool_state.clone(),
        con_read_guard,
        con_read_permit,
    ));

    tokio::task::spawn(write_task::<I>(
        write_con,
        config,
        pool_term,
        pool_state,
        con_write_guard,
        con_write_permit,
    ));
}

async fn read_task<I: PoolImp>(
    _con: tokio::io::ReadHalf<I::Connection>,
    _config: Arc<Tx3PoolConfig>,
    _pool_term: Term,
    _pool_state: PoolState<I::EndpointId>,
    _con_guard: Arc<ConGuard<I::EndpointId>>,
    _con_permit: Arc<tokio::sync::OwnedSemaphorePermit>,
) {
}

async fn write_task<I: PoolImp>(
    mut con: tokio::io::WriteHalf<I::Connection>,
    _config: Arc<Tx3PoolConfig>,
    pool_term: Term,
    pool_state: PoolState<I::EndpointId>,
    con_guard: Arc<ConGuard<I::EndpointId>>,
    _con_permit: Arc<tokio::sync::OwnedSemaphorePermit>,
) {
    use tokio::io::AsyncWriteExt;
    let peer_id = con_guard.peer_id();
    'write_loop: loop {
        let msg = match pool_state.out_dequeue(&peer_id) {
            None => break 'write_loop,
            Some(msg) => msg,
        };
        let (_permit, mut content, resolve) = match msg.extract_inner() {
            None => continue 'write_loop,
            Some(r) => r,
        };

        while content.has_remaining() {
            tokio::select! {
                _ = pool_term.on_term() => break 'write_loop,
                r = con.write(content.chunk()) => match r {
                    Ok(n) => content.advance(n),
                    Err(err) => {
                        tracing::debug!(?err);
                        let _ = resolve.send(Err(err));
                        break 'write_loop;
                    }
                }
            }
        }

        let _ = resolve.send(Ok(()));
    }
}
