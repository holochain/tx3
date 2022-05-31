//! A tx3 `rst` relay server.
//!
//! Note: unless you are writing test code, rather than using this directly
//! you probably want to use the commandline binary executable `tx3-relay`.
//!
//! ### The tx3 relay protocol
//!
//! Tx3 relay functions via TLS secured control streams, and rendezvous TCP
//! splicing. A client (typically behind a NAT) who wishes to be addressable
//! establishes a TLS secured control stream to a known relay server. If the
//! relay server agrees to relay, a single "all-clear" byte is sent back in
//! response.
//!
//! ```text
//! client                 relay server
//! --+---                 --------+---
//!   | --- control open (TLS) --> |
//!   | <-- "all-clear" (TLS) ---- |
//! ```
//!
//! The client then takes the relay server's url, replaces its tls cert digest
//! with the client's own tls cert digest to publish as the client's url.
//!
//! A peer who wishes to contact the client opens a TCP connection to the
//! relay server, and forwards in plain-text the 32 byte certificate digest
//! of the target it wishes to connect to. The server generates a unique
//! 32 byte "splice token" to identify the incoming connection, and forwards
//! that splice token over the secure control channel to the target client.
//!
//! If the client wishes to accept the incoming connection, it opens a new
//! TCP connection to the relay server, and forwards that splice token.
//! The server splices the connections together, and the client and peer
//! proceed to handshake TLS over the resulting tunnelled connection.
//!
//! ```text
//! client                  relay server                       peer
//! --+---                  ------+-----                       --+-
//!   |                           | <-- tls digest over (TCP) -- |
//!   | <-- splice token (TLS) -- |                              |
//!   | -- splice token (TCP) --> |                              |
//!   |          relay server splices TCP connections            |
//!   | <------------------- TLS handshaking ------------------> |
//! ```
//!
//! When the relay server informs the client of an incoming connection,
//! it prefixes the splice token with the origin socket address.
//!
//! The full message is in the form:
//!
//! ```text
//! | IP               | PORT    | SPLICE_TOKEN                     |
//! |------------------|---------|----------------------------------|
//! | XXXXXXXXXXXXXXXX | XX      | XXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX |
//! | 16 bytes         | 2 bytes | 32 bytes                         |
//! ```
//!
//! - `IP` - 16 byte ipv6 or mapped ipv4 address
//! - `PORT` - 2 byte network byte-order port
//! - `SPLICE_TOKEN` - 32 byte splice token

use crate::tls::*;
use crate::*;
use std::collections::HashMap;
use std::net::IpAddr;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;

fn default_tls() -> TlsConfig {
    TlsConfigBuilder::default().build().unwrap()
}

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

fn is_false(v: &bool) -> bool {
    !*v
}

/// An interface binding specification for relay nodes.
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tx3RelayBindSpec {
    /// The interface socket address to bind.
    pub interface: std::net::SocketAddr,

    /// The global public host name (or address) to publish.
    /// This can be the same as the interface address if this server
    /// is directly bound to a global ip address. If this is an ip
    /// v6 address, it should include the square brackets ("[..]").
    pub host: String,

    /// The global public port to publish.
    pub port: u16,

    /// If set to false, this binding should be ignored.
    pub enabled: bool,

    /// Normally a relay will error if the public host binding is not
    /// globally addressable. For testing, you can disable this check.
    #[serde(default, skip_serializing_if = "is_false")]
    pub allow_non_global_host: bool,

    /// User notes about this binding.
    #[serde(default)]
    pub notes: Vec<String>,
}

impl From<([u8; 4], u16)> for Tx3RelayBindSpec {
    fn from(s: ([u8; 4], u16)) -> Self {
        let ip = s.0.into();
        Tx3RelayBindSpec {
            interface: std::net::SocketAddr::V4(std::net::SocketAddrV4::new(
                ip, s.1,
            )),
            host: ip.to_string(),
            port: s.1,
            enabled: true,
            allow_non_global_host: false,
            notes: Vec::new(),
        }
    }
}

impl From<([u8; 4], u16, bool)> for Tx3RelayBindSpec {
    fn from(s: ([u8; 4], u16, bool)) -> Self {
        let ip = s.0.into();
        Tx3RelayBindSpec {
            interface: std::net::SocketAddr::V4(std::net::SocketAddrV4::new(
                ip, s.1,
            )),
            host: ip.to_string(),
            port: s.1,
            enabled: true,
            allow_non_global_host: s.2,
            notes: Vec::new(),
        }
    }
}

