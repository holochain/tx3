use crate::types::*;
use crate::*;
use std::net::SocketAddr;
use std::sync::Arc;

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

#[derive(Default)]
pub struct MyHooks;

impl PoolHooks for MyHooks {
    type ConnectPreFut = futures::future::Ready<bool>;
    type AcceptAddrFut = futures::future::Ready<bool>;
    type AcceptIdFut = futures::future::Ready<bool>;

    fn addr_update(&self, addr: Arc<Tx3Addr>) {
        tracing::warn!(?addr, "MyHooks::addr_update");
    }

    fn connect_pre(&self, addr: Arc<Tx3Addr>) -> Self::ConnectPreFut {
        tracing::warn!(?addr, "MyHooks::connect_pre");
        futures::future::ready(true)
    }

    fn accept_addr(&self, addr: SocketAddr) -> Self::AcceptAddrFut {
        tracing::warn!(?addr, "MyHooks::accept_addr");
        futures::future::ready(true)
    }

    fn accept_id(&self, id: Arc<Tx3Id>) -> Self::AcceptAddrFut {
        tracing::warn!(?id, "MyHooks::accept_id");
        futures::future::ready(true)
    }
}

#[derive(Default)]
pub struct MyImp {
    addr_store: Arc<AddrStoreMem>,
    pool_hooks: Arc<MyHooks>,
}

impl Tx3PoolImp for MyImp {
    type PoolHooks = MyHooks;
    type AddrStore = AddrStoreMem;

    fn get_addr_store(&self) -> &Arc<Self::AddrStore> {
        &self.addr_store
    }

    fn get_pool_hooks(&self) -> &Arc<Self::PoolHooks> {
        &self.pool_hooks
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn smoke_test() {
    init_tracing();

    let mut config = Tx3PoolConfig::default();
    config.con_tgt_time = std::time::Duration::from_secs(1);

    let (pool1, mut recv1) = Tx3Pool::new(config, Arc::new(MyImp::default()))
        .await
        .unwrap();

    let r_task = tokio::task::spawn(async move {
        use bytes::Buf;

        for _ in 0..4 {
            let (id, msg) = recv1.recv().await.unwrap();
            tracing::warn!(?id, byte_count = ?msg.remaining(), "GOT MSG");
            assert_eq!(msg.to_vec().as_slice(), b"hello");
        }
    });

    pool1.bind("tx3:-/st/127.0.0.1:0/").await.unwrap();
    let addr1 = pool1.local_addr().clone();

    let mut config = Tx3PoolConfig::default();
    config.con_tgt_time = std::time::Duration::from_secs(1);

    let (pool2, _) = Tx3Pool::new(config, Arc::new(MyImp::default()))
        .await
        .unwrap();

    pool2.as_imp().get_addr_store().set(&addr1);

    for _ in 0..4 {
        pool2
            .send(
                addr1.id.as_ref().unwrap().clone(),
                b"hello",
                std::time::Duration::from_secs(10),
            )
            .await
            .unwrap();

        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }

    r_task.await.unwrap();

    pool1.terminate();
}
