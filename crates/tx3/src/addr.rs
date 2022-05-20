//! addr utilities

// many of these are coppied from the nightly std library, don't want to muck
#![allow(clippy::nonminimal_bool)]

use crate::*;
use std::net::IpAddr;
use std::net::Ipv4Addr;
use std::net::Ipv6Addr;
use std::net::SocketAddr;
use std::sync::Arc;

/// Tx3 node/peer identifier. Sha256 digest of DER encoded tls certificate.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Tx3Id(pub [u8; 32]);

impl std::fmt::Debug for Tx3Id {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut a = self.to_b64();
        a.replace_range(8..a.len() - 8, "..");
        f.write_str(&a)
    }
}

impl std::fmt::Display for Tx3Id {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.to_b64())
    }
}

impl std::ops::Deref for Tx3Id {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        &self.0[..]
    }
}

impl AsRef<[u8]> for Tx3Id {
    fn as_ref(&self) -> &[u8] {
        &self.0[..]
    }
}

impl Tx3Id {
    /// Decode a base64 encoded Tx3Id.
    pub fn from_b64(s: &str) -> Result<Arc<Self>> {
        let v = base64::decode_config(s, base64::URL_SAFE_NO_PAD)
            .map_err(other_err)?;
        if v.len() != 32 {
            return Err(other_err("InvalidTlsCertDigest"));
        }
        let mut out = [0; 32];
        out.copy_from_slice(&v);
        Ok(Arc::new(Self(out)))
    }

    /// Encode a Tx3Id as base64.
    pub fn to_b64(&self) -> String {
        base64::encode_config(self, base64::URL_SAFE_NO_PAD)
    }
}

/// Available stacks for tx3 backend transports.
#[derive(Debug)]
pub enum Tx3Stack {
    /// `st` - direct TLS over TCP stream socket.
    TlsTcp(String),

    /// `rst` - relayed TLS over TCP.
    RelayTlsTcp(String),

    /// Other tx3 stack not handled by this implementation.
    Other(String, String),
}

impl std::fmt::Display for Tx3Stack {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Tx3Stack::TlsTcp(v) => write!(f, "st/{}", v),
            Tx3Stack::RelayTlsTcp(v) => write!(f, "rst/{}", v),
            Tx3Stack::Other(k, v) => write!(f, "{}/{}", k, v),
        }
    }
}

impl Tx3Stack {
    /// Parse &str pair into a Tx3Stack variant.
    pub fn from_pair(k: &str, v: &str) -> Self {
        match k {
            "st" => Tx3Stack::TlsTcp(v.to_string()),
            "rst" => Tx3Stack::RelayTlsTcp(v.to_string()),
            _ => Tx3Stack::Other(k.to_string(), v.to_string()),
        }
    }

    /// Get the stack as &str pair.
    pub fn as_pair(&self) -> (&str, &str) {
        match self {
            Tx3Stack::TlsTcp(v) => ("st", v.as_str()),
            Tx3Stack::RelayTlsTcp(v) => ("rst", v.as_str()),
            Tx3Stack::Other(k, v) => (k.as_str(), v.as_str()),
        }
    }
}

/// A Tx3 Addr is a canonical peer identifier grouped with a prioritized
/// list of stacks at which the peer can be reached.
/// A Tx3 Addr can be encoded as a `tx3:` url.
#[derive(Default, Clone)]
pub struct Tx3Addr {
    /// Peer identifier.
    pub id: Option<Arc<Tx3Id>>,

    /// Prioritized list of stacks.
    pub stack_list: Vec<Arc<Tx3Stack>>,
}

impl serde::Serialize for Tx3Addr {
    fn serialize<S>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_url())
    }
}

impl<'de> serde::Deserialize<'de> for Tx3Addr {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let tmp: String = serde::Deserialize::deserialize(deserializer)?;
        Ok(tmp.into())
    }
}

impl std::fmt::Debug for Tx3Addr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.to_url())
    }
}

impl std::fmt::Display for Tx3Addr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.to_url())
    }
}

/// Indicates a type that can be turned into an `Arc<Tx3Addr>`.
pub trait IntoAddr {
    /// Convert this type into an `Arc<Tx3Addr>`.
    fn into_addr(self) -> Arc<Tx3Addr>;
}

impl IntoAddr for Arc<Tx3Addr> {
    fn into_addr(self) -> Arc<Tx3Addr> {
        self
    }
}

impl IntoAddr for &Arc<Tx3Addr> {
    fn into_addr(self) -> Arc<Tx3Addr> {
        self.clone()
    }
}

impl IntoAddr for &str {
    #[inline(always)]
    fn into_addr(self) -> Arc<Tx3Addr> {
        Arc::new(self.into())
    }
}

impl IntoAddr for String {
    #[inline(always)]
    fn into_addr(self) -> Arc<Tx3Addr> {
        Arc::new(self.into())
    }
}

impl IntoAddr for &String {
    #[inline(always)]
    fn into_addr(self) -> Arc<Tx3Addr> {
        Arc::new(self.into())
    }
}

impl IntoAddr for url::Url {
    #[inline(always)]
    fn into_addr(self) -> Arc<Tx3Addr> {
        Arc::new(self.into())
    }
}

impl From<&str> for Tx3Addr {
    #[inline(always)]
    fn from(s: &str) -> Self {
        match url::Url::parse(s) {
            Ok(url) => url.into(),
            Err(_) => Tx3Addr::default(),
        }
    }
}

impl From<String> for Tx3Addr {
    #[inline(always)]
    fn from(s: String) -> Self {
        s.as_str().into()
    }
}

impl From<&String> for Tx3Addr {
    #[inline(always)]
    fn from(s: &String) -> Self {
        s.as_str().into()
    }
}

impl From<url::Url> for Tx3Addr {
    fn from(url: url::Url) -> Self {
        let mut this = Tx3Addr::default();

        let mut iter = url.path().split_terminator('/');

        match iter.next() {
            None => return this,
            Some(id) => {
                if let Ok(id) = Tx3Id::from_b64(id) {
                    this.id = Some(id);
                }
            }
        }

        loop {
            let k = match iter.next() {
                None => break,
                Some(k) => k,
            };

            let v = match iter.next() {
                None => return this,
                Some(v) => v,
            };

            this.stack_list.push(Arc::new(Tx3Stack::from_pair(k, v)));
        }

        this
    }
}

impl Tx3Addr {
    /// Encode this addr instance as a url.
    pub fn to_url(&self) -> String {
        let mut out = String::new();
        out.push_str("tx3:");
        if let Some(id) = &self.id {
            out.push_str(&id.to_b64());
        } else {
            out.push('-');
        }
        out.push('/');
        for stack in self.stack_list.iter() {
            let (k, v) = stack.as_pair();
            out.push_str(k);
            out.push('/');
            out.push_str(v);
            out.push('/');
        }
        out
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
