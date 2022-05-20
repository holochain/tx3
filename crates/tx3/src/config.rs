//! Tx3 configuration

// sometimes it's clearer to do it yourself clippy...
#![allow(clippy::derivable_impls)]

use crate::tls::*;
use crate::*;
use std::sync::Arc;

/// Tx3 configuration
#[non_exhaustive]
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tx3Config {
    /// The list of bindings we should attempt for addressablility by
    /// remote peers. The default empty list indicates we will NOT be
    /// addressable, and can only make outgoing connections.
    ///
    /// Currently, we support the following stacks:
    ///
    /// - `st` for binding to local tcp network interfaces
    ///   - e.g.: `tx3:-/st/0.0.0.0:0/`
    /// - `rst` for binding through a remote relay tcp splicer
    ///   - e.g.: `tx3:EHoKZ3-8R520Unp3vr4xeP6ogYAqoZ-em8lm-rMlwhw/rst/127.0.0.1:38141/`
    #[serde(default)]
    pub bind: Vec<Arc<Tx3Addr>>,

    /// TLS configuration for this node. This will never be serialized
    /// to prevent bad practices. You should set this after deserializing
    /// from your configuration file.
    /// If not specified a default / ephemeral tls config will be generated.
    #[serde(skip)]
    pub tls: Option<TlsConfig>,
}

impl Default for Tx3Config {
    fn default() -> Self {
        Self {
            bind: Vec::new(),
            tls: None,
        }
    }
}

impl Tx3Config {
    /// Construct a new default Tx3Config
    pub fn new() -> Self {
        Tx3Config::default()
    }

    /// Append a bind to the list of bindings
    pub fn with_bind<A>(mut self, bind: A) -> Self
    where
        A: IntoAddr,
    {
        self.bind.push(bind.into_addr());
        self
    }

    /// Specify a tls config to use.
    pub fn with_tls(mut self, tls: TlsConfig) -> Self {
        self.tls = Some(tls);
        self
    }

    // -- private -- //

    /// privately, the option is always some, so we can unwrap the ref
    pub(crate) fn priv_tls(&self) -> &TlsConfig {
        self.tls.as_ref().unwrap()
    }
}
