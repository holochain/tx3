#![allow(clippy::len_without_is_empty)]
use crate::*;

const ID__MASK: u64 = 0x0000ffffffffffff;
const LEN_MASK: u64 = 0xffff000000000000;

/// A frame marker letting us know the type, id, and length of a frame
#[derive(Clone, Copy)]
pub struct Mark(u64);

impl std::fmt::Debug for Mark {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let id = self.id();
        let len = self.len();
        f.debug_struct("Mark")
            .field("id", &id)
            .field("len", &len)
            .finish()
    }
}

impl Mark {
    /// The maximum frame ID
    pub const MAX_ID: u64 = ID__MASK;

    /// Construct a new frame mark from components
    pub fn new(id: u64, len: u16) -> Result<Self> {
        if id > ID__MASK {
            return Err(other_err("IdOverflow"));
        }
        let len = (len as u64) << 48;
        Ok(Self(len | id))
    }

    /// Get the raw frame mark u64
    pub fn raw(&self) -> u64 {
        self.0
    }

    /// Get the id of this frame mark
    pub fn id(&self) -> u64 {
        self.0 & ID__MASK
    }

    /// Get the length of this frame mark
    pub fn len(&self) -> u16 {
        ((self.0 & LEN_MASK) >> 48) as u16
    }
}
