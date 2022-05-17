//! tx3 pool IO agnostic state logic

use crate::types::*;
use crate::*;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::collections::HashSet;
use std::collections::VecDeque;
use std::fmt::Debug;
use std::hash::Hash;
use std::sync::atomic;

/// Marker trait for something that can act as a remote connection identifier.
pub trait RemId:
    'static + Send + Sync + Debug + PartialEq + Eq + PartialOrd + Ord + Hash
{
}

/// State mutation commands emitted from the state machine
pub enum Mutation<K: RemId> {
    /// Open a new connection, resolve the NewConnectionPermit
    /// with the success or failure of opening the connection.
    NewCon(Arc<K>, NewConResolve<K>),
}

/// Reply with the success or failure of trying to open the connection.
pub struct NewConResolve<K: RemId> {
    id: Arc<K>,
    inner: Option<Arc<Mutex<PoolStateInner<K>>>>,
}

impl<K: RemId> Drop for NewConResolve<K> {
    fn drop(&mut self) {
        self.reject();
    }
}

impl<K: RemId> NewConResolve<K> {
    /// Reject this new con permit... i.e. connect failed.
    pub fn reject(&mut self) {
        if let Some(inner) = self.inner.take() {
            let _ = inner.lock().connect_resolve(self.id.clone(), None);
        }
    }

    /// Resolve that this connect permit had a successful handshake.
    /// If this resolve errors, the connection should be closed.
    pub fn resolve<T>(&mut self, term: T) -> Result<()>
    where
        T: FnOnce() + 'static + Send,
    {
        let term: Term = Box::new(term);
        if let Some(inner) = self.inner.take() {
            inner.lock().connect_resolve(self.id.clone(), Some(term));
            Ok(())
        } else {
            term();
            Err(other_err("ApiError:OnlyResolveOnce"))
        }
    }
}

/// Accept permit indicating it's okay to use system-resources handshaking
/// a new incoming connection. Once that's complete, call `resolve()`.
pub struct AcceptResolve<K: RemId> {
    con_id: ConId,
    inner: Option<Arc<Mutex<PoolStateInner<K>>>>,
}

impl<K: RemId> Drop for AcceptResolve<K> {
    fn drop(&mut self) {
        self.reject();
    }
}

impl<K: RemId> AcceptResolve<K> {
    /// Reject this accept permit... i.e. the handshake failed.
    pub fn reject(&mut self) {
        if let Some(inner) = self.inner.take() {
            let _ = inner.lock().accept_resolve(self.con_id, None);
        }
    }

    /// Resolve that this accept permit had a successful handshake.
    /// If this resolve errors, the connection should be closed.
    pub fn resolve<T>(&mut self, id: Arc<K>, term: T) -> Result<()>
    where
        T: FnOnce() + 'static + Send,
    {
        let term: Term = Box::new(term);
        if let Some(inner) = self.inner.take() {
            inner.lock().accept_resolve(self.con_id, Some((id, term)))
        } else {
            term();
            Err(other_err("ApiError:OnlyResolveOnce"))
        }
    }
}

/// Future returned by `get_con()`, resolves to a ConId or error.
pub struct GetConFut(tokio::sync::oneshot::Receiver<Result<()>>);

impl std::future::Future for GetConFut {
    type Output = Result<()>;

    fn poll(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        match std::pin::Pin::new(&mut self.0).poll(cx) {
            std::task::Poll::Pending => std::task::Poll::Pending,
            std::task::Poll::Ready(Err(_)) => {
                std::task::Poll::Ready(Err(other_err("ConClosed")))
            }
            std::task::Poll::Ready(Ok(r)) => std::task::Poll::Ready(r),
        }
    }
}

/// tx3 pool IO agnostic state logic
#[derive(Clone)]
pub struct PoolState<K: RemId>(Arc<Mutex<PoolStateInner<K>>>);

impl<K: RemId> PoolState<K> {
    /// Construct a new state logic instance
    pub fn new<M>(
        max_cons: usize,
        connect_timeout: std::time::Duration,
        idle_timeout_loose: std::time::Duration,
        idle_timeout_tight: std::time::Duration,
        mutate_emit: M,
    ) -> Self
    where
        M: FnMut(Mutation<K>) + 'static + Send,
    {
        let mutate_emit = Box::new(mutate_emit);
        Self(Arc::new(Mutex::new(PoolStateInner {
            max_cons,
            connect_timeout,
            idle_timeout_loose,
            idle_timeout_tight,
            mutate_emit,
            cons: HashMap::new(),
            pending_incoming: HashSet::new(),
            outgoing_wait_queue: VecDeque::new(),
        })))
    }

    /// Check timeouts. Call this at the granularity you care about
    /// timing out your connections (and thus freeing permits to
    /// establish new connections). Once per second should be sufficient.
    pub fn timeout(&self) {
        let this = self.0.clone();
        self.access(move |inner| {
            inner.timeout(this);
        });
    }

