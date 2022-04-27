use criterion::*;
use futures::future::try_join_all;
use std::sync::Arc;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tx3::*;

const COUNT: usize = 3;

/// This control benchmark uses raw tcp streams with no tls
/// to provide a baseline for comparing other solutions.
struct BenchControl {
    data: Arc<[u8]>,
    send: Vec<tokio::net::TcpStream>,
    recv: Vec<tokio::net::TcpStream>,
}

impl BenchControl {
    pub async fn new(data: Arc<[u8]>) -> Self {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        let recv = tokio::task::spawn(async move {
            let mut out = Vec::with_capacity(COUNT);
            for _ in 0..COUNT {
                let (s, _a) = l.accept().await.unwrap();
                out.push(s);
            }
            out
        });

        let mut send = Vec::with_capacity(COUNT);
        for _ in 0..COUNT {
            send.push(tokio::net::TcpStream::connect(addr).await.unwrap());
        }

        let recv = recv.await.unwrap();

        Self { data, send, recv }
    }

    pub async fn test(self) -> Self {
        let Self { data, send, recv } = self;

        let mut recv_tasks = Vec::with_capacity(COUNT);
        for mut recv in recv {
            let data = data.clone();
            recv_tasks.push(tokio::task::spawn(async move {
                let mut got = vec![0; data.len()];
                recv.read_exact(&mut got).await.unwrap();
                assert_eq!(got.as_slice(), &data[..]);
                recv
            }));
        }

        let mut send_tasks = Vec::with_capacity(COUNT);
        for mut send in send {
            let data = data.clone();
            send_tasks.push(tokio::task::spawn(async move {
                send.write_all(&data).await.unwrap();
                send.flush().await.unwrap();
                send
            }));
        }

        let recv = try_join_all(recv_tasks).await.unwrap();
        let send = try_join_all(send_tasks).await.unwrap();

        Self { data, send, recv }
    }
}

struct BenchTx3st {
    data: Arc<[u8]>,
    recv: Vec<Tx3Connection>,
    send: Vec<Tx3Connection>,
}

impl BenchTx3st {
    pub async fn new(data: Arc<[u8]>) -> Self {
        let (r_ep, mut recv) = Tx3Node::new(
            Tx3Config::default().with_bind("tx3-st://127.0.0.1:0"),
        )
        .await
        .unwrap();

        let addr = r_ep.local_addrs()[0].to_owned();

        let recv = tokio::task::spawn(async move {
            let mut out = Vec::with_capacity(COUNT);
            for _ in 0..COUNT {
                out.push(recv.recv().await.unwrap().accept().await.unwrap());
            }
            out
        });

        let (s_ep, _) = Tx3Node::new(Tx3Config::default()).await.unwrap();

        let mut send = Vec::with_capacity(COUNT);
        for _ in 0..COUNT {
            send.push(s_ep.connect(addr.clone()).await.unwrap());
        }

        let recv = recv.await.unwrap();

        Self { data, recv, send }
    }

    pub async fn test(self) -> Self {
        let Self { data, recv, send } = self;

        let mut recv_tasks = Vec::with_capacity(COUNT);
        for mut recv in recv {
            let data = data.clone();
            recv_tasks.push(tokio::task::spawn(async move {
                let mut got = vec![0; data.len()];
                recv.read_exact(&mut got).await.unwrap();
                assert_eq!(got.as_slice(), &data[..]);
                recv
            }));
        }

        let mut send_tasks = Vec::with_capacity(COUNT);
        for mut send in send {
            let data = data.clone();
            send_tasks.push(tokio::task::spawn(async move {
                send.write_all(&data).await.unwrap();
                send.flush().await.unwrap();
                send
            }));
        }

        let recv = try_join_all(recv_tasks).await.unwrap();
        let send = try_join_all(send_tasks).await.unwrap();

        Self { data, recv, send }
    }
}

struct BenchTx3rst {
    data: Arc<[u8]>,
    recv: Vec<Tx3Connection>,
    send: Vec<Tx3Connection>,
}

impl BenchTx3rst {
    pub async fn new(data: Arc<[u8]>) -> Self {
        let relay = Tx3Relay::new(
            Tx3RelayConfig::default().with_bind("tx3-rst://127.0.0.1:0"),
        )
        .await
        .unwrap();

        let r_addr = relay.local_addrs()[0].to_owned();

        let (r_ep, mut recv) =
            Tx3Node::new(Tx3Config::default().with_bind(r_addr))
                .await
                .unwrap();

        let addr = r_ep.local_addrs()[0].to_owned();

        let recv = tokio::task::spawn(async move {
            let mut out = Vec::with_capacity(COUNT);
            for _ in 0..COUNT {
                out.push(recv.recv().await.unwrap().accept().await.unwrap());
            }
            out
        });

        let (s_ep, _) = Tx3Node::new(Tx3Config::default()).await.unwrap();

        let mut send = Vec::with_capacity(COUNT);
        for _ in 0..COUNT {
            send.push(s_ep.connect(addr.clone()).await.unwrap());
        }

        let recv = recv.await.unwrap();

        Self { data, recv, send }
    }

    pub async fn test(self) -> Self {
        let Self { data, recv, send } = self;

        let mut recv_tasks = Vec::with_capacity(COUNT);
        for mut recv in recv {
            let data = data.clone();
            recv_tasks.push(tokio::task::spawn(async move {
                let mut got = vec![0; data.len()];
                recv.read_exact(&mut got).await.unwrap();
                assert_eq!(got.as_slice(), &data[..]);
                recv
            }));
        }

        let mut send_tasks = Vec::with_capacity(COUNT);
        for mut send in send {
            let data = data.clone();
            send_tasks.push(tokio::task::spawn(async move {
                send.write_all(&data).await.unwrap();
                send.flush().await.unwrap();
                send
            }));
        }

        let recv = try_join_all(recv_tasks).await.unwrap();
        let send = try_join_all(send_tasks).await.unwrap();

        Self { data, recv, send }
    }
}

fn criterion_benchmark(c: &mut Criterion) {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();

    static KB: usize = 1024;
    let size_list = [3 * KB, 7 * KB];

    macro_rules! add_test_group {
        ($n:ident, $t:ty) => {
            let mut $n = c.benchmark_group(stringify!($n));
            for size in size_list.iter() {
                // size / COUNT because our benches send paralel messages
                let mut data = vec![0xdb; size / COUNT];

                use ring::rand::SecureRandom;
                ring::rand::SystemRandom::new().fill(&mut data).unwrap();
                let data: Arc<[u8]> = data.into_boxed_slice().into();

                let t = rt.block_on(<$t>::new(data));
                let t = Arc::new(parking_lot::Mutex::new(Some(t)));

                $n.throughput(Throughput::Bytes((*size as u64)));
                $n.bench_with_input(
                    BenchmarkId::from_parameter(size),
                    size,
                    |b, _size| {
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

    add_test_group!(tx3rst, BenchTx3rst);
    add_test_group!(tx3st, BenchTx3st);
    add_test_group!(control, BenchControl);
}

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);
