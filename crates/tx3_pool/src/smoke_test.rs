use crate::*;
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

#[tokio::test(flavor = "multi_thread")]
async fn smoke_test() {
    init_tracing();

    let (pool1, _recv1) = Tx3Pool::new(
        Tx3PoolConfig::default(),
        Arc::new(Tx3PoolImpDefault::default()),
    )
    .await
    .unwrap();

    tracing::info!(addr = ?pool1.local_addr());

    let bind_hnd = pool1.bind("tx3:-/st/127.0.0.1:0/").await.unwrap();

    tracing::info!(addr = ?pool1.local_addr());

    bind_hnd.terminate();

    tracing::info!(addr = ?pool1.local_addr());

    pool1.terminate();
}
