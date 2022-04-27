use crate::tls::*;
use crate::*;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;

/// A tx3-rst relay server
pub struct Tx3Relay {
    config: Arc<Tx3Config>,
    addrs: Vec<Tx3Url>,
}

impl Tx3Relay {
    /// Construct/bind a new Tx3Relay server instance with given config
    pub async fn new(mut config: Tx3Config) -> Result<Self> {
        if config.tls.is_none() {
            config.tls = Some(TlsConfigBuilder::default().build()?);
        }

        let state = RelayStateSync::new();

        let mut all_bind = Vec::new();

        let to_bind = config.bind.drain(..).collect::<Vec<_>>();

        let config = Arc::new(config);

        for bind in to_bind {
            match bind.scheme() {
                Tx3Scheme::Tx3rst => {
                    for addr in bind.socket_addrs().await? {
                        all_bind.push(bind_tx3_rst(
                            config.clone(),
                            addr,
                            state.clone(),
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
    config: Arc<Tx3Config>,
    addr: SocketAddr,
    state: RelayStateSync,
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
                Err(_err) => {
                    // TODO FIXME TRACING
                }
                Ok((socket, _addr)) => {
                    let socket = match crate::tcp::tx3_tcp_configure(socket) {
                        Err(_err) => {
                            // TODO FIXME TRACING
                            continue;
                        }
                        Ok(socket) => socket,
                    };
                    tokio::task::spawn(process_socket(
                        config.clone(),
                        socket,
                        state.clone(),
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
    config: Arc<Tx3Config>,
    socket: tokio::net::TcpStream,
    state: RelayStateSync,
) {
    if let Err(_err) = process_socket_err(config, socket, state).await {
        // TODO FIXME TRACING
    }
}

async fn process_socket_err(
    config: Arc<Tx3Config>,
    mut socket: tokio::net::TcpStream,
    state: RelayStateSync,
) -> Result<()> {
    let this_cert = config.priv_tls().cert_digest().clone();

    // all rst connections must initiate by sending a 32 byte token
    let mut token = [0; 32];

    // TODO - add a timeout here
    socket.read_exact(&mut token[..]).await?;

    let token = Arc::new(token);

    if token == this_cert {
        // someone is talking to us directly, trying to open a relay
        // control channel
        return process_relay_control(config, socket, state).await;
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
            // notify the control stream of the pending connection
            let _ = snd.send(ControlCmd::NotifyPending(splice_token)).await;
        }
        TokenRes::HaveConToken(mut socket, mut socket2) => {
            // splice the two sockets together
            // MAYBE: look into optimization on linux using
            //        https://man7.org/linux/man-pages/man2/splice.2.html
            tokio::io::copy_bidirectional(&mut socket, &mut socket2).await?;
        }
    }

    Ok(())
}

async fn process_relay_control(
    config: Arc<Tx3Config>,
    socket: tokio::net::TcpStream,
    state: RelayStateSync,
) -> Result<()> {
    let socket =
        Tx3Connection::priv_accept(config.priv_tls().clone(), socket).await?;

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
