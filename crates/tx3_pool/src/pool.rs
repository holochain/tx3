use crate::types::*;
use crate::*;

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
    _p: std::marker::PhantomData<T>,
}

impl<T: Tx3Transport> Tx3PoolIncoming<T> {
    /// Pull the next incoming message from the receive stream.
    pub async fn recv(
        &mut self,
    ) -> Option<(
        Arc<<<T as Tx3Transport>::Common as Tx3TransportCommon>::EndpointId>,
        BytesList,
    )> {
        todo!()
    }
}

/// Tx3 pool.
pub struct Tx3Pool<T: Tx3Transport> {
    _p: std::marker::PhantomData<T>,
}

impl<T: Tx3Transport> Tx3Pool<T> {
    /// Bind a new transport backend, wrapping it in Tx3Pool logic.
    pub async fn bind(
        transport: T,
        path: T::BindPath,
    ) -> Result<(T::BindAppData, Self, Tx3PoolIncoming<T>)> {
        let (app, _connector, _acceptor) = transport.bind(path).await?;
        let this = Tx3Pool {
            _p: std::marker::PhantomData,
        };
        let incoming = Tx3PoolIncoming {
            _p: std::marker::PhantomData,
        };
        Ok((app, this, incoming))
    }

    /// Attempt to send a framed message to a target node.
    ///
    /// This can experience backpressure in two ways:
    /// - There is no active connection and we are at our max connection count,
    ///   and are unable to close any per ongoing activity.
    /// - The backpressure of sending to the outgoing transport connection.
    ///
    /// This call can timeout per the timeout specified in Tx3PoolConfig.
    ///
    /// This future will resolve Ok() when all data has been offloaded to
    /// the underlying transport
    pub async fn send<B: Into<BytesList>>(
        &self,
        dst: <<T as Tx3Transport>::Common as Tx3TransportCommon>::EndpointId,
        data: B,
    ) -> Result<()> {
        let mut data = data.into();
        let rem = data.remaining();
        if rem > FI_LEN_MASK as usize {
            return Err(other_err("MsgTooLarge"));
        }
        data.0.push_front(bytes::Bytes::copy_from_slice(
            &(rem as u32).to_le_bytes()[..])
        );
        let (_msg, resolve) = <Message<T>>::new(dst, data);
        // because of the Drop impl on msg, this resolve should never err
        resolve.await.unwrap()
    }

    /// Immediately terminate all connections, stopping all processing, both
    /// incoming and outgoing. This is NOT a graceful shutdown, and probably
    /// should only be used in testing scenarios.
    pub async fn terminate() {
        todo!()
    }
}

#[derive(Debug)]
enum StoryLine {
    Dropped,
}

#[derive(Default, Debug)]
struct Story(Vec<StoryLine>);

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

struct Message<T: Tx3Transport> {
    dst: <<T as Tx3Transport>::Common as Tx3TransportCommon>::EndpointId,
    content: BytesList,
    story: Option<Story>,
    resolver: Option<tokio::sync::oneshot::Sender<Result<()>>>,
}

impl<T: Tx3Transport> Drop for Message<T> {
    fn drop(&mut self) {
        self.resolve(StoryLine::Dropped);
    }
}

impl<T: Tx3Transport> Message<T> {
    fn new(
        dst: <<T as Tx3Transport>::Common as Tx3TransportCommon>::EndpointId,
        content: BytesList,
    ) -> (Self, tokio::sync::oneshot::Receiver<Result<()>>) {
        let (s, r) = tokio::sync::oneshot::channel();
        (
            Self {
                dst,
                content,
                story: Some(Story::default()),
                resolver: Some(s),
            },
            r,
        )
    }

    fn resolve(&mut self, story_line: StoryLine) {
        if let Some(res) = self.resolver.take() {
            let mut story = self.story.take().unwrap();
            story.push(story_line);
            let _ = res.send(Err(other_err(story)));
        }
    }
}
