use crate::tls::*;
use crate::*;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::task::Context;
use std::task::Poll;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;

/// Resolves to a connection result. This abstraction allows optional parallel
/// processing of incoming connection handshakes.
#[must_use = "Tx3InboundAccept instances do nothing unless you call accept"]
pub struct Tx3InboundAccept {
    inner: Tx3InboundAcceptInner,
}

impl Tx3InboundAccept {
    /// Process this accept to completion
    pub async fn accept(self) -> Result<Tx3Connection> {
        match self.inner {
            Tx3InboundAcceptInner::Err(e) => Err(e),
            Tx3InboundAcceptInner::Tx3st(s) => s.accept().await,
            Tx3InboundAcceptInner::Tx3rst(s) => s.accept().await,
        }
    }
}

/// A stream of inbound Tx3Connections
pub struct Tx3Inbound {
    recv: tokio::sync::mpsc::Receiver<(Tx3InboundAccept, SocketAddr)>,
}

impl Tx3Inbound {
    /// Receive the next inbound connection from this stream.
    ///
    /// The `SocketAddr.ip()` may be `is_unspecified()` on some early
    /// error conditions.
    pub async fn recv(&mut self) -> Option<(Tx3InboundAccept, SocketAddr)> {
        self.recv.recv().await
    }
}

impl futures::Stream for Tx3Inbound {
    type Item = (Tx3InboundAccept, SocketAddr);

    fn poll_next(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Self::Item>> {
        self.recv.poll_recv(cx)
    }
}

/// A Tx3 p2p communications node
#[derive(Clone)]
pub struct Tx3Node {
    config: Arc<Tx3Config>,
    addr: Arc<Tx3Addr>,
    shutdown: Term,
}

impl Drop for Tx3Node {
    fn drop(&mut self) {
        self.shutdown.term();
    }
}

impl Tx3Node {
    /// Construct/bind a new Tx3Node with given configuration
    pub async fn new(mut config: Tx3Config) -> Result<(Self, Tx3Inbound)> {
        use futures::future::FutureExt;

        let shutdown = Term::new(None);

        if config.tls.is_none() {
            config.tls = Some(TlsConfigBuilder::default().build()?);
        }

        let (con_send, con_recv) = tokio::sync::mpsc::channel(1);

        let mut all_bind = Vec::new();

        let to_bind = config.bind.drain(..).collect::<Vec<_>>();

        let config = Arc::new(config);

        for bind in to_bind {
            for stack in bind.stack_list.iter() {
                match &**stack {
                    Tx3Stack::TlsTcp(addr) => {
                        for addr in tokio::net::lookup_host(addr)
                            .await
                            .map_err(other_err)?
                        {
                            all_bind.push(
                                bind_tx3_st(
                                    config.clone(),
                                    addr,
                                    con_send.clone(),
                                    shutdown.clone(),
                                )
                                .boxed(),
                            );
                        }
                    }
                    Tx3Stack::RelayTlsTcp(addr) => {
                        match &bind.id {
                            None => return Err(other_err("InvalidRstCert")),
                            Some(cert_digest) => {
                                // we are connecting to a remote relay
                                let addrs = tokio::net::lookup_host(addr)
                                    .await
                                    .map_err(other_err)?
                                    .collect();
                                all_bind.push(
                                    bind_tx3_rst(
                                        config.clone(),
                                        bind.clone(),
                                        addrs,
                                        cert_digest.clone(),
                                        con_send.clone(),
                                        shutdown.clone(),
                                    )
                                    .boxed(),
                                );
                            }
                        }
                    }
                    oth => {
                        return Err(other_err(format!(
                            "Unsupported Stack: {}",
                            oth,
                        )))
                    }
                }
            }
        }

        let stack_list = futures::future::try_join_all(all_bind)
            .await?
            .into_iter()
            .flatten()
            .collect();

        let addr = Arc::new(Tx3Addr {
            id: Some(config.priv_tls().cert_digest().clone()),
            stack_list,
        });

        Ok((
            Self {
                config,
                addr,
                shutdown,
            },
            Tx3Inbound { recv: con_recv },
        ))
    }

    /// Get the local TLS certificate digest associated with this node
    pub fn local_id(&self) -> &Arc<Tx3Id> {
        self.config.priv_tls().cert_digest()
    }

