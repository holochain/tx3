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
    recv: tokio::sync::mpsc::Receiver<Tx3InboundAccept>,
}

impl Tx3Inbound {
    /// receive the next inbound connection from this stream
    pub async fn recv(&mut self) -> Option<Tx3InboundAccept> {
        self.recv.recv().await
    }
}

impl futures::Stream for Tx3Inbound {
    type Item = Tx3InboundAccept;

    fn poll_next(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Self::Item>> {
        self.recv.poll_recv(cx)
    }
}

/// A Tx3 p2p communications node
pub struct Tx3Node {
    config: Arc<Tx3Config>,
    addrs: Vec<Tx3Url>,
}

impl Tx3Node {
    /// Construct/bind a new Tx3Node with given configuration
    pub async fn new(mut config: Tx3Config) -> Result<(Self, Tx3Inbound)> {
        use futures::future::FutureExt;

        if config.tls.is_none() {
            config.tls = Some(TlsConfigBuilder::default().build()?);
        }

        let (con_send, con_recv) = tokio::sync::mpsc::channel(1);

        let mut all_bind = Vec::new();

        let to_bind = config.bind.drain(..).collect::<Vec<_>>();

        let config = Arc::new(config);

        for bind in to_bind {
            match bind.scheme() {
                Tx3Scheme::Tx3st => {
                    for addr in bind.socket_addrs().await? {
                        all_bind.push(
                            bind_tx3_st(config.clone(), addr, con_send.clone())
                                .boxed(),
                        );
                    }
                }
                Tx3Scheme::Tx3rst => match bind.tls_cert_digest() {
                    None => return Err(other_err("InvalidRstCert")),
                    Some(cert_digest) => {
                        // we are connecting to a remote relay
                        let addrs = bind.socket_addrs().await?.collect();
                        all_bind.push(
                            bind_tx3_rst(
                                config.clone(),
                                bind.clone(),
                                addrs,
                                cert_digest,
                                con_send.clone(),
                            )
                            .boxed(),
                        );
                    }
                },
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

        Ok((Self { config, addrs }, Tx3Inbound { recv: con_recv }))
    }

    /// Get the local TLS certificate digest associated with this node
    pub fn local_cert(&self) -> &TlsCertDigest {
        self.config.priv_tls().cert_digest()
    }

    /// Get our bound addresses, if any
    pub fn local_addrs(&self) -> &[Tx3Url] {
        &self.addrs
    }

    /// Connect to a remote tx3 peer
    pub async fn connect(&self, peer: Tx3Url) -> Result<Tx3Connection> {
        match peer.scheme() {
            Tx3Scheme::Tx3st => {
                let mut errs = Vec::new();
                for addr in peer.socket_addrs().await? {
                    match connect_tx3_st(self.config.clone(), addr).await {
                        Err(e) => errs.push(e),
                        Ok(con) => return Ok(con),
                    }
                }
                Err(other_err(format!("{:?}", errs)))
            }
            Tx3Scheme::Tx3rst => {
                let tgt_cert_digest = match peer.tls_cert_digest() {
                    None => return Err(other_err("InvalidRstCert")),
                    Some(digest) => digest,
                };
                let mut errs = Vec::new();
                for addr in peer.socket_addrs().await? {
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
                Err(other_err(format!("{:?}", errs)))
            }
            oth => {
                Err(other_err(format!("Unsupported Scheme: {}", oth.as_str())))
            }
        }
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
    con_send: tokio::sync::mpsc::Sender<Tx3InboundAccept>,
) -> Result<Vec<Tx3Url>> {
    let listener = tokio::net::TcpListener::bind(addr).await?;
    let addr = listener.local_addr()?;
    let mut out = Vec::new();
    for a in upgrade_addr(addr)? {
        out.push(Tx3Url::new(
            url::Url::parse(&format!(
                "tx3-st://{}/{}",
                a,
                tls_cert_digest_b64_enc(config.priv_tls().cert_digest()),
            ))
            .map_err(other_err)?,
        ));
    }
    tokio::task::spawn(async move {
        loop {
            let ib_fut = match listener.accept().await {
                Err(e) => Tx3InboundAcceptInner::Err(e),
                Ok((socket, _addr)) => {
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

            if let Err(_) =
                con_send.send(Tx3InboundAccept { inner: ib_fut }).await
            {
                break;
            }
        }
    });
    Ok(out)
}

async fn bind_tx3_rst(
    config: Arc<Tx3Config>,
    relay_url: Tx3Url,
    addrs: Vec<SocketAddr>,
    tgt_cert_digest: TlsCertDigest,
    con_send: tokio::sync::mpsc::Sender<Tx3InboundAccept>,
) -> Result<Vec<Tx3Url>> {
    let mut errs = Vec::new();

    for addr in addrs {
        match bind_tx3_rst_inner(
            config.clone(),
            addr,
            tgt_cert_digest.clone(),
            con_send.clone(),
        )
        .await
        {
            Err(e) => errs.push(e),
            Ok(_) => {
                return Ok(vec![
                    relay_url.with_cert_digest(config.priv_tls().cert_digest())
                ]);
            }
        }
    }

    Err(other_err(format!("{:?}", errs)))
}

async fn bind_tx3_rst_inner(
    config: Arc<Tx3Config>,
    addr: SocketAddr,
    tgt_cert_digest: TlsCertDigest,
    con_send: tokio::sync::mpsc::Sender<Tx3InboundAccept>,
) -> Result<()> {
    let mut control_socket = crate::tcp::tx3_tcp_connect(addr).await?;
    control_socket.write_all(&tgt_cert_digest[..]).await?;
    let mut control_socket =
        Tx3Connection::priv_connect(config.priv_tls().clone(), control_socket)
            .await?;
    if control_socket.remote_cert() != &tgt_cert_digest {
        return Err(other_err("InvalidPeerCert"));
    }
    tokio::task::spawn(async move {
        let mut splice_token = [0; 32];
        loop {
            control_socket.read_exact(&mut splice_token).await?;
            let ib_fut = Tx3InboundAcceptInner::Tx3rst(Tx3InboundAcceptRst {
                tls: config.priv_tls().clone(),
                addr,
                splice_token: Arc::new(splice_token),
            });

            if let Err(_) =
                con_send.send(Tx3InboundAccept { inner: ib_fut }).await
            {
                break;
            }
        }

        Result::Ok(())
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
    tgt_cert_digest: TlsCertDigest,
) -> Result<Tx3Connection> {
    let mut socket = crate::tcp::tx3_tcp_connect(addr).await?;
    socket.write_all(&tgt_cert_digest[..]).await?;
    Tx3Connection::priv_connect(config.priv_tls().clone(), socket).await
}
