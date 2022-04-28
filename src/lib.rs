#![deny(missing_docs)]
#![deny(warnings)]
#![deny(unsafe_code)]
//! tx3 p2p communications
//!
//! ### Run a locally addressable tx3 node
//!
//! ```
//! # #[tokio::main]
//! # async fn main() {
//! let tx3_config = tx3::Tx3Config::new().with_bind("tx3-st://127.0.0.1:0");
//!
//! let (node, _inbound_con) = tx3::Tx3Node::new(tx3_config).await.unwrap();
//!
//! println!("listening on addresses: {:#?}", node.local_addrs());
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
//! let tx3_relay_config = tx3::Tx3RelayConfig::new()
//!     .with_bind("tx3-rst://127.0.0.1:0");
//! let relay = tx3::Tx3Relay::new(tx3_relay_config).await.unwrap();
//!
//! println!("relay listening on addresses: {:#?}", relay.local_addrs());
//! # }
//! ```
//!
//! ### Run a tx3 node relayed over the given relay address
//!
//! ```
//! # #[tokio::main]
//! # async fn main() {
//! # let tx3_relay_config = tx3::Tx3RelayConfig::new()
//! #     .with_bind("tx3-rst://127.0.0.1:0");
//! # let relay = tx3::Tx3Relay::new(tx3_relay_config).await.unwrap();
//! # let relay_addr = relay.local_addrs()[0].to_owned();
//! # let relay_addr = &relay_addr;
//! // set relay_addr to your relay address, something like:
//! // let relay_addr = "tx3-rst://127.0.0.1:38141/EHoKZ3-8R520Unp3vr4xeP6ogYAqoZ-em8lm-rMlwhw";
//!
//! let tx3_config = tx3::Tx3Config::new().with_bind(relay_addr);
//!
//! let (node, _inbound_con) = tx3::Tx3Node::new(tx3_config).await.unwrap();
//!
//! println!("listening on addresses: {:#?}", node.local_addrs());
//! # }
//! ```

#![doc = include_str!("docs/tx3_relay_help.md")]

/// Tx3 helper until `std::io::Error::other()` is stablized
pub fn other_err<E: Into<Box<dyn std::error::Error + Send + Sync>>>(
    error: E,
) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::Other, error)
}

use std::io::Result;

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

mod relay;
pub use relay::*;

#[cfg(test)]
mod smoke_test;
