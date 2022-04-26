//! Tx3 configuration

use crate::tls::*;
use crate::*;

/// Tx3 configuration
#[non_exhaustive]
#[derive(Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tx3Config {
    /// The list of bindings we should attempt for addressablility by
    /// remote peers. The default empty list indicates we will NOT be
    /// addressable, and can only make outgoing connections.
    ///
    /// Currently, we support the following schemes:
    ///
    /// - `tx3-st` for binding to local tcp network interfaces
    /// - `tx3-rst` for binding through a remote relay tcp splicer
    pub bind: Vec<Tx3Url>,

    /// TLS configuration for this node. This will never be serialized
    /// to prevent bad practices. You should set this after deserializing
    /// from your configuration file.
    /// If not specified a default / ephemeral tls config will be generated.
    #[serde(skip)]
    pub tls: Option<TlsConfig>,
}

impl Tx3Config {
    /// Construct a new default Tx3Config
    pub fn new() -> Self {
        Tx3Config::default()
    }

    /// Append a bind to the list of bindings
    pub fn with_bind<B>(mut self, bind: B) -> Self
    where
        B: Into<Tx3Url>,
    {
        self.bind.push(bind.into());
        self
    }

    // -- private -- //

    /// privately, the option is always some, so we can unwrap the ref
    pub(crate) fn priv_tls(&self) -> &TlsConfig {
        self.tls.as_ref().unwrap()
    }
}