    /// Get a permit to accept a new incoming connection,
    /// if this returns `None` the connection should be rejected.
    /// (TooManyConnections)
    pub fn accept(&self) -> Option<AcceptResolve<K>> {
        let this = self.0.clone();
        self.access(move |inner| inner.accept(this))
    }

    /// If a connection is already established, will resolve,
    /// otherwise will queue a request to establish an
    /// outgoing connection, causing a NewCon mutation to be emitted,
    /// and resolving with the result of that mutation or a timeout error.
    pub fn get_con<F>(&self, id: Arc<K>) -> GetConFut {
        let this = self.0.clone();
        let (s, r) = tokio::sync::oneshot::channel();
        self.access(move |inner| {
            inner.get_con(id, s, this);
        });
        GetConFut(r)
    }

    /// Remove a connection, potentially freeing permits for pending
    /// outgoing or incomming connections to be established. This should
    /// be called on error reading or writing to the connection, but can
    /// also be called arbitrarily if the connection should be closed.
    pub fn close_con(&self, id: Arc<K>) {
        self.access(move |inner| {
            inner.remove_con(id);
        });
    }

    /// Mark that a connection did work, either reading incoming data
    /// or writing outgoing data. I.e. this connection is no longer idle.
    pub fn active_con(&self, id: &Arc<K>) {
        self.access(move |inner| {
            inner.active_con(id);
        });
    }

    // -- private -- //

    fn access<R, F>(&self, f: F) -> R
    where
        F: FnOnce(&mut PoolStateInner<K>) -> R,
    {
        let mut inner = self.0.lock();
        f(&mut *inner)
    }
}

// -- private -- //

/// private opaque connection identifier
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct ConId(u64);

impl ConId {
    fn next() -> Self {
        static NEXT: atomic::AtomicU64 = atomic::AtomicU64::new(0);

        Self(NEXT.fetch_add(1, atomic::Ordering::Relaxed))
    }
}

type MutateEmit<K> = Box<dyn FnMut(Mutation<K>) + 'static + Send>;
type GetCon = tokio::sync::oneshot::Sender<Result<()>>;
type Term = Box<dyn FnOnce() + 'static + Send>;

struct ConInfo {
    last_time: tokio::time::Instant,
    active: bool,
    waiting: Vec<(tokio::time::Instant, GetCon)>,
    term: Option<Term>,
}

impl Drop for ConInfo {
    fn drop(&mut self) {
        if let Some(term) = self.term.take() {
            for (_to, f) in self.waiting.drain(..) {
                let _ = f.send(Err(other_err("ConDropped")));
            }
            term();
        }
    }
}

struct PoolStateInner<K: RemId> {
    /// Max connection count.
    max_cons: usize,

    /// Time to allow for connection establishment
    connect_timeout: std::time::Duration,

    /// If we are not near the max_cons count, we allow this time
    /// for connections to remain idle.
    idle_timeout_loose: std::time::Duration,

    /// If we are near the max_cons count, we allow this time
    /// for connections to remain idle.
    idle_timeout_tight: std::time::Duration,

    /// Callback for outputing state mutations
    /// (like the directive to establish a new outgoing connection).
    mutate_emit: MutateEmit<K>,

    /// Our list of active connections (both incoming and outgoing),
    /// as well as our pending outgoing connections.
    cons: HashMap<Arc<K>, ConInfo>,

    /// Our list of pending incoming connections.
    /// This has to be separate because we don't know the remote id yet.
    pending_incoming: HashSet<ConId>,

    /// Local code has requested an outgoing connection, but we were at
    /// our max connection count. This is a queue for those pending requests
    /// until they timeout or are able to be serviced.
    outgoing_wait_queue: VecDeque<(tokio::time::Instant, Arc<K>, GetCon)>,
}

impl<K: RemId> PoolStateInner<K> {
    /// Count our current connections.
    /// outgoing_wait_queue connections don't count, because
    /// they haven't been opened yet.
    /// pending_incoming *do* count, because they've been
    /// accepted, and are in the process of handshaking,
    /// we can't put the min the cons list, because we don't
    /// know the remote id yet, until handshaking completes.
    fn count(&self) -> usize {
        self.cons.len() + self.pending_incoming.len()
    }

    /// we're about to do something that will increase our open connection limit
    /// check to see if we need to clear up some space first.
    fn check_clear(&mut self) {
        if self.count() >= self.max_cons {
            // we're at our max connection limit, use the tight timeout
            self.check_idle_timeout(self.idle_timeout_tight);
        }
        if self.count() >= self.max_cons {
            self.check_connect_timeout();
        }
    }

