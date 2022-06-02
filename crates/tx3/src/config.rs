//! Tx3 configuration

// sometimes it's clearer to do it yourself clippy...
#![allow(clippy::derivable_impls)]

use crate::tls::*;
use crate::*;
use std::sync::Arc;

fn default_tls() -> TlsConfig {
    TlsConfigBuilder::default().build().unwrap()
}

fn is_false(v: &bool) -> bool {
    !*v
}

/// An interface binding specification defines how to bind to a specific
/// local network interface. While it's possible to set the local_interface
/// to an UNSPECIFIED ip, you will still only be able to specify a single
/// wan host for this binding entry. In general, it is recommended to
/// specify a specific local_interface. If you set the local interface
/// port to UNSPECIFIED (`0`), you should also set the wan_port to `0`.
/// The `notes` field is not parsed by this library. It exists only for
/// configuration files.
#[derive(
    Clone,
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    serde::Serialize,
    serde::Deserialize,
)]
#[serde(rename_all = "camelCase")]
pub struct Tx3BindSpec {
    /// The local interface socket address to bind.
    pub local_interface: std::net::SocketAddr,

    /// The global public host name (or address) to publish.
    /// This can be the same as the interface address if this node
    /// is directly bound to a global ip address. If this is an ip
    /// v6 address, it should include the square brackets ("[..]").
    pub wan_host: String,

    /// The global public port to publish.
    pub wan_port: u16,

    /// If set to false, this binding should be ignored.
    pub enabled: bool,

    /// Normally a binding will error if the public host resolves to
    /// an ip that is not globally addressable.
    /// For testing, you can disable this check.
    #[serde(default, skip_serializing_if = "is_false")]
    pub allow_non_global_host: bool,

    /// User notes about this binding.
    #[serde(default)]
    pub notes: Vec<String>,
}

impl Tx3BindSpec {
    /// Various sanity checks / fixes to determine the resultant address
    /// from a binding operation.
    pub async fn resolve_binding(&self, bound_port: u16) -> Result<String> {
        let port = if self.wan_port == 0 {
            bound_port
        } else {
            self.wan_port
        };

        use std::str::FromStr;
        if let Ok(ip) = std::net::IpAddr::from_str(&self.wan_host) {
            if ip.is_unspecified() {
                return Err(other_err("wan_host cannot be an unspecified ip"));
            }
        }

        let out_addr = format!("{}:{}", &self.wan_host, port);

        for addr in tokio::net::lookup_host(&out_addr).await? {
            if !self.allow_non_global_host && !addr.ip().ext_is_global() {
                return Err(other_err(format!(
                    "{}({}) is not globally addressable, cannot bind. Configure port forwarding and adjust host string.",
                    out_addr,
                    addr,
                )));
            }
        }

        Ok(out_addr)
    }
}

/// Indicates a type that can be turned into a `Tx3BindSpec`.
pub trait IntoBindSpec {
    /// Convert this type into a `Tx3BindSpec`.
    fn into_bind_spec(self) -> Result<Tx3BindSpec>;
}

impl IntoBindSpec for Tx3BindSpec {
    #[inline(always)]
    fn into_bind_spec(self) -> Result<Tx3BindSpec> {
        Ok(self)
    }
}

impl IntoBindSpec for &Tx3BindSpec {
    #[inline(always)]
    fn into_bind_spec(self) -> Result<Tx3BindSpec> {
        Ok(self.clone())
    }
}

impl IntoBindSpec for &str {
    #[inline(always)]
    fn into_bind_spec(self) -> Result<Tx3BindSpec> {
        use std::str::FromStr;
        let addr = std::net::SocketAddr::from_str(self).map_err(other_err)?;
        let wan_host = match &addr {
            std::net::SocketAddr::V4(a) => a.ip().to_string(),
            std::net::SocketAddr::V6(a) => format!("[{}]", a.ip()),
        };
        Ok(Tx3BindSpec {
            local_interface: addr,
            wan_host,
            wan_port: addr.port(),
            enabled: true,
            allow_non_global_host: false,
            notes: Vec::new(),
        })
    }
}

impl IntoBindSpec for (&str, bool) {
    #[inline(always)]
    fn into_bind_spec(self) -> Result<Tx3BindSpec> {
        use std::str::FromStr;
        let addr = std::net::SocketAddr::from_str(self.0).map_err(other_err)?;
        let wan_host = match &addr {
            std::net::SocketAddr::V4(a) => a.ip().to_string(),
            std::net::SocketAddr::V6(a) => format!("[{}]", a.ip()),
        };
        Ok(Tx3BindSpec {
            local_interface: addr,
            wan_host,
            wan_port: addr.port(),
            enabled: true,
            allow_non_global_host: self.1,
            notes: Vec::new(),
        })
    }
}

/// Tx3 configuration
#[non_exhaustive]
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tx3Config {
    /// TLS configuration for this node. This will never be serialized
    /// to prevent bad practices. You should set this after deserializing
    /// from your configuration file.
    /// If not specified an ephemeral tls config will be generated.
    #[serde(skip, default = "default_tls")]
    pub tls: TlsConfig,

    /// A prioritized list of relay servers to use for both address reflection
    /// and as a fallback for data relay if we are unable to resolve globally
    /// addressable network addresses.
    #[serde(default)]
    pub relay_list: Vec<Arc<Tx3Addr>>,

    /// The list of bindings we should attempt for addressablility by
    /// remote peers.
    /// If this list does not contain at least one ipv4 address and at
    /// least one ipv6 address, an auto-detection routine will be run
    /// to see if there is a local interface capable of prividing a global
    /// address. If there is not, a relay server will be chosen from
    /// the relay_list, and appended as a fallback option.
    #[serde(default)]
    pub bind: Vec<Tx3BindSpec>,
}

impl Default for Tx3Config {
    fn default() -> Self {
        Self {
            relay_list: Vec::new(),
            bind: Vec::new(),
            tls: default_tls(),
        }
    }
}

impl Tx3Config {
    /// Construct a new default Tx3Config
    pub fn new() -> Self {
        Tx3Config::default()
    }

    /// Append a relay address to the list of relays
    pub fn with_relay<A: IntoAddr>(mut self, addr: A) -> Result<Self> {
        self.relay_list.push(addr.into_addr()?);
        Ok(self)
    }

    /// Append a bind to the list of bindings
    pub fn with_bind<B: IntoBindSpec>(mut self, bind: B) -> Result<Self> {
        self.bind.push(bind.into_bind_spec()?);
        Ok(self)
    }

    /// Specify a tls config to use.
    pub fn with_tls(mut self, tls: TlsConfig) -> Self {
        self.tls = tls;
        self
    }
}
