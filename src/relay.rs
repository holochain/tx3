use crate::tls::*;
use crate::*;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;

fn default_max_inbound_connections() -> u32 {
    20480
}

fn default_max_control_streams() -> u32 {
    320
}

fn default_max_control_streams_per_ip() -> u32 {
    4
}

fn default_max_relays_per_control() -> u32 {
    64
}

fn default_connection_timeout_ms() -> u32 {
    1000 * 20
}

/// A wrapper around Tx3Config with some additional parameters specific
/// to configuring a relay server.
#[non_exhaustive]
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tx3RelayConfig {
    /// The standard tx3 configuration parameters.
    #[serde(flatten)]
    pub tx3_config: Tx3Config,

    /// The maximum incoming connections that will be allowed. Any incoming
    /// connections beyond this limit will be dropped without response.
    /// The default value is 20480, which, while approaching as large as it
    /// can be, isn't that large... if every user has the default max of 64
    /// relays open, we can only support 320 users. It would be ideal to only
    /// use the following more specific limits, but we cannot know the stream
    /// type until reading from it, so we need this way to drop streams
    /// quicker immediately on accept().
    #[serde(default = "default_max_inbound_connections")]
    pub max_inbound_connections: u32,

    /// The maximum control streams we are willing to handle as a relay node.
    /// The default value is 320.
    /// 320 * 64 relays per control stream = 20480
    /// which is approaching the max ephemeral port range
    /// on many systems (32768–60999 = 28231).
    #[serde(default = "default_max_control_streams")]
    pub max_control_streams: u32,

    /// The maximum control streams we allow per remote ip address... Sorry
    /// folks behind campus or corporate NATs, this is the best we can do
    /// to at least require the effort of distributing a DDoS attack.
    /// But even with the small number of 4 here, it only takes 80 distributed
    /// nodes to lock down a relay server.
    /// The default value is 4.
    /// TODO - THIS IS NOT YET IMPLEMENTED
    #[serde(default = "default_max_control_streams_per_ip")]
    pub max_control_streams_per_ip: u32,

    /// The maximum relay streams allowed for each relay client (identified
    /// by the control stream). Clients should close least recently used
    /// connections before the open count reaches this number, or any new
    /// incoming connections will be dropped before being reported to the
    /// control stream.
    /// The default value is 64.
    /// TODO - THIS IS NOT YET IMPLEMENTED
    #[serde(default = "default_max_relays_per_control")]
    pub max_relays_per_control: u32,

    /// This timeout is applied to two different types of incoming
    /// streams. First, it requires control stream TLS negotiation to complete
    /// in this time period. Second, it requires initiated streams to be
    /// matched and spliced to a target within this time frame.
    /// The default value is 20 seconds (1000 * 20).
    #[serde(default = "default_connection_timeout_ms")]
    pub connection_timeout_ms: u32,
}

impl Default for Tx3RelayConfig {
    fn default() -> Self {
        Self {
            tx3_config: Tx3Config::default(),
            max_inbound_connections: default_max_inbound_connections(),
            max_control_streams: default_max_control_streams(),
            max_control_streams_per_ip: default_max_control_streams_per_ip(),
            max_relays_per_control: default_max_relays_per_control(),
            connection_timeout_ms: default_connection_timeout_ms(),
        }
    }
}

impl Tx3RelayConfig {
    /// Construct a new default Tx3RelayConfig
    pub fn new() -> Self {
        Tx3RelayConfig::default()
    }

    /// Append a bind to the list of bindings
    /// (shadow the deref builder function so we return ourselves)
    pub fn with_bind<B>(mut self, bind: B) -> Self
    where
        B: Into<Tx3Url>,
    {
        self.tx3_config.bind.push(bind.into());
        self
    }
}

impl std::ops::Deref for Tx3RelayConfig {
    type Target = Tx3Config;

    fn deref(&self) -> &Self::Target {
        &self.tx3_config
    }
}

impl std::ops::DerefMut for Tx3RelayConfig {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.tx3_config
    }
}

/// A tx3-rst relay server
pub struct Tx3Relay {
    config: Arc<Tx3RelayConfig>,
    addrs: Vec<Tx3Url>,
}

