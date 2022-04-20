use crate::*;

#[tokio::test(flavor = "multi_thread")]
async fn smoke_test() {
    let (ep1, _recv1) = tx3_endpoint(Default::default()).await.unwrap();
    let addr1 = ep1.bound_urls().await.unwrap().remove(0);
    println!("ep1 addr: {}", addr1);

    let (ep2, _recv2) = tx3_endpoint(Default::default()).await.unwrap();
    let addr2 = ep2.bound_urls().await.unwrap().remove(0);
    println!("ep2 addr: {}", addr2);

    /*
    let Tx3Remote {
        cert_digest: con_cert_1,
        send: _,
        recv: _,
    } = ep1.connect(addr2).await.unwrap();
    println!("con cert1: {:?}", con_cert_1);
    */
}

/*
use crate::ws_framed::*;
use crate::*;
use std::net::SocketAddr;

async fn listener() -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::task::spawn(async move {
        while let Ok((con, _addr)) = listener.accept().await {
            handle_con_srv(con);
        }
    });
    addr
}

fn handle_con_srv(con: tokio::net::TcpStream) {
    tokio::task::spawn(async move {
        let (cert, pk) = tls::localhost_self_signed_tls_cert();
        let config = tls::simple_server_config(cert, pk);
        let con: tokio_rustls::TlsStream<tokio::net::TcpStream> =
            config.accept(con).await.unwrap().into();
        let con = WsFramed::accept(con).await.unwrap();
        let con = rw_stream_sink::RwStreamSink::new(con);
        let _con = yamux::Connection::new(
            con,
            Default::default(),
            yamux::Mode::Server,
        );
        //handle_tls_con(con);
    });
}

fn handle_con_cli(con: tokio::net::TcpStream) {
    tokio::task::spawn(async move {
        let config = tls::trusting_client_config();
        let name = "localhost".try_into().unwrap();
        let con: tokio_rustls::TlsStream<tokio::net::TcpStream> =
            config.connect(name, con).await.unwrap().into();
        let con = WsFramed::connect(con).await.unwrap();
        let con = rw_stream_sink::RwStreamSink::new(con);
        let _con = yamux::Connection::new(
            con,
            Default::default(),
            yamux::Mode::Client,
        );
        //handle_tls_con(con);
    });
}

/*
fn handle_tls_con(mut con: WsFramed<tokio_rustls::TlsStream<tokio::net::TcpStream>>) {
    tokio::task::spawn(async move {
        use futures::SinkExt;
        use futures::StreamExt;
        con.send("hello".into()).await.unwrap();
        con.flush().await.unwrap();
        let msg = con.next().await.unwrap();
        println!("got: {:?}", msg);
    });
}
*/

#[tokio::test(flavor = "multi_thread")]
async fn smoke_test() {
    let addr = listener().await;
    println!("connected: {:?}", addr);

    let con = tokio::net::TcpStream::connect(addr).await.unwrap();
    handle_con_cli(con);

    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
}
*/