impl From<([u8; 16], u16)> for Tx3RelayBindSpec {
    fn from(s: ([u8; 16], u16)) -> Self {
        let ip = s.0.into();
        Tx3RelayBindSpec {
            interface: std::net::SocketAddr::V6(std::net::SocketAddrV6::new(
                ip, s.1, 0, 0,
            )),
            host: format!("[{}]", ip),
            port: s.1,
            enabled: true,
            allow_non_global_host: false,
            notes: Vec::new(),
        }
    }
}

impl From<([u8; 16], u16, bool)> for Tx3RelayBindSpec {
    fn from(s: ([u8; 16], u16, bool)) -> Self {
        let ip = s.0.into();
        Tx3RelayBindSpec {
            interface: std::net::SocketAddr::V6(std::net::SocketAddrV6::new(
                ip, s.1, 0, 0,
            )),
            host: format!("[{}]", ip),
            port: s.1,
            enabled: true,
            allow_non_global_host: s.2,
            notes: Vec::new(),
        }
    }
}

/// A wrapper around Tx3Config with some additional parameters specific
/// to configuring a relay server.
#[non_exhaustive]
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tx3RelayConfig {
    /// Tls configuration to use for this tx3 relay node.
    #[serde(skip)]
    #[serde(default = "default_tls")]
    pub tls: TlsConfig,

    /// The interface addresses to bind for this relay node.
    /// In general, this should include exactly one ipv4 address
    /// and exactly one ipv6 address.
    #[serde(default)]
    pub bind: Vec<Tx3RelayBindSpec>,

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
    #[serde(default = "default_max_control_streams_per_ip")]
    pub max_control_streams_per_ip: u32,

    /// The maximum relay streams allowed for each relay client (identified
    /// by the control stream). Clients should close least recently used
    /// connections before the open count reaches this number, or any new
    /// incoming connections will be dropped before being reported to the
    /// control stream.
    /// The default value is 64.
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
            tls: default_tls(),
            bind: Vec::new(),
            max_inbound_connections: default_max_inbound_connections(),
            max_control_streams: default_max_control_streams(),
            max_control_streams_per_ip: default_max_control_streams_per_ip(),
            max_relays_per_control: default_max_relays_per_control(),
            connection_timeout_ms: default_connection_timeout_ms(),
        }
    }
}

impl Tx3RelayConfig {
    /// Push a bind spec into the list of bindings for this config.
    pub fn with_bind<B: Into<Tx3RelayBindSpec>>(mut self, bind: B) -> Self {
        self.bind.push(bind.into());
        self
    }
}

/// A tx3 `rst` relay server.
///
/// See module-level docs for additional details.
pub struct Tx3Relay {
    config: Arc<Tx3RelayConfig>,
    addr: Arc<Tx3Addr>,
    shutdown: Term,
}

impl Drop for Tx3Relay {
    fn drop(&mut self) {
        self.shutdown.term();
    }
}

impl Tx3Relay {
    /// Construct/bind a new Tx3Relay server instance with given config
    pub async fn new(mut config: Tx3RelayConfig) -> Result<Self> {
        let shutdown = Term::new("RelayShutdown", None);

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
            if !bind.enabled {
                continue;
            }
            all_bind.push(bind_tx3_rst(
                config.clone(),
                bind,
                state.clone(),
                inbound_limit.clone(),
                control_limit.clone(),
                shutdown.clone(),
            ));
        }

        let stack_list = futures::future::try_join_all(all_bind)
            .await?
            .into_iter()
            .collect();

        let addr = Arc::new(Tx3Addr {
            id: Some(config.tls.cert_digest().clone()),
            stack_list,
        });

        tracing::info!(%addr, "relay running");

        Ok(Self {
            config,
            addr,
            shutdown,
        })
    }

    /// Get the local TLS certificate digest associated with this relay
    pub fn local_id(&self) -> &Arc<Tx3Id> {
        self.config.tls.cert_digest()
    }

    /// Get our bound addresses, if any
    pub fn local_addr(&self) -> &Arc<Tx3Addr> {
        &self.addr
    }
}