impl Tx3Relay {
    /// Construct/bind a new Tx3Relay server instance with given config
    pub async fn new(mut config: Tx3RelayConfig) -> Result<Self> {
        if config.tls.is_none() {
            config.tls = Some(TlsConfigBuilder::default().build()?);
        }

        let state = RelayStateSync::new();

        let mut all_bind = Vec::new();

        let to_bind = config.bind.drain(..).collect::<Vec<_>>();

        let config = Arc::new(config);

        let inbound_limit = Arc::new(tokio::sync::Semaphore::new(
            config.max_inbound_connections as usize,
        ));

        let control_limit = Arc::new(tokio::sync::Semaphore::new(
            config.max_control_streams as usize,
        ));

        for bind in to_bind {
            match bind.scheme() {
                Tx3Scheme::Tx3rst => {
                    for addr in bind.socket_addrs().await? {
                        all_bind.push(bind_tx3_rst(
                            config.clone(),
                            addr,
                            state.clone(),
                            inbound_limit.clone(),
                            control_limit.clone(),
                        ));
                    }
                }
                oth => {
                    return Err(other_err(format!(
                        "Unsupported Scheme: {}",
                        oth.as_str()
                    )))
                }
            }
        }

        let addrs = futures::future::try_join_all(all_bind)
            .await?
            .into_iter()
            .flatten()
            .collect();

        Ok(Self { config, addrs })
    }

    /// Get the local TLS certificate digest associated with this relay
    pub fn local_tls_cert_digest(&self) -> &TlsCertDigest {
        self.config.priv_tls().cert_digest()
    }

    /// Get our bound addresses, if any
    pub fn local_addrs(&self) -> &[Tx3Url] {
        &self.addrs
    }
}

async fn bind_tx3_rst(
    config: Arc<Tx3RelayConfig>,
    addr: SocketAddr,
    state: RelayStateSync,
    inbound_limit: Arc<tokio::sync::Semaphore>,
    control_limit: Arc<tokio::sync::Semaphore>,
) -> Result<Vec<Tx3Url>> {
    let listener = tokio::net::TcpListener::bind(addr).await?;
    let addr = listener.local_addr()?;
    let mut out = Vec::new();

    for a in upgrade_addr(addr)? {
        out.push(Tx3Url::new(
            url::Url::parse(&format!(
                "tx3-rst://{}/{}",
                a,
                tls_cert_digest_b64_enc(config.priv_tls().cert_digest()),
            ))
            .map_err(other_err)?,
        ));
    }

    tokio::task::spawn(async move {
        loop {
            match listener.accept().await {
                Err(err) => {
                    tracing::warn!(?err);
                }
                Ok((socket, _addr)) => {
                    let con_permit = match inbound_limit
                        .clone()
                        .try_acquire_owned()
                    {
                        Err(_) => {
                            tracing::warn!("Dropping incoming connection, max_inbound_connections reached");
                            continue;
                        }
                        Ok(con_permit) => con_permit,
                    };

                    let socket = match crate::tcp::tx3_tcp_configure(socket) {
                        Err(err) => {
                            tracing::warn!(?err);
                            continue;
                        }
                        Ok(socket) => socket,
                    };
                    tokio::task::spawn(process_socket(
                        config.clone(),
                        socket,
                        state.clone(),
                        con_permit,
                        control_limit.clone(),
                    ));
                }
            }
        }
    });

    Ok(out)
}

enum ControlCmd {
    NotifyPending(Arc<[u8; 32]>),
}

struct RelayState {
    control_channels:
        HashMap<TlsCertDigest, tokio::sync::mpsc::Sender<ControlCmd>>,
    pending_tokens: HashMap<Arc<[u8; 32]>, tokio::net::TcpStream>,
}

impl RelayState {
    fn new() -> Self {
        Self {
            control_channels: HashMap::new(),
            pending_tokens: HashMap::new(),
        }
    }
}

#[derive(Clone)]
struct RelayStateSync(Arc<parking_lot::Mutex<RelayState>>);

impl RelayStateSync {
    fn new() -> Self {
        Self(Arc::new(parking_lot::Mutex::new(RelayState::new())))
    }

    fn access<R, F>(&self, f: F) -> R
    where
        F: FnOnce(&mut RelayState) -> R,
    {
        let mut inner = self.0.lock();
        f(&mut *inner)
    }
}

async fn process_socket(
    config: Arc<Tx3RelayConfig>,
    socket: tokio::net::TcpStream,
    state: RelayStateSync,
    con_permit: tokio::sync::OwnedSemaphorePermit,
    control_limit: Arc<tokio::sync::Semaphore>,
) {
    if let Err(err) =
        process_socket_err(config, socket, state, con_permit, control_limit)
            .await
    {
        tracing::warn!(?err);
    }
}

