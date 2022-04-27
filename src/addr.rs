//! addr utilities

use crate::tls::*;
use crate::*;
use std::net::IpAddr;
use std::net::Ipv4Addr;
use std::net::Ipv6Addr;
use std::net::SocketAddr;

/// Tx3 url schemes, see Tx3Url for more details
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum Tx3Scheme<'a> {
    /// `tx3-st` - direct TLS over TCP stream socket
    Tx3st,

    /// `tx3-rst` - relayed TLS over TCP
    Tx3rst,

    /// Other tx3 scheme not handled by this implementation
    Other(&'a str),
}

impl Tx3Scheme<'_> {
    /// Get the scheme as a &str
    pub fn as_str(&self) -> &str {
        match self {
            Tx3Scheme::Tx3st => "tx3-st",
            Tx3Scheme::Tx3rst => "tx3-rst",
            Tx3Scheme::Other(s) => s,
        }
    }
}

impl<'a> From<&'a str> for Tx3Scheme<'a> {
    fn from(s: &'a str) -> Self {
        match s {
            "tx3-st" => Tx3Scheme::Tx3st,
            "tx3-rst" => Tx3Scheme::Tx3rst,
            oth => Tx3Scheme::Other(oth),
        }
    }
}

/// A url representing a tx3 p2p endpoint.
///
/// Example binding urls:
///
/// - `tx3-st://0.0.0.0:0` - bind to all interfaces, os-determined port
/// - `tx3-rst://relay.holo.host:12345/relay_cert_digest` - bind as a
///   relay client to a relay hosted at the given host/port using a tls
///   cert with the given sha256 digest (base64url encoded with no pad)
///
/// Example addressable urls after binding:
///
/// - `tx3-st://1.1.1.1:12345/my_node_cert` - reachable at host/port
///   with the given node tls cert sha256 digest (base64url encoded no pad)
/// - `tx3-rst://relay.holo.host:12345/my_node_cert` - reachable via relay
///   with the node cert instead of the relay cert
#[derive(
    Clone,
    serde::Serialize,
    serde::Deserialize,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
)]
#[serde(transparent)]
pub struct Tx3Url(url::Url);

impl std::fmt::Display for Tx3Url {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl std::fmt::Debug for Tx3Url {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<Tx3Url> for url::Url {
    fn from(t: Tx3Url) -> Self {
        t.0
    }
}

impl From<&str> for Tx3Url {
    fn from(s: &str) -> Self {
        Tx3Url::new(url::Url::parse(s).expect("invalid tx3 url"))
    }
}

impl From<&Tx3Url> for Tx3Url {
    fn from(u: &Tx3Url) -> Self {
        u.clone()
    }
}

impl Tx3Url {
    /// Construct & verify a tx3 url
    pub fn new(url: url::Url) -> Self {
        url.host_str().expect("tx3 url must include a host");
        url.port().expect("tx3 url must include a port");
        Self(url)
    }

    /// Read the tx3 scheme from the url
    pub fn scheme(&self) -> Tx3Scheme<'_> {
        self.0.scheme().into()
    }

    /// Read the certificate digest (if it exists) from the url
    pub fn tls_cert_digest(&self) -> Option<TlsCertDigest> {
        if let Some(mut i) = self.0.path_segments() {
            if let Some(s) = i.next() {
                if let Ok(d) = tls_cert_digest_b64_dec(s) {
                    return Some(d);
                }
            }
        }

        None
    }

    /// Generate a new tx3 url with a specific tls cert digest
    pub fn with_cert_digest(&self, cert_digest: &TlsCertDigest) -> Self {
        let mut u = self.0.clone();
        u.set_path(&tls_cert_digest_b64_enc(cert_digest));
        Self(u)
    }

    /// Translate this tx3 url into a socket addr we can use to
    /// attempt a connection
    pub(crate) async fn socket_addrs(
        &self,
    ) -> Result<impl Iterator<Item = SocketAddr>> {
        tokio::net::lookup_host(format!(
            "{}:{}",
            self.0.host_str().unwrap(),
            self.0.port().unwrap(),
        ))
        .await
        .map_err(other_err)
    }
}

trait IpAddrExt {
    fn ext_is_global(&self) -> bool;
}

impl IpAddrExt for Ipv4Addr {
    #[inline]
    fn ext_is_global(&self) -> bool {
        if u32::from_be_bytes(self.octets()) == 0xc0000009
            || u32::from_be_bytes(self.octets()) == 0xc000000a
        {
            return true;
        }
        !self.is_private()
            && !self.is_loopback()
            && !self.is_link_local()
            && !self.is_broadcast()
            && !self.is_documentation()
            // is_shared()
            && !(self.octets()[0] == 100 && (self.octets()[1] & 0b1100_0000 == 0b0100_0000))
            && !(self.octets()[0] == 192 && self.octets()[1] == 0 && self.octets()[2] == 0)
            // is_reserved()
            && !(self.octets()[0] & 240 == 240 && !self.is_broadcast())
            // is_benchmarking()
            && !(self.octets()[0] == 198 && (self.octets()[1] & 0xfe) == 18)
            && self.octets()[0] != 0
    }
}

impl IpAddrExt for Ipv6Addr {
    #[inline]
    fn ext_is_global(&self) -> bool {
        !self.is_multicast()
            && !self.is_loopback()
            //&& !self.is_unicast_link_local()
            && !((self.segments()[0] & 0xffc0) == 0xfe80)
            //&& !self.is_unique_local()
            && !((self.segments()[0] & 0xfe00) == 0xfc00)
            && !self.is_unspecified()
            //&& !self.is_documentation()
            && !((self.segments()[0] == 0x2001) && (self.segments()[1] == 0xdb8))
    }
}

pub(crate) fn upgrade_addr(addr: SocketAddr) -> Result<Vec<SocketAddr>> {
    let port = addr.port();
    Ok(match &addr {
        SocketAddr::V4(a) => {
            if a.ip() == &Ipv4Addr::UNSPECIFIED {
                let mut loopback = None;
                let mut lan = None;
                let mut out = Vec::new();
                for iface in get_if_addrs::get_if_addrs()? {
                    if let IpAddr::V4(a) = iface.ip() {
                        if a.ext_is_global() {
                            out.push((a, port).into());
                        } else {
                            if loopback.is_none() && a.is_loopback() {
                                loopback = Some((a, port).into());
                            }
                            if lan.is_none() && !a.is_loopback() {
                                lan = Some((a, port).into());
                            }
                        }
                    }
                }
                if out.is_empty() && lan.is_some() {
                    out.push(lan.take().unwrap());
                }
                if out.is_empty() && loopback.is_some() {
                    out.push(loopback.take().unwrap());
                }
                out
            } else {
                vec![addr]
            }
        }
        SocketAddr::V6(a) => {
            if a.ip() == &Ipv6Addr::UNSPECIFIED {
                let mut loopback = None;
                let mut lan = None;
                let mut out = Vec::new();
                for iface in get_if_addrs::get_if_addrs()? {
                    if let IpAddr::V6(a) = iface.ip() {
                        if a.ext_is_global() {
                            out.push((a, port).into());
                        } else {
                            if loopback.is_none() && a.is_loopback() {
                                loopback = Some((a, port).into());
                            }
                            if lan.is_none() && !a.is_loopback() {
                                lan = Some((a, port).into());
                            }
                        }
                    }
                }
                if out.is_empty() && lan.is_some() {
                    out.push(lan.take().unwrap());
                }
                if out.is_empty() && loopback.is_some() {
                    out.push(loopback.take().unwrap());
                }
                out
            } else {
                vec![addr]
            }
        }
    })
}
