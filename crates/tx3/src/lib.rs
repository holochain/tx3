#![deny(missing_docs)]
#![deny(warnings)]
#![deny(unsafe_code)]
//! tx3 p2p communications
//!
//! [![Project](https://img.shields.io/badge/project-holochain-blue.svg?style=flat-square)](http://holochain.org/)
//! [![Forum](https://img.shields.io/badge/chat-forum%2eholochain%2enet-blue.svg?style=flat-square)](https://forum.holochain.org)
//! [![Chat](https://img.shields.io/badge/chat-chat%2eholochain%2enet-blue.svg?style=flat-square)](https://chat.holochain.org)
//!
//! [![License: Apache-2.0](https://img.shields.io/badge/License-Apache%202.0-blue.svg)](https://www.apache.org/licenses/LICENSE-2.0)
//!
//! ### An interim Holochain networking solution
//!
//! Holochain has experimented with many different networking solutions.
//! We mainly have our eye on webrtc, due to the benefits of direct STUN
//! p2p negotiation, with fallback TURN relaying for non-symmetric network
//! cases. We've also run on QUIC. For the time being, however, none of these
//! solutions have proven mature enough in the rust ecosystem to get us
//! the performance, reliability, and end-to-end encryption we need.
//!
//! Tx3 provides a very simple interim stack: TLS over TCP. A relay server
//! splices raw TCP streams together providing addressability for clients
//! behind NATs. The clients negotiate end-to-end TLS over these spliced
//! streams, ensuring the relay server or any MITM has no access to the
//! plaintext. (If you want to learn more about the relay protocol, see
//! the [crate::relay] docs.)
//!
//! ```text
//!        +-------+
//!        | relay | (TCP splicing)
//!        +-------+
//!         /     \
//!        /       \  (TLS over TCP)
//!       /         \
//! +-------+     +-------+
//! | node1 |     | node2 |
//! +-------+     +-------+
//! ```
//!
//! As with many p2p solutions, ensuring you are talking to who you think you
//! are is left to other layers of the application. But tx3 tries to make
//! this simple by including the TLS certificate digest in the url, so that
//! holochain can easily provide signature verification.
//!
//! E.g.
//!
//! ```text
//! tx3:EHoKZ3-8R520Unp3vr4xeP6ogYAqoZ-em8lm-rMlwhw/rst/127.0.0.1:38141/
//! ```
//!
//! Nodes that are directly addressable, or can configure port-forwarding are
//! welcome and encouraged to make direct connections, instead of relaying.
//!
//! ```text
//! +-------+ (TLS over TCP) +-------+
//! | node1 |----------------| node2 |
//! +-------+                +-------+
//! ```
//!
//! ### Run a locally addressable tx3 node
//!
//! ```
//! # #[tokio::main]
//! # async fn main() {
//! let tx3_config = tx3::Tx3Config::new().with_bind("tx3:-/st/127.0.0.1:0/");
//!
//! let (node, _inbound_con) = tx3::Tx3Node::new(tx3_config).await.unwrap();
//!
//! println!("listening on address: {:?}", node.local_addr());
//! # }
//! ```
//!
//! ### Run an in-process relay node
//!
//! Note: Unless you're writing test code, you probably want the executable.
//! See below for `tx3-relay` commandline flags and options.
//!
//! ```
//! # #[tokio::main]
//! # async fn main() {
//! let tx3_relay_config = tx3::relay::Tx3RelayConfig::new()
//!     .with_bind("tx3:-/rst/127.0.0.1:0/");
//! let relay = tx3::relay::Tx3Relay::new(tx3_relay_config).await.unwrap();
//!
//! println!("relay listening on address: {:?}", relay.local_addr());
//! # }
//! ```
//!
//! ### Run a tx3 node relayed over the given relay address
//!
//! ```
//! # #[tokio::main]
//! # async fn main() {
//! # let tx3_relay_config = tx3::relay::Tx3RelayConfig::new()
//! #     .with_bind("tx3:-/rst/127.0.0.1:0/");
//! # let relay = tx3::relay::Tx3Relay::new(tx3_relay_config).await.unwrap();
//! # let relay_addr = relay.local_addr().clone();
//! # let relay_addr = &relay_addr;
//! // set relay_addr to your relay address, something like:
//! // let relay_addr = "tx3:EHoKZ3-8R520Unp3vr4xeP6ogYAqoZ-em8lm-rMlwhw/rst/127.0.0.1:38141/";
//!
//! let tx3_config = tx3::Tx3Config::new().with_bind(relay_addr);
//!
//! let (node, _inbound_con) = tx3::Tx3Node::new(tx3_config).await.unwrap();
//!
//! println!("listening on address: {:?}", node.local_addr());
//! # }
//! ```
//!
//! ### Connect and receive connections
//!
//! ```
//! # #[tokio::main]
//! # async fn main() {
//! # use tokio::io::AsyncReadExt;
//! # use tokio::io::AsyncWriteExt;
//! // create a listening node
//! let tx3_config = tx3::Tx3Config::new().with_bind("tx3:-/st/127.0.0.1:0/");
//! let (node1, mut recv1) = tx3::Tx3Node::new(tx3_config).await.unwrap();
//! let addr1 = node1.local_addr().clone();
//!
//! // listen for incoming connections
//! let task = tokio::task::spawn(async move {
//!     let (acceptor, _addr) = recv1.recv().await.unwrap();
//!
//!     // in production code we might want to spawn this so we can
//!     // receive more connections, handling their handshakes in paralel
//!     let mut in_con = acceptor.accept().await.unwrap();
//!
//!     // make sure we can read data
//!     let mut got = [0; 5];
//!     in_con.read_exact(&mut got).await.unwrap();
//!     assert_eq!(b"hello", &got[..]);
//!
//!     // make sure we can write data
//!     in_con.write_all(b"world").await.unwrap();
//!     in_con.shutdown().await.unwrap();
//! });
//!
//! // create an outgoing-only node
//! let tx3_config = tx3::Tx3Config::new();
//! let (node2, _) = tx3::Tx3Node::new(tx3_config).await.unwrap();
//!
//! // connect from our outgoing node to our receive node
//! let mut out_con = node2.connect(&addr1).await.unwrap();
//!
//! // make sure we can write data
//! out_con.write_all(b"hello").await.unwrap();
//! out_con.shutdown().await.unwrap();
//!
//! // make sure we can read data
//! let mut got = [0; 5];
//! out_con.read_exact(&mut got).await.unwrap();
//! assert_eq!(b"world", &got[..]);
//!
//! // make sure our receiver task shuts down cleanly
//! task.await.unwrap();
//! # }
//! ```