async fn process_socket_err(
    config: Arc<Tx3RelayConfig>,
    mut socket: tokio::net::TcpStream,
    state: RelayStateSync,
    con_permit: tokio::sync::OwnedSemaphorePermit,
    control_limit: Arc<tokio::sync::Semaphore>,
) -> Result<()> {
    let timeout = tokio::time::Instant::now()
        + std::time::Duration::from_millis(config.connection_timeout_ms as u64);

    let this_cert = config.priv_tls().cert_digest().clone();

    // all rst connections must initiate by sending a 32 byte token
    let mut token = [0; 32];

    tokio::time::timeout_at(timeout, socket.read_exact(&mut token[..]))
        .await??;

    let token = Arc::new(token);

    if token == this_cert {
        let control_permit = match control_limit.try_acquire_owned() {
            Err(_) => {
                tracing::warn!(
                    "Dropping incoming connection, max_control_streams reached"
                );
                return Ok(());
            }
            Ok(control_permit) => control_permit,
        };

        // someone is talking to us directly, trying to open a relay
        // control channel
        return process_relay_control(
            config,
            socket,
            state,
            timeout,
            con_permit,
            control_permit,
        )
        .await;
    }

    let mut splice_token = [0; 32];
    use ring::rand::SecureRandom;
    ring::rand::SystemRandom::new()
        .fill(&mut splice_token[..])
        .map_err(|_| other_err("SystemRandomFailure"))?;
    let splice_token = Arc::new(splice_token);

    enum TokenRes {
        /// we host a control with this cert digest
        /// send the control a message about the incoming connection
        HaveControl(tokio::sync::mpsc::Sender<ControlCmd>),

        /// we have this connection token in our store,
        /// splice the connections together
        HaveConToken(tokio::net::TcpStream, tokio::net::TcpStream),

        /// we do not recognize this token, drop the socket
        Unknown,
    }

    let res = {
        let splice_token = splice_token.clone();
        state.access(move |state| {
            if let Some(snd) = state.control_channels.get(&token) {
                state.pending_tokens.insert(splice_token, socket);
                TokenRes::HaveControl(snd.clone())
            } else if let Some(socket2) = state.pending_tokens.remove(&token) {
                TokenRes::HaveConToken(socket, socket2)
            } else {
                drop(socket);
                TokenRes::Unknown
            }
        })
    };

    match res {
        TokenRes::Unknown => (), // do nothing / the socket has been dropped
        TokenRes::HaveControl(snd) => {
            // we stored the socket in the pending map

            // set up a task to remove the entry if it's not claimed in the
            // timeout
            {
                let state = state.clone();
                let splice_token = splice_token.clone();
                tokio::task::spawn(async move {
                    tokio::time::sleep_until(timeout).await;
                    state.access(move |state| {
                        state.pending_tokens.remove(&splice_token);
                    });
                });
            }

            // notify the control stream of the pending connection
            let _ = snd.send(ControlCmd::NotifyPending(splice_token)).await;
        }
        TokenRes::HaveConToken(mut socket, mut socket2) => {
            // splice the two sockets together
            // MAYBE: look into optimization on linux using
            //        https://man7.org/linux/man-pages/man2/splice.2.html
            // ALSO: how do we rate-limit this?
            tokio::io::copy_bidirectional(&mut socket, &mut socket2).await?;
        }
    }

    drop(con_permit);

    Ok(())
}

async fn process_relay_control(
    config: Arc<Tx3RelayConfig>,
    socket: tokio::net::TcpStream,
    state: RelayStateSync,
    timeout: tokio::time::Instant,
    con_permit: tokio::sync::OwnedSemaphorePermit,
    control_permit: tokio::sync::OwnedSemaphorePermit,
) -> Result<()> {
    let socket = tokio::time::timeout_at(
        timeout,
        Tx3Connection::priv_accept(config.priv_tls().clone(), socket),
    )
    .await??;

    let remote_cert = socket.remote_tls_cert_digest().clone();

    let (ctrl_send, ctrl_recv) = tokio::sync::mpsc::channel(1);

    {
        let remote_cert = remote_cert.clone();

        // from here on out, if we end, we need to remove the control_channel
        if let Some(_old) = state.access(move |state| {
            state
                .control_channels
                .insert(remote_cert.clone(), ctrl_send)
        }) {
            // if the old sender is dropped, the old control connection
            // loop will end as well
        }
    }

    let res =
        process_relay_control_inner(socket, state.clone(), ctrl_recv).await;

    state.access(|state| {
        state.control_channels.remove(&remote_cert);
    });

    drop(con_permit);
    drop(control_permit);

    res
}

async fn process_relay_control_inner(
    socket: Tx3Connection,
    _state: RelayStateSync,
    mut ctrl_recv: tokio::sync::mpsc::Receiver<ControlCmd>,
) -> Result<()> {
    let (mut read, mut write) = tokio::io::split(socket);

    tokio::select! {
        // read side - it is an error to send any data to the read side
        //             of a control connection
        _ = async move {
            let mut buf = [0];
            read.read_exact(&mut buf).await?;
            Result::Ok(())
        } => {
            Err(other_err("ControlReadData"))
        }

        // write side - write data to the control connection
        _ = async move {
            while let Some(cmd) = ctrl_recv.recv().await {
                match cmd {
                    ControlCmd::NotifyPending(splice_token) => {
                        write.write_all(&splice_token[..]).await?;
                    }
                }
            }
            Result::Ok(())
        } => {
            Ok(())
        }
    }
}
