#![deny(missing_docs)]
#![deny(warnings)]
#![deny(unsafe_code)]
//! tx3 p2p communications

/// Tx3 helper until `std::io::Error::other()` is stablized
pub fn other_err<E: Into<Box<dyn std::error::Error + Send + Sync>>>(
    error: E,
) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::Other, error)
}

use std::io::Result;

mod addr;
pub use addr::*;

mod config;
pub use config::*;

pub mod tls;

mod connection;
pub use connection::*;

mod node;
pub use node::*;

#[cfg(test)]
mod smoke_test;
