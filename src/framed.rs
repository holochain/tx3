//! Wrapper types for framed messages

use crate::*;
use futures::{Stream, Sink};
use std::pin::Pin;
use std::task::Context;
use std::task::Poll;
use tokio_tungstenite::tungstenite::Message;
use bytes::Buf;

/// Represents an IO device that can send / recv framed binary data
pub trait Framed: Sink<BytesList, Error = std::io::Error> + Stream<Item = Result<BytesList>> {}

mod ws_framed;
pub use ws_framed::*;

mod framed_stream;
pub use framed_stream::*;
