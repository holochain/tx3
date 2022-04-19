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

pub mod addr;
pub mod config;
pub mod direct;
pub mod tls;
pub mod ws_framed;
pub mod yamux_framed;

mod tx3_endpoint;
pub use tx3_endpoint::*;

#[cfg(test)]
pub mod smoke_test;
