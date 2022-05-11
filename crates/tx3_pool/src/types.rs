//! Types you'll need if you're implementing transport newtypes for a pool.

use crate::*;
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
    'static + Send + Stream<Item = Self::AcceptFut>
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
}

impl Term {
    pub fn new() -> Self {
        Self {
            term: Arc::new(atomic::AtomicBool::new(false)),
            sig: Arc::new(tokio::sync::Notify::new()),
        }
    }

    pub fn term(&self) {
        self.term.store(true, atomic::Ordering::Release);
        self.sig.notify_waiters();
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

#[derive(Debug)]
pub(crate) enum StoryLine {
    Success,
    Dropped,
}

impl std::fmt::Display for StoryLine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self)
    }
}

impl std::error::Error for StoryLine {}

#[derive(Default, Debug)]
pub(crate) struct Story(Vec<StoryLine>);

impl std::fmt::Display for Story {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self)
    }
}

impl Story {
    pub fn push(&mut self, story_line: StoryLine) {
        self.0.push(story_line);
    }
}

impl std::error::Error for Story {}

pub(crate) struct MsgRes {
    resolver: Option<tokio::sync::oneshot::Sender<Result<()>>>,
    create_time: tokio::time::Instant,
    story: Option<Story>,
}

impl Default for MsgRes {
    fn default() -> Self {
        Self {
            resolver: None,
            create_time: tokio::time::Instant::now(),
            story: Some(Story::default()),
        }
    }
}

impl Drop for MsgRes {
    fn drop(&mut self) {
        self.resolve(StoryLine::Dropped);
    }
}

impl MsgRes {
    pub fn new(
        resolver: Option<tokio::sync::oneshot::Sender<Result<()>>>,
    ) -> Self {
        Self {
            resolver,
            create_time: tokio::time::Instant::now(),
            story: Some(Story::default()),
        }
    }

    pub fn record(&mut self, story_line: StoryLine) {
        if let Some(story) = self.story.as_mut() {
            story.push(story_line);
        }
    }

    pub fn resolve(&mut self, story_line: StoryLine) {
        let story = self.story.take();
        if let Some(res) = self.resolver.take() {
            if let StoryLine::Success = story_line {
                let _ = res.send(Ok(()));
                return;
            }
            if let Some(mut story) = story {
                story.push(story_line);
                let _ = res.send(Err(other_err(story)));
            } else {
                let _ = res.send(Err(other_err(story_line)));
            }
        }
    }
}

pub(crate) struct Message<T: Tx3Transport> {
    pub remote:
        Arc<<<T as Tx3Transport>::Common as Tx3TransportCommon>::EndpointId>,
    pub content: BytesList,
    pub _permit: tokio::sync::OwnedSemaphorePermit,
    pub msg_res: MsgRes,
}

impl<T: Tx3Transport> Message<T> {
    pub fn new(
        remote: Arc<
            <<T as Tx3Transport>::Common as Tx3TransportCommon>::EndpointId,
        >,
        content: BytesList,
        permit: tokio::sync::OwnedSemaphorePermit,
        msg_res: Option<MsgRes>,
    ) -> Self {
        let msg_res = msg_res.unwrap_or_default();
        Self {
            remote,
            content,
            _permit: permit,
            msg_res,
        }
    }

    pub fn complete(
        self,
    ) -> (
        Arc<<<T as Tx3Transport>::Common as Tx3TransportCommon>::EndpointId>,
        BytesList,
    ) {
        let Self {
            remote,
            content,
            mut msg_res,
            ..
        } = self;
        msg_res.resolve(StoryLine::Success);
        (remote, content)
    }
}
