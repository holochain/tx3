use crate::*;
//use crate::framed::*;
//use crate::ws_stream::*;
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
        let (cert, pk) = dev_utils::localhost_self_signed_tls_cert();
        let config = dev_utils::simple_server_config(cert, pk);
        let _con: tokio_rustls::TlsStream<tokio::net::TcpStream> =
            config.accept(con).await.unwrap().into();
        //let con = tokio_tungstenite::accept_async(con).await.unwrap();
        //let _con = WsStream::accept(con);
        //let con = WsFramed::new(con);
        //handle_tls_con(con);
    });
}

fn handle_con_cli(con: tokio::net::TcpStream) {
    tokio::task::spawn(async move {
        let config = dev_utils::trusting_client_config();
        let name = "localhost".try_into().unwrap();
        let _con: tokio_rustls::TlsStream<tokio::net::TcpStream> =
            config.connect(name, con).await.unwrap().into();
        /*
        let (con, _) = tokio_tungstenite::client_async("wss://localhost", con)
            .await
            .unwrap();
        */
        //let _con = WsStream::connect(con);
        //let con = WsFramed::new(con);
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