    /// Get our local bound addr.
    pub fn local_addr(&self) -> &Arc<Tx3Addr> {
        &self.addr
    }

    /// Connect to a remote tx3 peer
    pub async fn connect<A>(&self, peer: A) -> Result<Tx3Connection>
    where
        A: IntoAddr,
    {
        let peer = peer.into_addr();
        let mut errs = Vec::new();
        for stack in peer.stack_list.iter() {
            match &**stack {
                Tx3Stack::TlsTcp(addr) => {
                    for addr in tokio::net::lookup_host(addr)
                        .await
                        .map_err(other_err)?
                    {
                        match connect_tx3_st(self.config.clone(), addr).await {
                            Err(e) => errs.push(e),
                            Ok(con) => return Ok(con),
                        }
                    }
                }
                Tx3Stack::RelayTlsTcp(addr) => match &peer.id {
                    None => return Err(other_err("InvalidRstCert")),
                    Some(tgt_cert_digest) => {
                        for addr in tokio::net::lookup_host(addr)
                            .await
                            .map_err(other_err)?
                        {
                            match connect_tx3_rst(
                                self.config.clone(),
                                addr,
                                tgt_cert_digest.clone(),
                            )
                            .await
                            {
                                Err(e) => errs.push(e),
                                Ok(con) => return Ok(con),
                            }
                        }
                    }
                },
                oth => {
                    return Err(other_err(format!(
                        "Unsupported Stack: {}",
                        oth
                    )));
                }
            }
        }
        Err(other_err(format!("{:?}", errs)))
    }
}

enum Tx3InboundAcceptInner {
    Err(std::io::Error),
    Tx3st(Tx3InboundAcceptSt),
    Tx3rst(Tx3InboundAcceptRst),
}

struct Tx3InboundAcceptSt {
    tls: TlsConfig,
    socket: tokio::net::TcpStream,
}

impl Tx3InboundAcceptSt {
    async fn accept(self) -> Result<Tx3Connection> {
        Tx3Connection::priv_accept(self.tls, self.socket).await
    }
}

struct Tx3InboundAcceptRst {
    tls: TlsConfig,
    addr: SocketAddr,
    splice_token: Arc<[u8; 32]>,
}

impl Tx3InboundAcceptRst {
    async fn accept(self) -> Result<Tx3Connection> {
        let mut socket = crate::tcp::tx3_tcp_connect(self.addr).await?;
        socket.write_all(&self.splice_token[..]).await?;
        Tx3Connection::priv_accept(self.tls, socket).await
    }
}

async fn bind_tx3_st(
    config: Arc<Tx3Config>,
    addr: SocketAddr,
    con_send: tokio::sync::mpsc::Sender<(Tx3InboundAccept, SocketAddr)>,
    shutdown: Term,
) -> Result<Vec<Arc<Tx3Stack>>> {
    let listener = tokio::net::TcpListener::bind(addr).await?;
    let addr = listener.local_addr()?;
    let mut out = Vec::new();
    for a in upgrade_addr(addr)? {
        out.push(Arc::new(Tx3Stack::TlsTcp(a.to_string())));
    }
    tokio::task::spawn(async move {
        tokio::select! {
            _ = shutdown.on_term() => (),
            _ = async move {
                loop {
                    let res = listener.accept().await;
                    let mut rem_addr = SocketAddr::V4(
                        std::net::SocketAddrV4::new(
                            std::net::Ipv4Addr::new(0, 0, 0, 0),
                            0,
                        )
                    );

                    let ib_fut = match res {
                        Err(e) => Tx3InboundAcceptInner::Err(e),
                        Ok((socket, addr)) => {
                            rem_addr = addr;
                            match crate::tcp::tx3_tcp_configure(socket) {
                                Err(e) => Tx3InboundAcceptInner::Err(e),
                                Ok(socket) => {
                                    Tx3InboundAcceptInner::Tx3st(Tx3InboundAcceptSt {
                                        tls: config.priv_tls().clone(),
                                        socket,
                                    })
                                }
                            }
                        }
                    };

                    if con_send
                        .send((Tx3InboundAccept { inner: ib_fut }, rem_addr))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
            } => (),
        }
    });
    Ok(out)
}

async fn bind_tx3_rst(
    config: Arc<Tx3Config>,
    relay_addr: Arc<Tx3Addr>,
    addrs: Vec<SocketAddr>,
    tgt_cert_digest: Arc<Tx3Id>,
    con_send: tokio::sync::mpsc::Sender<(Tx3InboundAccept, SocketAddr)>,
    shutdown: Term,
) -> Result<Vec<Arc<Tx3Stack>>> {
    let mut errs = Vec::new();

    for addr in addrs {
        match bind_tx3_rst_inner(
            config.clone(),
            addr,
            tgt_cert_digest.clone(),
            con_send.clone(),
            shutdown.clone(),
        )
        .await
        {
            Err(e) => errs.push(e),
            Ok(_) => {
                let mut out = Vec::new();
                for stack in relay_addr.stack_list.iter() {
                    let (k, v) = stack.as_pair();
                    assert_eq!("rst", k);
                    out.push(Arc::new(Tx3Stack::RelayTlsTcp(v.to_string())));
                }
                return Ok(out);
            }
        }
    }

    Err(other_err(format!("{:?}", errs)))
}

async fn bind_tx3_rst_inner(
    config: Arc<Tx3Config>,
    addr: SocketAddr,
    tgt_cert_digest: Arc<Tx3Id>,
    con_send: tokio::sync::mpsc::Sender<(Tx3InboundAccept, SocketAddr)>,
    shutdown: Term,
) -> Result<()> {
    let mut control_socket = crate::tcp::tx3_tcp_connect(addr).await?;
    control_socket.write_all(&tgt_cert_digest[..]).await?;
    let mut control_socket =
        Tx3Connection::priv_connect(config.priv_tls().clone(), control_socket)
            .await?;
    if control_socket.remote_id() != &tgt_cert_digest {
        return Err(other_err("InvalidPeerCert"));
    }

    // read the "all-clear" byte before spawning the task
    let mut all_clear = [0];
    control_socket.read_exact(&mut all_clear).await?;

    tokio::task::spawn(async move {
        let mut buf = [0_u8; 16 + 2 + 32];
        tokio::select! {
            _ = shutdown.on_term() => Result::Ok(()),
            r = async move {
                loop {
                    control_socket.read_exact(&mut buf).await?;

                    let port = u16::from_be_bytes([buf[16], buf[17]]);

                    let rem_addr = match &buf[..16] {
                        [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xff, 0xff, a, b, c, d] => {
                            std::net::SocketAddr::V4(
                                std::net::SocketAddrV4::new(
                                    std::net::Ipv4Addr::new(*a, *b, *c, *d),
                                    port,
                                )
                            )
                        }
                        [a, b, c, d, e, f, g, h, i, j, k, l, m, n, o, p] => {
                            std::net::SocketAddr::V6(
                                std::net::SocketAddrV6::new(
                                    std::net::Ipv6Addr::from([*a, *b, *c, *d, *e, *f, *g, *h, *i, *j, *k, *l, *m, *n, *o, *p]),
                                    port,
                                    0,
                                    0,
                                )
                            )
                        }
                        _ => unreachable!(),
                    };

                    tracing::trace!(?rem_addr, "accept incoming");

                    let mut splice_token = [0; 32];
                    splice_token.copy_from_slice(&buf[18..]);

                    let ib_fut = Tx3InboundAcceptInner::Tx3rst(Tx3InboundAcceptRst {
                        tls: config.priv_tls().clone(),
                        addr,
                        splice_token: Arc::new(splice_token),
                    });

                    if con_send
                        .send((
                            Tx3InboundAccept { inner: ib_fut },
                            rem_addr,
                        ))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }

                Result::Ok(())
            } => r,
        }
    });

    Ok(())
}

async fn connect_tx3_st(
    config: Arc<Tx3Config>,
    addr: SocketAddr,
) -> Result<Tx3Connection> {
    let socket = crate::tcp::tx3_tcp_connect(addr).await?;
    Tx3Connection::priv_connect(config.priv_tls().clone(), socket).await
}

async fn connect_tx3_rst(
    config: Arc<Tx3Config>,
    addr: SocketAddr,
    tgt_cert_digest: Arc<Tx3Id>,
) -> Result<Tx3Connection> {
    let mut socket = crate::tcp::tx3_tcp_connect(addr).await?;
    socket.write_all(&tgt_cert_digest[..]).await?;
    Tx3Connection::priv_connect(config.priv_tls().clone(), socket).await
}
