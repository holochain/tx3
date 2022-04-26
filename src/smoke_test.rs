use crate::*;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;

#[tokio::test(flavor = "multi_thread")]
async fn smoke_test_st() {
    let (node1, mut recv1) =
        Tx3Node::new(Tx3Config::default().with_bind("tx3-st://127.0.0.1:0"))
            .await
            .unwrap();
    let addr1 = node1.local_addrs()[0].to_owned();

    let rtask = tokio::task::spawn(async move {
        let mut con = recv1.recv().await.unwrap().accept().await.unwrap();
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
