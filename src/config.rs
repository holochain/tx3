//! Tx3 configuration

use std::net::SocketAddr;
use url::Url;

fn default_tcp_bind() -> Url {
    Url::parse("tcp://0.0.0.0:0").unwrap()
}

const TX3_STACK_FWST: &str = "fwst";
const TX3_STACK_FSPWST: &str = "fspwst";

/// The Tx3 stack to build. Higher-level items to the left,
/// lower-level items to the right. E.g. tls over tcp would be `"st"`.
///
/// - `p` - yamux based proxy/relay tunneling
/// - `f` - yamux based head-of-line block mitigation framing (5 streams)
/// - `w` - websocket binary-only message "stream-like" abstraction
/// - `s` - TLS server + client certificate based encryption
/// - `t` - TCP socket
///
/// Default: `"fwst"`
#[derive(Debug, Clone)]
pub enum Tx3Stack {
    /// This is the most generic direct tx3 connection stack. If you are WAN
    /// addressable, use this. If you are connecting to a WAN addressable node,
    /// you'll likely use this for outgoing connections.
    ///
    /// `YamuxFramed<WebsocketBin<TLS<TCP>>>`
    Fwst,

    /// This tx3 connection stack works around NAT using a relay (reverse proxy)
    /// that is WAN addressable. If you are behind a NAT, or are an app
    /// that cannot bind to network interfaces (such as a browser) you'll
    /// likely use this for binding an addressable endpoint.
    ///
    /// `YamuxFramed<TLS<YamuxRelay<WebsocketBin<TLS<TCP>>>>>`
    Fspwst,

    /// Other Tx3 stack
    Other(String),
}

impl Default for Tx3Stack {
    fn default() -> Self {
        Tx3Stack::Fwst
    }
}

impl AsRef<str> for Tx3Stack {
    fn as_ref(&self) -> &str {
        match self {
            Tx3Stack::Fwst => TX3_STACK_FWST,
            Tx3Stack::Fspwst => TX3_STACK_FSPWST,
            Tx3Stack::Other(s) => s.as_str(),
        }
    }
}

impl serde::Serialize for Tx3Stack {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.as_ref())
    }
}

impl<'de> serde::Deserialize<'de> for Tx3Stack {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let tmp: &'de str = serde::Deserialize::deserialize(deserializer)?;
        Ok(tmp.into())
    }
}

impl<'a> From<&'a str> for Tx3Stack {
    fn from(s: &'a str) -> Self {
        match s {
            TX3_STACK_FWST => Tx3Stack::Fwst,
            TX3_STACK_FSPWST => Tx3Stack::Fspwst,
            oth => Tx3Stack::Other(oth.to_string()),
        }
    }
}

/// Tx3 configuration
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tx3Config {
    // -- TCP -- //
    #[serde(default = "default_tcp_bind")]
    pub(crate) tcp_bind: Url,

    // -- TLS -- //
    pub(crate) tls_keylog: bool,

    #[serde(skip)]
    pub(crate) tls_cert_and_pk: Option<(Vec<u8>, Vec<u8>)>,

    // -- TX3 -- //
    pub(crate) tx3_stack: Tx3Stack,
}

impl Tx3Config {
    pub(crate) async fn tcp_bind_socket_addr(
        &self,
    ) -> crate::Result<SocketAddr> {
        tokio::net::lookup_host(format!(
            "{}:{}",
            self.tcp_bind.host_str().unwrap_or("0.0.0.0"),
            self.tcp_bind.port().unwrap_or(0),
        ))
        .await
        .map_err(crate::other_err)?
        .next()
        .ok_or_else(|| crate::other_err("invalid tcp_bind"))
    }
}

impl Default for Tx3Config {
    fn default() -> Self {
        Self {
            tcp_bind: default_tcp_bind(),
            tls_keylog: false,
            tls_cert_and_pk: None,
            tx3_stack: Tx3Stack::Fwst,
        }
    }
}

macro_rules! mk_set {
    ($(
        $(#[doc = $doc:expr])*
        $bname:ident $sname:ident $vname:ident $t:ty,
    )*) => {
        impl Tx3Config {$(
            $(#[doc = $doc])*
            pub fn $bname(mut self, $vname: $t) -> Self {
                self.$sname($vname);
                self
            }

            $(#[doc = $doc])*
            pub fn $sname(&mut self, $vname: $t) {
                self.$vname = $vname;
            }
        )*}
    };
}

mk_set! {
    /// Interface which should be bound using the "tcp://" scheme.
    /// To bind to all ipv4 interfaces, specify "tcp://0.0.0.0:0".
    ///
    /// Panicks if the scheme is not "tcp".
    ///
    /// Default: `"tcp://0.0.0.0:0"`.
    with_tcp_bind set_tcp_bind tcp_bind Url,

    /// Enable TLS keylog. The standard `SSLKEYLOGFILE` environment
    /// variable must also be set for keylogging to be enabled.
    ///
    /// Default: `false`.
    with_tls_keylog set_tls_keylog tls_keylog bool,

    /// Tx3 will either generate an ephemeral tls certificate / private key
    /// ore use a provided cert / pk. This field will never be serialized
    /// or deserialized, because it would be bad form to store the private
    /// key in a configuration file. Please specify *after* loading.
    ///
    /// Default: `None` - i.e. generate a new ephemeral pair
    with_tls_cert_and_pk set_tls_cert_and_pk tls_cert_and_pk Option<(Vec<u8>, Vec<u8>)>,

    /// The Tx3 stack to build. Higher-level items to the left,
    /// lower-level items to the right. E.g. tls over tcp would be `"st"`.
    ///
    /// - `p` - yamux based proxy/relay tunneling
    /// - `f` - yamux based head-of-line block mitigation framing (5 streams)
    /// - `w` - websocket binary-only message "stream-like" abstraction
    /// - `s` - TLS server + client certificate based encryption
    /// - `t` - TCP socket
    ///
    /// Default: `"fwst"`
    with_tx3_stack set_tx3_stack tx3_stack Tx3Stack,
}
