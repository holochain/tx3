use crate::*;
use std::net::SocketAddr;

pub(crate) async fn tx3_tcp_connect(
    addr: SocketAddr,
) -> Result<tokio::net::TcpStream> {
    let socket = tokio::net::TcpStream::connect(addr).await?;
    tx3_tcp_configure(socket)
}

pub(crate) fn tx3_tcp_configure(
    socket: tokio::net::TcpStream,
) -> Result<tokio::net::TcpStream> {
    let socket = socket.into_std()?;
    let socket = socket2::Socket::from(socket);

    let keepalive = socket2::TcpKeepalive::new()
        .with_time(std::time::Duration::from_secs(7))
        .with_interval(std::time::Duration::from_secs(7));

    // we'll close unresponsive connections after 21-28 seconds (7 * 3)
    // (it's a little unclear how long it'll wait after the final probe)
    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    let keepalive = keepalive.with_retries(3);

    socket.set_tcp_keepalive(&keepalive)?;

    let socket = std::net::TcpStream::from(socket);
    tokio::net::TcpStream::from_std(socket)
}
