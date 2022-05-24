use crate::relay::*;
use crate::*;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;

fn init_tracing() {
    let subscriber = tracing_subscriber::FmtSubscriber::builder()
        .with_env_filter(
            tracing_subscriber::filter::EnvFilter::from_default_env(),
        )
        .with_file(true)
        .with_line_number(true)
        .finish();
    let _ = tracing::subscriber::set_global_default(subscriber);
}

#[tokio::test(flavor = "multi_thread")]
async fn smoke_test_st() {
    init_tracing();

    let (node1, mut recv1) =
        Tx3Node::new(Tx3Config::default().with_bind("tx3:-/st/127.0.0.1:0/"))
            .await
            .unwrap();
    let addr1 = node1.local_addr().clone();
    tracing::info!(%addr1);

    let rtask = tokio::task::spawn(async move {
        let (accept, _addr) = recv1.recv().await.unwrap();
        let mut con = accept.accept().await.unwrap();
        let mut buf = [0; 5];
        con.read_exact(&mut buf[..]).await.unwrap();
        assert_eq!(b"hello", &buf[..]);
        con.write_all(b"world").await.unwrap();
    });

    let (node2, _) = Tx3Node::new(Tx3Config::default()).await.unwrap();

    let mut con = node2.connect(addr1).await.unwrap();
    con.write_all(b"hello").await.unwrap();
    let mut buf = [0; 5];
    con.read_exact(&mut buf[..]).await.unwrap();
    assert_eq!(b"world", &buf[..]);

    rtask.await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn smoke_test_rst() {
    init_tracing();

    let relay = Tx3Relay::new(
        Tx3RelayConfig::default().with_bind("tx3:-/rst/127.0.0.1:0/"),
    )
    .await
    .unwrap();
    let addr_r = relay.local_addr().clone();
    tracing::info!(%addr_r);

    let (node1, mut recv1) =
        Tx3Node::new(Tx3Config::default().with_bind(&addr_r))
            .await
            .unwrap();
    let addr1 = node1.local_addr().clone();
    tracing::info!(%addr1);

    let rtask = tokio::task::spawn(async move {
        let (accept, _addr) = recv1.recv().await.unwrap();
        let mut con = accept.accept().await.unwrap();
        let mut buf = [0; 5];
        con.read_exact(&mut buf[..]).await.unwrap();
        assert_eq!(b"hello", &buf[..]);
        con.write_all(b"world").await.unwrap();
    });

    let (node2, _) = Tx3Node::new(Tx3Config::default()).await.unwrap();
    let mut con = node2.connect(addr1).await.unwrap();
    con.write_all(b"hello").await.unwrap();
    let mut buf = [0; 5];
    con.read_exact(&mut buf[..]).await.unwrap();
    assert_eq!(b"world", &buf[..]);

    rtask.await.unwrap();
}