async fn bind_tx3_rst(
    config: Arc<Tx3RelayConfig>,
    bind: Tx3RelayBindSpec,
    state: RelayStateSync,
    inbound_limit: Arc<tokio::sync::Semaphore>,
    control_limit: Arc<tokio::sync::Semaphore>,
    shutdown: Term,
) -> Result<Arc<Tx3Stack>> {
    let listener = tokio::net::TcpListener::bind(&bind.interface).await?;
    let mut port = bind.port;
    if port == 0 {
        port = listener.local_addr()?.port();
    }

    let out_addr = format!("{}:{}", &bind.host, port);
    if !bind.allow_non_global_host {
        for addr in tokio::net::lookup_host(&out_addr).await? {
            if !addr.ip().ext_is_global() {
                return Err(other_err(format!(
                    "{}({}) is not globally addressable, cannot bind. Configure port forwarding and adjust host string.",
                    out_addr,
                    addr,
                )));
            }
        }
    }

    let out = Arc::new(Tx3Stack::RelayTlsTcp(out_addr));

    shutdown.clone().spawn(async move {
        loop {
            match listener.accept().await {
                Err(err) => {
                    tracing::warn!(?err, "accept error");
                }
                Ok((socket, addr)) => {
                    tracing::trace!(?addr, "inbound connection");

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
                            tracing::warn!(?err, "tcp_configure error");
                            continue;
                        }
                        Ok(socket) => socket,
                    };

                    shutdown.spawn(process_socket(
                        config.clone(),
                        socket,
                        addr,
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
    NotifyPending(Arc<Tx3Id>, SocketAddr),
}

#[derive(Clone)]
struct ControlInfo {
    ctrl_send: tokio::sync::mpsc::Sender<ControlCmd>,
    relay_limit: Arc<tokio::sync::Semaphore>,
}

struct PendingInfo {
    socket: tokio::net::TcpStream,
    relay_permit: tokio::sync::OwnedSemaphorePermit,
}

struct RelayState {
    control_channels: HashMap<Arc<Tx3Id>, ControlInfo>,
    control_addrs: HashMap<IpAddr, u32>,
    pending_tokens: HashMap<Arc<Tx3Id>, PendingInfo>,
}

impl RelayState {
    fn new() -> Self {
        Self {
            control_channels: HashMap::new(),
            control_addrs: HashMap::new(),
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
    mut socket: tokio::net::TcpStream,
    rem_addr: SocketAddr,
    state: RelayStateSync,
    con_permit: tokio::sync::OwnedSemaphorePermit,
    control_limit: Arc<tokio::sync::Semaphore>,
) -> Result<()> {
    let timeout = tokio::time::Instant::now()
        + std::time::Duration::from_millis(config.connection_timeout_ms as u64);

    let this_cert = config.tls.cert_digest().clone();

    // all rst connections must initiate by sending a 32 byte token
    let mut token = [0; 32];

    tokio::time::timeout_at(timeout, socket.read_exact(&mut token[..]))
        .await??;

    let token = Arc::new(Tx3Id(token));

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
            rem_addr,
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
    let splice_token = Arc::new(Tx3Id(splice_token));

    enum TokenRes {
        /// we host a control with this cert digest
        /// send the control a message about the incoming connection
        HaveControl(tokio::sync::mpsc::Sender<ControlCmd>),

        /// we have this connection token in our store,
        /// splice the connections together
        HaveConToken(
            tokio::net::TcpStream,
            tokio::net::TcpStream,
            tokio::sync::OwnedSemaphorePermit,
        ),

        /// we should drop the socket
        Drop,
    }

    let res = {
        let splice_token = splice_token.clone();
        state.access(move |state| {
            if let Some(info) = state.control_channels.get(&token) {
                let relay_permit = match info.relay_limit.clone().try_acquire_owned() {
                    Err(_) => {
                        tracing::debug!("Dropping incoming connection, max_relays_per_control reached");
                        return TokenRes::Drop;
                    }
                    Ok(permit) => permit,
                };
                tracing::debug!(cert = ?token, splice = ?splice_token, "incoming pending relay");
                state.pending_tokens.insert(splice_token, PendingInfo {
                    socket,
                    relay_permit,
                });
                TokenRes::HaveControl(info.ctrl_send.clone())
            } else if let Some(info) = state.pending_tokens.remove(&token) {
                tracing::debug!(splice = ?token, "incoming relay fulfill");
                TokenRes::HaveConToken(socket, info.socket, info.relay_permit)
            } else {
                TokenRes::Drop
            }
        })
    };

    match res {
        TokenRes::Drop => (), // do nothing / the socket has been dropped
        TokenRes::HaveControl(ctrl_send) => {
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
            let _ = ctrl_send
                .send(ControlCmd::NotifyPending(splice_token, rem_addr))
                .await;
        }
        TokenRes::HaveConToken(mut socket, mut socket2, relay_permit) => {
            // we just need to hold this permit until this splice ends
            let _relay_permit = relay_permit;

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
    rem_addr: SocketAddr,
    state: RelayStateSync,
    timeout: tokio::time::Instant,
    con_permit: tokio::sync::OwnedSemaphorePermit,
    control_permit: tokio::sync::OwnedSemaphorePermit,
) -> Result<()> {
    let mut socket = tokio::time::timeout_at(
        timeout,
        Tx3Connection::priv_accept(config.tls.clone(), socket),
    )
    .await??;

    let remote_cert = socket.remote_id().clone();
    tracing::debug!(cert = ?remote_cert, "control stream established");

    let mut con_type = [0];
    tokio::time::timeout_at(timeout, socket.read_exact(&mut con_type))
        .await??;

    tracing::trace!(?con_type);

    if con_type[0] > 1 {
        return Err(other_err("InvalidConType"));
    }

    if con_type[0] == 1 {
        return Err(other_err("TodoAddressReflect"));
    }

    let (ctrl_send, ctrl_recv) = tokio::sync::mpsc::channel(1);

    {
        let remote_cert2 = remote_cert.clone();

        let relay_limit = Arc::new(tokio::sync::Semaphore::new(
            config.max_relays_per_control as usize,
        ));

        state.access(move |state| {
            {
                let ip_count = state.control_addrs.entry(rem_addr.ip()).or_default();
                if *ip_count < config.max_control_streams_per_ip {
                    *ip_count += 1;
                } else {
                    tracing::warn!("Dropping incoming connection, max_control_streams_per_ip reached");
                    // return the abort signal
                    return Err(other_err("ExceededMaxCtrlPerIp"));
                }
            }

            if let Some(_old) = state.control_channels.insert(
                remote_cert2.clone(),
                ControlInfo {
                    ctrl_send,
                    relay_limit,
                },
            ) {
                tracing::debug!(
                    cert = ?remote_cert2,
                    "replaced existing control stream",
                );
                // if the old sender is dropped, the old control connection
                // loop will end as well

                // we could check the ip and if it matches don't check the
                // per ip count here... but hopefully this situation doesn't
                // come up often enough for it to make any difference.
            }

            Ok(())
        })?;

        // from here on out, if we end, we need to remove the control_channel
    }

    let res =
        process_relay_control_inner(socket, state.clone(), ctrl_recv).await;

    state.access(|state| {
        if match state.control_addrs.get_mut(&rem_addr.ip()) {
            Some(ip_count) => {
                if *ip_count > 0 {
                    *ip_count -= 1;
                }
                *ip_count == 0
            }
            None => false,
        } {
            state.control_addrs.remove(&rem_addr.ip());
        }

        state.control_channels.remove(&remote_cert);
    });

    tracing::debug!(cert = ?remote_cert, "control stream ended");

    drop(con_permit);
    drop(control_permit);

    res
}

async fn process_relay_control_inner(
    mut socket: Tx3Connection,
    _state: RelayStateSync,
    mut ctrl_recv: tokio::sync::mpsc::Receiver<ControlCmd>,
) -> Result<()> {
    // first, send the "all-clear" byte.
    // without this, it's hard to wait for succesful control stream setup
    // from the client side.
    socket.write_all(&[0]).await?;
    socket.flush().await?;

    let (mut read, mut write) = tokio::io::split(socket);

    tokio::select! {
        // read side - it is an error to send any data to the read side
        //             of a control connection
        r = async move {
            let mut buf = [0];
            if read.read_exact(&mut buf).await.is_err() {
                // we're taking a read-side error as an indication
                // the control stream has been shutdown.
                Ok(())
            } else {
                // otherwise, we read some data from the stream,
                // which is an error.
                Err(other_err("ControlReadData"))
            }
        } => r,

        // write side - write data to the control connection
        _ = async move {
            while let Some(cmd) = ctrl_recv.recv().await {
                match cmd {
                    ControlCmd::NotifyPending(splice_token, rem_addr) => {
                        let ip = match rem_addr.ip() {
                            IpAddr::V4(a) => a.to_ipv6_mapped().octets(),
                            IpAddr::V6(a) => a.octets(),
                        };
                        let port = rem_addr.port();
                        let mut out = [0_u8; 16 + 2 + 32];
                        out[..16].copy_from_slice(&ip[..]);
                        out[16..18].copy_from_slice(&port.to_be_bytes());
                        out[18..].copy_from_slice(&splice_token[..]);
                        write.write_all(&out[..]).await?;
                    }
                }
            }
            Result::Ok(())
        } => {
            Ok(())
        }
    }
}
