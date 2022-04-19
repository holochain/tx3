//! addr utilities

use crate::config::*;
use crate::*;
use std::net::IpAddr;
use std::net::Ipv4Addr;
use std::net::Ipv6Addr;
use std::net::SocketAddr;

/// A url representing a tx3 p2p endpoint
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

impl Tx3Url {
    /// Construct & verify a tx3 url
    pub fn new(url: url::Url) -> Self {
        if url.scheme() != "tx3" {
            panic!("tx3 url scheme must = 'tx3'");
        }
        url.host_str().expect("tx3 url must include a host");
        url.port().expect("tx3 url must include a port");
        {
            let mut i = url.path_segments().expect("no stack in tx3 url");
            let stack = i.next().expect("no stack in tx3 url");
            match Tx3Stack::from(stack) {
                Tx3Stack::Fwst => {
                    i.next().expect("no tls cert digest in fwst tx3 url");
                }
                oth => panic!("unhandle tx3 stack: {:?}", oth),
            }
        }
        Self(url)
    }

    /// Read the tx3 stack config from the url
    pub fn stack(&self) -> (Tx3Stack, std::str::Split<'_, char>) {
        let mut i = self.0.path_segments().unwrap();
        (Tx3Stack::from(i.next().unwrap()), i)
    }

    /// Translate this tx3 url into a socket addr we can use to
    /// attempt a connection
    pub async fn socket_addr(&self) -> Result<SocketAddr> {
        tokio::net::lookup_host(format!(
            "{}:{}",
            self.0.host_str().unwrap(),
            self.0.port().unwrap(),
        ))
        .await
        .map_err(other_err)?
        .next()
        .ok_or_else(|| other_err("invalid tx3 url"))
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
