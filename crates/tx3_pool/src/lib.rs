#![deny(missing_docs)]
#![deny(warnings)]
#![deny(unsafe_code)]
//! Tx3 p2p generic connection pool management.
//!
//! [![Project](https://img.shields.io/badge/project-holochain-blue.svg?style=flat-square)](http://holochain.org/)
//! [![Forum](https://img.shields.io/badge/chat-forum%2eholochain%2enet-blue.svg?style=flat-square)](https://forum.holochain.org)
//! [![Chat](https://img.shields.io/badge/chat-chat%2eholochain%2enet-blue.svg?style=flat-square)](https://chat.holochain.org)
//!
//! [![License: Apache-2.0](https://img.shields.io/badge/License-Apache%202.0-blue.svg)](https://www.apache.org/licenses/LICENSE-2.0)
//!
//! This library is a bit of a swiss army blade for p2p connection management.
//!
//! - Message framing - converts streaming channels into framed send/recv
//! - Connection pool - pool manages connection open/close
//! - TODO - connection rate limiting on both outgoing and incoming data
//! - TODO - message multiplexing to mitigate head-of-line blocking

use bytes::Buf;
use futures::stream::Stream;
use std::fmt::Debug;
use std::future::Future;
use std::hash::Hash;
use std::io::Result;
use std::sync::Arc;
use tokio::io::AsyncRead;
use tokio::io::AsyncWrite;

pub mod types;

mod pool_con;
pub(crate) use pool_con::*;

mod pool;
pub use pool::*;

#[cfg(test)]
mod tests {
    use super::*;
    use parking_lot::Mutex;
    use std::collections::HashMap;
    use std::pin::Pin;
    use std::task::Context;
    use std::task::Poll;
    use types::*;

    struct MemListen {
        bind: &'static str,
        recv: tokio::sync::mpsc::Receiver<(
            &'static str,
            tokio::io::DuplexStream,
        )>,
    }

    impl Drop for MemListen {
        fn drop(&mut self) {
            mem_access(|inner| {
                inner.bind.remove(self.bind);
            });
        }
    }

    struct MemInner {
        bind: HashMap<
            &'static str,
            tokio::sync::mpsc::Sender<(&'static str, tokio::io::DuplexStream)>,
        >,
    }

    static MEM: Mutex<Option<MemInner>> = parking_lot::const_mutex(None);

    fn mem_access<R, F>(f: F) -> R
    where
        F: FnOnce(&mut MemInner) -> R,
    {
        let mut inner = MEM.lock();
        if inner.is_none() {
            *inner = Some(MemInner {
                bind: HashMap::new(),
            })
        }
        f(inner.as_mut().unwrap())
    }

    async fn mem_connect(
        src: &'static str,
        dst: &'static str,
    ) -> Result<tokio::io::DuplexStream> {
        let snd = mem_access(move |inner| match inner.bind.get(dst) {
            None => Result::Err(std::io::ErrorKind::ConnectionRefused.into()),
            Some(snd) => Result::Ok(snd.clone()),
        })?;
        let (one, two) = tokio::io::duplex(4096);
        snd.send((src, one)).await.map_err(|_| {
            std::io::Error::from(std::io::ErrorKind::ConnectionReset)
        })?;
        Ok(two)
    }

    fn mem_bind(bind: &'static str) -> Result<MemListen> {
        let (s, r) = tokio::sync::mpsc::channel(32);
        mem_access(move |inner| match inner.bind.entry(bind) {
            std::collections::hash_map::Entry::Occupied(_) => {
                Result::Err(std::io::ErrorKind::AddrInUse.into())
            }
            std::collections::hash_map::Entry::Vacant(e) => {
                e.insert(s);
                Result::Ok(())
            }
        })?;
        Ok(MemListen { bind, recv: r })
    }

    struct Common;
    impl Tx3TransportCommon for Common {
        type EndpointId = &'static str;
        type Connection = tokio::io::DuplexStream;
    }

    struct Connector(Arc<&'static str>);
    impl Tx3TransportConnector<Common> for Connector {
        type ConnectFut = Pin<
            Box<
                dyn Future<
                        Output = Result<
                            <Common as Tx3TransportCommon>::Connection,
                        >,
                    >
                    + 'static
                    + Send,
            >,
        >;

        fn connect(
            &self,
            id: Arc<<Common as Tx3TransportCommon>::EndpointId>,
        ) -> Self::ConnectFut {
            let src = self.0.clone();
            Box::pin(async move { mem_connect(*src, *id).await })
        }
    }

    struct Acceptor(MemListen);
    impl Stream for Acceptor {
        type Item = <Acceptor as Tx3TransportAcceptor<Common>>::AcceptFut;
        fn poll_next(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
        ) -> Poll<Option<Self::Item>> {
            match self.0.recv.poll_recv(cx) {
                Poll::Pending => Poll::Pending,
                Poll::Ready(None) => Poll::Ready(None),
                Poll::Ready(Some((bind, stream))) => {
                    Poll::Ready(Some(Box::pin(async move {
                        Ok((Arc::new(bind), stream))
                    })))
                }
            }
        }
    }
    impl Tx3TransportAcceptor<Common> for Acceptor {
        type AcceptFut = Pin<
            Box<
                dyn Future<
                        Output = Result<(
                            Arc<<Common as Tx3TransportCommon>::EndpointId>,
                            <Common as Tx3TransportCommon>::Connection,
                        )>,
                    >
                    + 'static
                    + Send,
            >,
        >;
    }

    struct Transport;
    impl Tx3Transport for Transport {
        type Common = Common;
        type Connector = Connector;
        type Acceptor = Acceptor;
        type BindPath = &'static str;
        type BindAppData = ();
        type BindFut = Pin<
            Box<
                dyn Future<
                        Output = Result<(
                            Self::BindAppData,
                            Self::Connector,
                            Self::Acceptor,
                        )>,
                    >
                    + 'static
                    + Send,
            >,
        >;
        fn bind(self, path: Arc<Self::BindPath>) -> Self::BindFut {
            Box::pin(async move {
                let listen = mem_bind(*path)?;

                let connector = Connector(path);
                let acceptor = Acceptor(listen);
                Ok(((), connector, acceptor))
            })
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_api() {
        let one = Arc::new("one");
        let two = Arc::new("two");

        let (_, _p1, _r1) =
            Tx3Pool::bind(Transport, one.clone()).await.unwrap();
        let (_, p2, _r2) = Tx3Pool::bind(Transport, two.clone()).await.unwrap();

        p2.send(one.clone(), b"hello").await.unwrap();
    }
}
