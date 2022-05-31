//! bind url types

use crate::*;
use std::sync::Arc;

/// A tx3 bind specification lets us know how to go about listening for
/// incoming connections. This is a fully resolved specification that
/// likely won't be in a configuration file. See [Tx3BindConfig] for
/// the enum that would likely be put into configuration.
#[derive(
    Clone,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    serde::Serialize,
    serde::Deserialize,
)]
#[serde(rename_all = "camelCase", tag = "type")]
pub enum Tx3BindSpec {
    /// Directly bind to a specific network interface.
    Direct {
        /// The local interface address to bind.
        interface_addr: std::net::SocketAddr,

        /// The global address to publish. This could be the same as
        /// `interface` if this is a server with a directly bound ip addr.
        publish_addr: std::net::SocketAddr,
    },

    /// Accept incoming connections from a specific relay server.
    Relay {
        /// The address of the relay server through which connections
        /// should be relayed.
        relay_addr: Arc<Tx3Addr>,
    },
}

/// Configuration indicating how to go about listening for incoming connections.
#[derive(
    Clone,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    serde::Serialize,
    serde::Deserialize,
)]
#[serde(rename_all = "camelCase", tag = "type")]
pub enum Tx3BindConfig {
    /// Make a best effort to bind one global ipv6 address and one
    /// global ipv4 address, making use of the given relay servers
    /// for reflection. If unable to obtain both address types,
    /// a fallback relay connection will be established to one of the
    /// given relay servers.
    FullAuto {
        /// An optional port. Must be > 0. If `None`, a random port in
        /// the ephemeral range (32768-60999) will be chosen.
        port: Option<u16>,

        /// A prioritized list of relay server addresses to be used for
        /// reflection, or fallback relay services.
        relay_list: Vec<Tx3Addr>,
    },
}
