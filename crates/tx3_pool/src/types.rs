//! Types you'll need if you're implementing transport newtypes for a pool.

use crate::*;

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
        + PartialOrd
        + Hash;

    /// The backend system transport connection type.
    type Connection: 'static + Send + AsyncRead + AsyncWrite + Unpin;
}

/// Connector type.
pub trait Tx3TransportConnector<C: Tx3TransportCommon>:
    'static + Send + Sync
{
    /// A future returned by the "connect" function.
    type ConnectFut: 'static
        + Send
        + Future<Output = Result<C::Connection>>;

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
pub trait Tx3Transport {
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
    fn bind(self, path: Self::BindPath) -> Self::BindFut;
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
        if !data.is_empty() {
            self.0.push_back(data);
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
                cnt -= rem
            } else if rem > cnt {
                item.advance(cnt);
                self.0.push_front(item);
                return;
            }
        }
    }
}