#![doc = include_str!("docs/tx3_relay_help.md")]

use std::future::Future;
use std::io::Result;
use std::sync::atomic;
use std::sync::Arc;

/// Tx3 helper until `std::io::Error::other()` is stablized
pub fn other_err<E: Into<Box<dyn std::error::Error + Send + Sync>>>(
    error: E,
) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::Other, error)
}

/// Terminal notification helper utility.
#[derive(Clone)]
pub(crate) struct Term {
    term: Arc<atomic::AtomicBool>,
    sig: Arc<tokio::sync::Notify>,
    trgr: Arc<dyn Fn() + 'static + Send + Sync>,
}

impl Term {
    /// Construct a new term instance with optional term callback.
    pub fn new(trgr: Option<Arc<dyn Fn() + 'static + Send + Sync>>) -> Self {
        let trgr = trgr.unwrap_or_else(|| Arc::new(|| {}));
        Self {
            term: Arc::new(atomic::AtomicBool::new(false)),
            sig: Arc::new(tokio::sync::Notify::new()),
            trgr,
        }
    }

    /// Trigger termination.
    pub fn term(&self) {
        self.term.store(true, atomic::Ordering::Release);
        self.sig.notify_waiters();
        (self.trgr)();
    }

    /*
    /// Returns `true` if termination has been triggered.
    pub fn is_term(&self) -> bool {
        self.term.load(atomic::Ordering::Acquire)
    }
    */

    /// Returns a future that will resolve when termination is triggered.
    pub fn on_term(
        &self,
    ) -> impl std::future::Future<Output = ()> + 'static + Send + Unpin {
        use std::pin::Pin;
        use std::task::Poll;
        let sig = self.sig.clone();
        let mut fut = Box::pin(async move { sig.notified().await });
        let term = self.term.clone();
        futures::future::poll_fn(move |cx| {
            if term.load(atomic::Ordering::Acquire) {
                return Poll::Ready(());
            }
            match Pin::new(&mut fut).poll(cx) {
                Poll::Pending => (),
                Poll::Ready(_) => return Poll::Ready(()),
            }
            // even though we create the "notified" future first, there's still
            // a small chance we could have missed the notification, between
            // the previous flag load and when we called poll... so check again
            if term.load(atomic::Ordering::Acquire) {
                return Poll::Ready(());
            }
            // now, our cx is registered for wake, and we've double checked
            // the flag, safe to return pending
            Poll::Pending
        })
    }
}

pub(crate) mod tcp;

mod addr;
pub use addr::*;

mod config;
pub use config::*;

pub mod tls;

mod connection;
pub use connection::*;

mod node;
pub use node::*;

pub mod relay;

#[cfg(test)]
mod relay_test;

#[cfg(test)]
mod smoke_test;
