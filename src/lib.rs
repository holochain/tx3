#![deny(missing_docs)]
#![deny(warnings)]
#![deny(unsafe_code)]
//! tx3 p2p communications

/// Tx3 helper until `std::io::Error::other()` is stablized
pub fn other_err<E: Into<Box<dyn std::error::Error + Send + Sync>>>(error: E) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::Other, error)
}

use std::io::Result;

mod bytes_list;
pub use bytes_list::*;

pub mod framed;
pub mod mplex;

#[cfg(any(test, feature = "dev_utils"))]
pub mod dev_utils;

#[cfg(test)]
pub mod smoke_test;
