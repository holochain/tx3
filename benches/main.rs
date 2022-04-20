use criterion::*;
use parking_lot::Mutex;
use rw_stream_sink::*;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tx3::ws_framed::*;
use tx3::yamux_framed::*;

/// This control benchmark uses a single in-memory pseudo stream
/// and sends our 10 messages serially to provide a baseline for
/// comparing other solutions.
struct Control {
    data: Arc<[u8]>,
    send: tokio::io::DuplexStream,
    recv: tokio::io::DuplexStream,
}

impl Control {
    pub async fn new(data: Arc<[u8]>) -> Self {
        let (send, recv) = tokio::io::duplex(4096);
        Self { data, send, recv }
    }

    pub async fn test(self) -> Self {
        let Self {
            data,
            mut send,
            mut recv,
        } = self;

        let send_task = {
            let data = data.clone();
            tokio::task::spawn(async move {
                for _ in 0..10 {
                    send.write_all(&data).await.unwrap();
                    send.flush().await.unwrap();
                }
                send
            })
        };

        let recv_task = {
            let data = data.clone();
            tokio::task::spawn(async move {
                let mut buf = vec![0; data.len()];
                for _ in 0..10 {
                    recv.read_exact(&mut buf[..]).await.unwrap();
                    assert_eq!(buf.as_slice(), &*data);
                }
                recv
            })
        };

        let send = send_task.await.unwrap();
        let recv = recv_task.await.unwrap();

        Self { data, send, recv }
    }
}

/// Yamux muxer on top of tls secured websocket stream
struct YamuxWss {
    data: Arc<[u8]>,
    send: Arc<YamuxFramedSend>,
    _recv: YamuxFramedRecv,
    _send: YamuxFramedSend,
    recv: YamuxFramedRecv,
}

impl YamuxWss {
    pub async fn new(data: Arc<[u8]>) -> Self {
        let listen =
            tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let recv_addr = listen.local_addr().unwrap();
        let recv = tokio::task::spawn(async move {
            let (recv, _) = listen.accept().await.unwrap();
            recv
        });
        let send = tokio::net::TcpStream::connect(recv_addr).await.unwrap();
        let recv = recv.await.unwrap();

        let (tls_srv, _tls_cli) = tx3::tls::Config::default().build().unwrap();
        let (_tls_srv, tls_cli) = tx3::tls::Config::default().build().unwrap();

        let recv =
            tokio::task::spawn(
                async move { tls_srv.accept(recv).await.unwrap() },
            );
        let send = tls_cli.connect(send).await.unwrap();
        let recv = recv.await.unwrap();

        let recv = tokio::task::spawn(async move {
            RwStreamSink::new(WsFramed::accept(recv).await.unwrap())
        });
        let send = RwStreamSink::new(WsFramed::connect(send).await.unwrap());
        let recv = recv.await.unwrap();

        let (send, _recv) = yamux_framed_cli(send);
        let (_send, recv) = yamux_framed_srv(recv);

        Self {
            data,
            send: Arc::new(send),
            _recv,
            _send,
            recv,
        }
    }

    pub async fn test(self) -> Self {
        let Self {
            data,
            send,
            _recv,
            _send,
            mut recv,
        } = self;

        let mut all = Vec::new();
        for _ in 0..10 {
            let data = data.clone();
            let send = send.clone();
            all.push(tokio::task::spawn(async move {
                let data = data.to_vec();
                send.send(data).await.unwrap();
            }));
        }
        for _ in 0..10 {
            let got = recv.recv().await.unwrap().unwrap();
            assert_eq!(got.as_slice(), &*data);
        }
        for t in all {
            t.await.unwrap();
        }

        Self {
            data,
            send,
            _recv,
            _send,
            recv,
        }
    }
}

struct S2nQuic {
    data: Arc<[u8]>,
    send: s2n_quic::connection::Connection,
    recv: s2n_quic::connection::Connection,
}

impl S2nQuic {
    pub async fn new(data: Arc<[u8]>) -> Self {
        const ALPN: &[u8] = b"Tx3Bench/1";
        let (tls_srv, _tls_cli) =
            tx3::tls::Config::default().with_alpn(ALPN).build().unwrap();
        let (_tls_srv, tls_cli) =
            tx3::tls::Config::default().with_alpn(ALPN).build().unwrap();

        let tls_srv = tls_srv.to_s2n();
        let tls_cli = tls_cli.to_s2n();

        let mut server = s2n_quic::server::Server::builder()
            .with_tls(tls_srv)
            .unwrap()
            .with_io("127.0.0.1:0")
            .unwrap()
            .start()
            .unwrap();
        let addr = server.local_addr().unwrap();

        let client = s2n_quic::client::Client::builder()
            .with_tls(tls_cli)
            .unwrap()
            .with_io("127.0.0.1:0")
            .unwrap()
            .start()
            .unwrap();

        let recv =
            tokio::task::spawn(async move { server.accept().await.unwrap() });

        let connect =
            s2n_quic::client::Connect::new(addr).with_server_name("stub");

        let send = client.connect(connect).await.unwrap();

        let recv = recv.await.unwrap();

        Self { data, send, recv }
    }

    pub async fn test(self) -> Self {
        let Self {
            data,
            mut send,
            mut recv,
        } = self;

        let mut all = Vec::new();

        for _ in 0..10 {
            let data = data.clone();
            let mut send = send.open_send_stream().await.unwrap();
            all.push(tokio::task::spawn(async move {
                let bytes = bytes::Bytes::copy_from_slice(&*data);
                send.send(bytes).await.unwrap();
                send.close().await.unwrap();
            }));
        }

        for _ in 0..10 {
            let data = data.clone();
            let mut recv = recv.accept_receive_stream().await.unwrap().unwrap();
            all.push(tokio::task::spawn(async move {
                let mut got = Vec::with_capacity(data.len());
                tokio::io::AsyncReadExt::read_to_end(&mut recv, &mut got)
                    .await
                    .unwrap();
                assert_eq!(&got, &*data);
            }));
        }

        for t in all {
            t.await.unwrap();
        }

        Self { data, send, recv }
    }
}

fn criterion_benchmark(c: &mut Criterion) {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();

    static KB: usize = 1024;
    let size_list = [
        // should fit in one datagram even with some quic overhead
        60 * KB,
        // test with a larger amount of data
        200 * KB,
    ];

    macro_rules! add_test_group {
        ($n:ident, $t:ty) => {
            let mut $n = c.benchmark_group(stringify!($n));
            for size in size_list.iter() {
                // multiply by 10 because our bench cases send ten messages
                // (hopefully in parallel depending on the underlying solution)
                $n.throughput(Throughput::Bytes((*size as u64) * 10));
                $n.bench_with_input(
                    BenchmarkId::from_parameter(size),
                    size,
                    |b, &size| {
                        let mut data = vec![0xdb; size];

                        use ring::rand::SecureRandom;
                        ring::rand::SystemRandom::new()
                            .fill(&mut data)
                            .unwrap();
                        let data: Arc<[u8]> = data.into_boxed_slice().into();

                        let t = rt.block_on(<$t>::new(data));
                        let t = Arc::new(Mutex::new(Some(t)));
                        b.to_async(&rt).iter(|| async {
                            let inner = t.lock().take().unwrap();
                            let inner = inner.test().await;
                            *t.lock() = Some(inner);
                        })
                    },
                );
            }
            $n.finish();
        };
    }

    add_test_group!(yamux_wss, YamuxWss);
    add_test_group!(s2n_quic, S2nQuic);
    add_test_group!(control, Control);
}

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);
