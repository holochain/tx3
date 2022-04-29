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
async fn relay_test_max_inbound_connections() {
    init_tracing();

    let mut relay_config =
        Tx3RelayConfig::new().with_bind("tx3-rst://127.0.0.1:0");
    relay_config.max_inbound_connections = 1;

    let relay = Tx3Relay::new(relay_config).await.unwrap();
    let r_addr = relay.local_addrs()[0].to_owned();

    // make the first connection
    let node1 = Tx3Node::new(Tx3Config::new().with_bind(&r_addr))
        .await
        .unwrap();

    // the second connection should error
    assert!(Tx3Node::new(Tx3Config::new().with_bind(&r_addr))
        .await
        .is_err());

    // if we drop the first connection
    drop(node1);

    // give the system time to notify the socket is closed
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;

    // the third connection should be a success again
    assert!(Tx3Node::new(Tx3Config::new().with_bind(&r_addr))
        .await
        .is_ok());
}

/// this test should be identical to max_inbound_connections above,
/// it just is not as efficient from the relay server's perspective.
#[tokio::test(flavor = "multi_thread")]
async fn relay_test_max_control_streams() {
    init_tracing();

    let mut relay_config =
        Tx3RelayConfig::new().with_bind("tx3-rst://127.0.0.1:0");
    relay_config.max_control_streams = 1;

    let relay = Tx3Relay::new(relay_config).await.unwrap();
    let r_addr = relay.local_addrs()[0].to_owned();

    // make the first connection
    let node1 = Tx3Node::new(Tx3Config::new().with_bind(&r_addr))
        .await
        .unwrap();

    // the second connection should error
    assert!(Tx3Node::new(Tx3Config::new().with_bind(&r_addr))
        .await
        .is_err());

    // if we drop the first connection
    drop(node1);

    // give the system time to notify the socket is closed
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;

    // the third connection should be a success again
    assert!(Tx3Node::new(Tx3Config::new().with_bind(&r_addr))
        .await
        .is_ok());
}

/// this test should be identical to the above two tests,
/// since we're establishing our test connections from the same loopback dev
#[tokio::test(flavor = "multi_thread")]
async fn relay_test_max_control_streams_per_ip() {
    init_tracing();

    let mut relay_config =
        Tx3RelayConfig::new().with_bind("tx3-rst://127.0.0.1:0");
    relay_config.max_control_streams_per_ip = 1;

    let relay = Tx3Relay::new(relay_config).await.unwrap();
    let r_addr = relay.local_addrs()[0].to_owned();

    // make the first connection
    let node1 = Tx3Node::new(Tx3Config::new().with_bind(&r_addr))
        .await
        .unwrap();

    // the second connection should error
    assert!(Tx3Node::new(Tx3Config::new().with_bind(&r_addr))
        .await
        .is_err());

    // if we drop the first connection
    drop(node1);

    // give the system time to notify the socket is closed
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;

    // the third connection should be a success again
    assert!(Tx3Node::new(Tx3Config::new().with_bind(&r_addr))
        .await
        .is_ok());
}

/// this test is a little unique because we actually need to setup relay streams
#[tokio::test(flavor = "multi_thread")]
async fn relay_test_max_relays_per_control() {
    init_tracing();

    let mut relay_config =
        Tx3RelayConfig::new().with_bind("tx3-rst://127.0.0.1:0");
    relay_config.max_relays_per_control = 1;

    let relay = Tx3Relay::new(relay_config).await.unwrap();
    let r_addr = relay.local_addrs()[0].to_owned();

    // setup the main addressee
    let (main_ep, mut main_recv) =
        Tx3Node::new(Tx3Config::new().with_bind(&r_addr))
            .await
            .unwrap();
    let main_addr = main_ep.local_addrs()[0].to_owned();

    // handle incoming connections
    let r_task = tokio::task::spawn(async move {
        let mut all = Vec::new();

        for _ in 0..2 {
            let a = main_recv.recv().await.unwrap();
            all.push(tokio::task::spawn(async move {
                let mut socket = a.accept().await.unwrap();
                let mut got = [0; 5];
                socket.read_exact(&mut got).await.unwrap();
                assert_eq!(b"hello", &got[..]);
            }));
        }

        for t in all {
            t.await.unwrap();
        }
    });

    let (n1, _) = Tx3Node::new(Tx3Config::new()).await.unwrap();
    let mut c1 = n1.connect(&main_addr).await.unwrap();

    let (n2, _) = Tx3Node::new(Tx3Config::new()).await.unwrap();
    // c1 is still open, so n2 cannot create a second relay stream
    assert!(n2.connect(&main_addr).await.is_err());

    // but c1 still works
    c1.write_all(b"hello").await.unwrap();

    // and after we drop it
    drop(c1);
    drop(n1);

    // give the system time to notify the socket is closed
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;

    let (n3, _) = Tx3Node::new(Tx3Config::new()).await.unwrap();
    // we are able to successfully establish c3
    let mut c3 = n3.connect(&main_addr).await.unwrap();

    // and use it
    c3.write_all(b"hello").await.unwrap();

    // make sure we receive just c1 and c3 on the recv side
    r_task.await.unwrap();
}