    /// See if we need to close any connections due to idle timeout.
    fn check_idle_timeout(&mut self, timeout: std::time::Duration) {
        let now = tokio::time::Instant::now();
        let mut remove = Vec::new();
        for (id, con) in self.cons.iter() {
            if con.active && now - con.last_time >= timeout {
                remove.push(id.clone());
            }
        }
        for id in remove {
            self.remove_con(id);
        }
    }

    /// See if we need to close any pending connections due to connect timeout.
    fn check_connect_timeout(&mut self) {
        let now = tokio::time::Instant::now();
        for (time, id, f) in std::mem::take(&mut self.outgoing_wait_queue) {
            if now - time >= self.connect_timeout {
                let _ = f.send(Err(other_err("Timeout")));
            } else {
                self.outgoing_wait_queue.push_back((time, id, f));
            }
        }
        let mut remove = Vec::new();
        for (id, con) in self.cons.iter() {
            if !con.active && now - con.last_time >= self.connect_timeout {
                remove.push(id.clone());
            }
        }
        for id in remove {
            self.remove_con(id);
        }
    }

    /// See if there are any wait connections queued that we can service now.
    fn check_wait_queue(&mut self, inner: Arc<Mutex<Self>>) {
        while !self.outgoing_wait_queue.is_empty()
            && self.count() < self.max_cons
        {
            let (to, id, f) = self.outgoing_wait_queue.pop_front().unwrap();
            self.cons.insert(
                id.clone(),
                ConInfo {
                    last_time: to,
                    active: false,
                    waiting: vec![(to, f)],
                    term: None,
                },
            );
            (self.mutate_emit)(Mutation::NewCon(
                id.clone(),
                NewConResolve {
                    id,
                    inner: Some(inner.clone()),
                },
            ));
        }
    }

    /// Code to run when the high-level "timeout" function is called.
    pub fn timeout(&mut self, inner: Arc<Mutex<Self>>) {
        let tq = (self.max_cons * 3) / 4;
        if self.count() > tq {
            self.check_idle_timeout(self.idle_timeout_tight);
        } else {
            self.check_idle_timeout(self.idle_timeout_loose);
        }
        self.check_connect_timeout();
        self.check_wait_queue(inner);
    }

    /// See if we can accept an incoming connection.
    pub fn accept(
        &mut self,
        inner: Arc<Mutex<Self>>,
    ) -> Option<AcceptResolve<K>> {
        self.check_clear();

        if self.count() >= self.max_cons {
            return None;
        }

        let con_id = ConId::next();
        self.pending_incoming.insert(con_id);
        Some(AcceptResolve {
            con_id,
            inner: Some(inner),
        })
    }

    /// Resolve a previously pending incoming connection.
    pub fn accept_resolve(
        &mut self,
        con_id: ConId,
        res: Option<(Arc<K>, Term)>,
    ) -> Result<()> {
        self.pending_incoming.remove(&con_id);
        if let Some((id, term)) = res {
            use std::collections::hash_map::Entry;
            match self.cons.entry(id) {
                Entry::Occupied(_) => {
                    term();
                    Err(other_err("ExistingRemId"))
                }
                Entry::Vacant(e) => {
                    e.insert(ConInfo {
                        last_time: tokio::time::Instant::now(),
                        active: true,
                        waiting: Vec::new(),
                        term: Some(term),
                    });
                    Ok(())
                }
            }
        } else {
            Err(other_err("ConError"))
        }
    }

    /// Resolve a previously pending outgoing connection.
    pub fn connect_resolve(&mut self, id: Arc<K>, res: Option<Term>) {
        if let Some(term) = res {
            if let Some(con) = self.cons.get_mut(&id) {
                con.last_time = tokio::time::Instant::now();
                con.active = true;
                con.term = Some(term);
                for (_to, f) in con.waiting.drain(..) {
                    let _ = f.send(Ok(()));
                }
            }
        } else {
            self.remove_con(id);
        }
    }

    /// Internal get connection code.
    pub fn get_con(&mut self, id: Arc<K>, f: GetCon, inner: Arc<Mutex<Self>>) {
        let now = tokio::time::Instant::now();

        if let Some(con) = self.cons.get_mut(&id) {
            if con.active {
                let _ = f.send(Ok(()));
            } else {
                con.waiting.push((now, f));
            }
            return;
        }

        self.check_clear();
        self.outgoing_wait_queue.push_back((now, id, f));
        self.check_wait_queue(inner);
    }

    /// Internal close connection code.
    pub fn remove_con(&mut self, id: Arc<K>) {
        if let Some(mut con) = self.cons.remove(&id) {
            for (_to, f) in con.waiting.drain(..) {
                let _ = f.send(Err(other_err("ConRemoved")));
            }
        }
    }

    /// Internal activate connection code.
    pub fn active_con(&mut self, id: &Arc<K>) {
        if let Some(con) = self.cons.get_mut(id) {
            con.last_time = tokio::time::Instant::now();
        }
    }
}
